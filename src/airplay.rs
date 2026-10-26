use capture::sources::sine::SineSource;
use capture::AudioSource;
use pipeline::{PipelineMetricsTracker, DEFAULT_RING_BUFFER_PACKETS};
use raop::ap2::session::{Ap2Config, Ap2Session};
use std::net::SocketAddr;
use std::sync::Arc;
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

#[derive(Debug)]
enum Control {
    Volume(Volume),
    Stop,
}

pub struct StreamHandle {
    control: mpsc::UnboundedSender<Control>,
    worker: JoinHandle<()>,
}

impl StreamHandle {
    #[cfg(windows)]
    pub fn start(target: SocketAddr, endpoint: String, initial_volume: Volume) -> Self {
        let (control, receiver) = mpsc::unbounded_channel();
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
                run_session(target, Box::new(source), initial_volume, receiver, None).await
            });
            if let Err(error) = result {
                eprintln!("AirPlay stream to {target} ended: {error}");
            }
        });
        Self { control, worker }
    }

    pub fn set_volume(&self, volume: Volume) {
        let _ = self.control.send(Control::Volume(volume));
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
            Volume {
                scalar: 0.35,
                muted: false,
            },
            receiver,
            Some(duration),
        )
        .await
    })
}

async fn run_session(
    target: SocketAddr,
    source: Box<dyn AudioSource + Send>,
    volume: Volume,
    mut control: mpsc::UnboundedReceiver<Control>,
    duration: Option<Duration>,
) -> Result<(), String> {
    let config = Ap2Config {
        target,
        latency_frames: -4410,
        volume_db: volume.receiver_db(),
        quiet_mode: false,
        ..Default::default()
    };
    let metrics = Arc::new(PipelineMetricsTracker::new(DEFAULT_RING_BUFFER_PACKETS));
    let mut session =
        Ap2Session::start_with_source(config, source, metrics, DEFAULT_RING_BUFFER_PACKETS)
            .await
            .map_err(|error| error.to_string())?;
    let mut feedback = tokio::time::interval(Duration::from_secs(2));
    feedback.tick().await;
    let deadline = duration.map(|duration| tokio::time::Instant::now() + duration);
    loop {
        tokio::select! {
            message = control.recv() => match message {
                Some(Control::Volume(volume)) => {
                    if let Err(error) = session.set_volume(volume.receiver_db()).await {
                        eprintln!("Could not update receiver volume: {error}");
                    }
                }
                Some(Control::Stop) | None => break,
            },
            _ = feedback.tick() => {
                if let Err(error) = session.feedback().await {
                    eprintln!("AirPlay receiver disconnected: {error}");
                    break;
                }
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
    session.stop().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
