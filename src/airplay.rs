use crate::{AudioEncoding, StreamProfile};
use capture::sources::sine::SineSource;
use capture::AudioSource;
use pipeline::{PipelineMetricsTracker, DEFAULT_RING_BUFFER_PACKETS};
use raop::ap2::session::{Ap2AudioCodec, Ap2Config, Ap2Session};
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::sync::RwLock;
use std::thread::{self, JoinHandle};
use std::time::Duration;
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug)]
pub struct Volume {
    pub scalar: f32,
    pub muted: bool,
}

impl Volume {
    fn receiver_db(self) -> f32 {
        raop::volume::volume_to_db(self.scalar.clamp(0.0, 1.0) * 100.0, self.muted)
    }
}

/// A zero event without a preceding move toward zero is ambiguous: some
/// receivers send it as an unsolicited status update while audio is playing.
fn accept_receiver_volume(current: f32, incoming: f32) -> bool {
    incoming > 0.001 || current <= 0.15
}

#[derive(Debug)]
enum Control {
    Volume(Volume),
    Stop,
}

pub struct StreamHandle {
    control: mpsc::UnboundedSender<Control>,
    worker: JoinHandle<()>,
    channel_gains: Arc<RwLock<Vec<f32>>>,
}

struct BalancedSource {
    inner: Box<dyn AudioSource + Send>,
    channel_gains: Arc<RwLock<Vec<f32>>>,
}

struct UsbPcmSource {
    queue: Arc<crate::usbip_audio::PcmQueue>,
    pending: VecDeque<f32>,
}

impl AudioSource for UsbPcmSource {
    fn sample_rate(&self) -> u32 {
        44_100
    }
    fn channels(&self) -> u16 {
        2
    }
    fn is_live(&self) -> bool {
        true
    }

    fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // The USB endpoint starts delivering PCM while AirPlay pairing runs.
        // Begin each new session with current Windows audio.
        self.queue.clear();
        self.pending.clear();
        Ok(())
    }

    fn read_chunk(
        &mut self,
        max_frames: usize,
    ) -> Result<Option<capture::AudioChunk>, Box<dyn std::error::Error + Send + Sync>> {
        if self.pending.is_empty() {
            if let Some(bytes) = self.queue.pop_timeout(Duration::from_millis(10)) {
                for pair in bytes.as_chunks::<2>().0 {
                    self.pending
                        .push_back(i16::from_le_bytes([pair[0], pair[1]]) as f32 / 32768.0);
                }
            }
        }
        if self.pending.is_empty() {
            return Ok(None);
        }
        let samples = (max_frames * 2).min(self.pending.len() & !1);
        let data = self.pending.drain(..samples).collect();
        Ok(Some(capture::AudioChunk::new(
            data,
            44_100,
            2,
            std::time::Instant::now(),
        )))
    }
}

impl AudioSource for BalancedSource {
    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn read_chunk(
        &mut self,
        max_frames: usize,
    ) -> Result<Option<capture::AudioChunk>, Box<dyn std::error::Error + Send + Sync>> {
        let mut chunk = match self.inner.read_chunk(max_frames)? {
            Some(chunk) => chunk,
            None => return Ok(None),
        };
        let gains = self
            .channel_gains
            .read()
            .unwrap_or_else(|error| error.into_inner());
        let channels = chunk.channels as usize;
        if channels > 0 {
            for frame in chunk.data.chunks_mut(channels) {
                for (channel, sample) in frame.iter_mut().enumerate() {
                    *sample *= gains.get(channel).copied().unwrap_or(1.0);
                }
            }
        }
        Ok(Some(chunk))
    }

    fn start(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.start()
    }

    fn is_live(&self) -> bool {
        self.inner.is_live()
    }

    fn stop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.inner.stop()
    }
}

impl StreamHandle {
    pub fn start_usbip(
        target: SocketAddr,
        queue: Arc<crate::usbip_audio::PcmQueue>,
        profile: StreamProfile,
        initial_volume: Volume,
        initial_channel_gains: Vec<f32>,
        remote_volume: Option<Sender<f32>>,
    ) -> Self {
        let (control, receiver) = mpsc::unbounded_channel();
        let channel_gains = Arc::new(RwLock::new(initial_channel_gains));
        let worker_gains = channel_gains.clone();
        let worker = thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("AirPlay runtime failed: {error}");
                    return;
                }
            };
            let source = UsbPcmSource {
                queue,
                pending: VecDeque::new(),
            };
            let balanced = BalancedSource {
                inner: Box::new(source),
                channel_gains: worker_gains,
            };
            if let Err(error) = runtime.block_on(run_session(
                target,
                Box::new(balanced),
                profile,
                initial_volume,
                receiver,
                remote_volume,
                None,
            )) {
                eprintln!("AirPlay USB stream to {target} ended: {error}");
            }
        });
        Self {
            control,
            worker,
            channel_gains,
        }
    }

    #[cfg(windows)]
    pub fn start(
        target: SocketAddr,
        endpoint: String,
        profile: StreamProfile,
        initial_volume: Volume,
        initial_channel_gains: Vec<f32>,
        remote_volume: Option<Sender<f32>>,
    ) -> Self {
        let (control, receiver) = mpsc::unbounded_channel();
        let channel_gains = Arc::new(RwLock::new(initial_channel_gains));
        let worker_gains = channel_gains.clone();
        let worker = thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("AirPlay runtime failed: {error}");
                    return;
                }
            };
            let result = runtime.block_on(async move {
                let source = capture::wasapi::WasapiLoopbackSource::with_config(
                    capture::wasapi::WasapiLoopbackConfig {
                        device_spec: capture::wasapi::DeviceSpec::ByName(endpoint),
                        ..Default::default()
                    },
                )
                .map_err(|error| error.to_string())?;
                let balanced = BalancedSource {
                    inner: Box::new(source),
                    channel_gains: worker_gains,
                };
                run_session(
                    target,
                    Box::new(balanced),
                    profile,
                    initial_volume,
                    receiver,
                    remote_volume,
                    None,
                )
                .await
            });
            if let Err(error) = result {
                eprintln!("AirPlay stream to {target} ended: {error}");
            }
        });
        Self {
            control,
            worker,
            channel_gains,
        }
    }

    pub fn set_volume(&self, volume: Volume) {
        let _ = self.control.send(Control::Volume(volume));
    }

    pub fn set_channel_gains(&self, gains: Vec<f32>) {
        *self
            .channel_gains
            .write()
            .unwrap_or_else(|error| error.into_inner()) = gains;
    }

    pub fn is_finished(&self) -> bool {
        self.worker.is_finished()
    }

    pub fn stop(self) {
        let _ = self.control.send(Control::Stop);
        // The worker owns the network teardown. Detaching avoids blocking device
        // discovery if a receiver has disappeared from the network.
    }
}

pub fn test_tone(target: SocketAddr, duration: Duration) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let source = SineSource::new(440.0, -24.0, 44_100, 2)
            .with_duration(duration)
            .with_beeps(2);
        let (_control, receiver) = mpsc::unbounded_channel();
        run_session(
            target,
            Box::new(source),
            StreamProfile::default(),
            Volume {
                scalar: 0.35,
                muted: false,
            },
            receiver,
            None,
            Some(duration),
        )
        .await
    })
}

async fn run_session(
    target: SocketAddr,
    source: Box<dyn AudioSource + Send>,
    profile: StreamProfile,
    volume: Volume,
    mut control: mpsc::UnboundedReceiver<Control>,
    remote_volume: Option<Sender<f32>>,
    duration: Option<Duration>,
) -> Result<(), String> {
    let config = Ap2Config {
        target,
        latency_frames: profile.latency_frames,
        start_lead_ms: profile.start_lead_ms,
        prefill_ms: profile.prefill_ms(),
        codec: match profile.encoding {
            AudioEncoding::Pcm => Ap2AudioCodec::Pcm,
            AudioEncoding::Alac => Ap2AudioCodec::Alac,
            AudioEncoding::CompressedAlac => Ap2AudioCodec::AlacCompressed,
        },
        volume_db: volume.receiver_db(),
        quiet_mode: false,
        initial_progress: true,
        ..Default::default()
    };
    let metrics = Arc::new(PipelineMetricsTracker::new(DEFAULT_RING_BUFFER_PACKETS));
    let mut session =
        Ap2Session::start_with_source(config, source, metrics.clone(), DEFAULT_RING_BUFFER_PACKETS)
            .await
            .map_err(|error| error.to_string())?;
    let stats = session.stats();
    let startup_lead_ms = {
        let now = std::time::Instant::now();
        if session.start_instant() >= now {
            session.start_instant().duration_since(now).as_millis() as i64
        } else {
            -(now.duration_since(session.start_instant()).as_millis() as i64)
        }
    };
    eprintln!(
        "AirPlay session started: target={target} codec={:?} latency_frames={} start_lead_ms={} prefill_ms={} startup_lead_remaining_ms={startup_lead_ms} volume_db={:.1}",
        profile.encoding,
        profile.latency_frames,
        profile.start_lead_ms,
        profile.prefill_ms(),
        volume.receiver_db()
    );
    let mut remote_commands = session.take_remote_commands();
    let mut current_volume = volume.scalar;
    let mut feedback = tokio::time::interval(Duration::from_secs(2));
    feedback.tick().await;
    let mut report = tokio::time::interval(Duration::from_secs(10));
    report.tick().await;
    let deadline = duration.map(|duration| tokio::time::Instant::now() + duration);
    loop {
        tokio::select! {
            message = control.recv() => match message {
                Some(Control::Volume(volume)) => {
                    if let Err(error) = session.set_volume(volume.receiver_db()).await {
                        eprintln!("Could not update receiver volume: {error}");
                    } else {
                        current_volume = volume.scalar;
                    }
                }
                Some(Control::Stop) | None => break,
            },
            command = async {
                match &mut remote_commands {
                    Some(commands) => commands.recv().await,
                    None => std::future::pending().await,
                }
            } => match command {
                Some(raop::ap2::remote::Command::Volume(value)) => {
                    if !accept_receiver_volume(current_volume, value) {
                        eprintln!("Ignoring abrupt receiver zero volume: target={target} current={current_volume:.3} reported={value:.3}");
                        continue;
                    }
                    current_volume = value;
                    if let Some(sender) = &remote_volume {
                        let _ = sender.send(value.clamp(0.0, 1.0));
                    }
                }
                Some(_) => {},
                None => remote_commands = None,
            },
            _ = feedback.tick() => {
                if let Err(error) = session.feedback().await {
                    eprintln!("AirPlay receiver disconnected: {error}");
                    break;
                }
            },
            _ = report.tick() => {
                let m = metrics.snapshot();
                eprintln!("AirPlay audio target={target} sent={} send_errors={} dropped={} gaps={} buffer={}/{} retransmits={}",
                    m.packets_sent, stats.audio_send_errors.load(Ordering::Relaxed),
                    m.packets_dropped, m.capture_discontinuities,
                    m.buffer_fill_packets, m.buffer_capacity_packets,
                    stats.retransmit_requests.load(Ordering::Relaxed));
            },
            _ = async {
                if let Some(deadline) = deadline {
                    tokio::time::sleep_until(deadline).await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => break,
        }
    }
    let m = metrics.snapshot();
    eprintln!("AirPlay session ending: target={target} sent={} send_errors={} dropped={} gaps={} retransmits={}",
        m.packets_sent, stats.audio_send_errors.load(Ordering::Relaxed),
        m.packets_dropped, m.capture_discontinuities,
        stats.retransmit_requests.load(Ordering::Relaxed));
    session.stop().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receiver_zero_needs_volume_near_zero() {
        assert!(!accept_receiver_volume(0.55, 0.0));
        assert!(accept_receiver_volume(0.55, 0.45));
        assert!(accept_receiver_volume(0.10, 0.0));
    }

    #[test]
    fn balance_applies_channel_gain_to_captured_audio() {
        let mut source = BalancedSource {
            inner: Box::new(SineSource::new(440.0, -12.0, 44_100, 2)),
            channel_gains: Arc::new(RwLock::new(vec![1.0, 0.0])),
        };
        let chunk = source.read_chunk(256).unwrap().unwrap();
        assert!(chunk.data.chunks(2).all(|frame| frame[1] == 0.0));
        assert!(chunk.data.chunks(2).any(|frame| frame[0].abs() > 0.0));
    }

    #[test]
    fn windows_volume_and_mute_map_to_receiver() {
        assert_eq!(
            Volume {
                scalar: 1.0,
                muted: false
            }
            .receiver_db(),
            0.0
        );
        assert_eq!(
            Volume {
                scalar: 0.0,
                muted: false
            }
            .receiver_db(),
            -144.0
        );
        assert_eq!(
            Volume {
                scalar: 0.5,
                muted: true
            }
            .receiver_db(),
            -144.0
        );
    }
}
