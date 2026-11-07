//! The AirPlay receiver's microphone: a USB Audio 2.0 capture device of the shared usbip-audio crate. Its
//! connector is plugged in while an AirPlay sender is connected, so Windows shows the input unplugged otherwise.
//!
//! The server completes capture transfers at the end of the time they cover, so Windows takes the PCM at the
//! rate it plays, however far ahead it queues its requests.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use usbip_audio::uac2::{Uac2Config, Uac2Device, Uac2Streams};

pub const SAMPLE_RATE: u32 = 44_100;
const CHANNELS: u16 = 2;
/// Prototype id; production requires an assigned VID/PID
const VENDOR_ID: u16 = 0xcafe;
const PRODUCT_ID: u16 = 0x4002;
/// Half a second: older audio is dropped
#[cfg_attr(not(windows), allow(dead_code))]
const MAX_QUEUED_SAMPLES: usize = SAMPLE_RATE as usize * CHANNELS as usize / 2;
/// Frames between statistics in the log: 10 s
const STATS_FRAMES: u64 = SAMPLE_RATE as u64 * 10;

/// PCM from the AirPlay receiver, waiting for Windows to record it.
#[derive(Default)]
pub struct Queue {
    samples: Mutex<VecDeque<i16>>,
    underflow_frames: AtomicU64,
    overwritten_frames: AtomicU64,
    captured_frames: AtomicU64,
}

impl Queue {
    fn reset_stats(&self) {
        self.underflow_frames.store(0, Ordering::Relaxed);
        self.overwritten_frames.store(0, Ordering::Relaxed);
        self.captured_frames.store(0, Ordering::Relaxed);
    }
}

impl Uac2Streams for Queue {
    fn playback(&self, _frame: u64, _samples: &[i32]) {}

    /// Windows records the queued PCM, and silence once it runs out.
    fn capture(&self, _frame: u64, samples: &mut [i32]) {
        let mut queue = self.samples.lock().unwrap();
        let available = samples.len().min(queue.len());
        for (sample, pcm) in samples.iter_mut().zip(queue.drain(..available)) {
            *sample = i32::from(pcm) << 16;
        }
        samples[available..].fill(0);
        let queued = queue.len();
        drop(queue);
        let channels = usize::from(CHANNELS);
        self.underflow_frames.fetch_add(
            ((samples.len() - available) / channels) as u64,
            Ordering::Relaxed,
        );
        let frames = (samples.len() / channels) as u64;
        let before = self.captured_frames.fetch_add(frames, Ordering::Relaxed);
        if (before + frames) / STATS_FRAMES != before / STATS_FRAMES {
            eprintln!(
                "USB input audio: frames={} underflow_frames={} overwritten_frames={} queued_ms={}",
                before + frames,
                self.underflow_frames.load(Ordering::Relaxed),
                self.overwritten_frames.load(Ordering::Relaxed),
                (queued / channels) as u64 * 1000 / u64::from(SAMPLE_RATE)
            );
        }
    }
}

pub type InputDevice = Uac2Device<Arc<Queue>>;

pub struct CaptureInput {
    device: Arc<InputDevice>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl CaptureInput {
    pub(super) fn new(name: &str, serial: &str) -> Self {
        let config = Uac2Config {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            bcd_device: 0x0200,
            manufacturer: "Win AirPlay".into(),
            product: name.to_owned(),
            serial: serial.to_owned(),
            sample_rate: SAMPLE_RATE,
            playback_channels: 0,
            capture_channels: CHANNELS,
            playback_name: None,
            capture_name: None,
            capture_connector: true,
        };
        let device = Uac2Device::new(config, Arc::new(Queue::default()))
            .expect("a stereo 44.1 kHz capture device is valid");
        Self {
            device: Arc::new(device),
        }
    }

    pub(super) fn device(&self) -> Arc<InputDevice> {
        self.device.clone()
    }

    fn queue(&self) -> &Queue {
        self.device.streams()
    }

    /// Plugs the input in while an AirPlay sender is connected. Unplugging drops the queued audio.
    pub fn set_connected(&self, connected: bool) {
        if self.device.capture_connected() == connected {
            return;
        }
        eprintln!("USB input connector state changed: connected={connected}");
        if connected {
            self.queue().reset_stats();
        } else {
            self.clear();
        }
        self.device.set_capture_connected(connected);
    }

    #[cfg(test)]
    fn connected(&self) -> bool {
        self.device.capture_connected()
    }

    /// Queues interleaved stereo PCM at 44.1 kHz, dropping the oldest beyond half a second.
    pub fn write_pcm(&self, samples: &[i16]) {
        let mut queue = self.queue().samples.lock().unwrap();
        queue.extend(samples);
        let overwritten = queue.len().saturating_sub(MAX_QUEUED_SAMPLES);
        queue.drain(..overwritten);
        drop(queue);
        self.queue().overwritten_frames.fetch_add(
            (overwritten / usize::from(CHANNELS)) as u64,
            Ordering::Relaxed,
        );
    }

    pub fn clear(&self) {
        self.queue().samples.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_pcm_is_recorded_then_silence() {
        let input = CaptureInput::new("Test Input", "test");
        input.write_pcm(&[0x1234, -2]);
        let mut samples = [7; 4];
        input.queue().capture(0, &mut samples);
        assert_eq!(samples, [0x1234 << 16, -2 << 16, 0, 0]);
        assert_eq!(input.queue().underflow_frames.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn the_queue_keeps_the_latest_half_second() {
        let input = CaptureInput::new("Test Input", "test");
        let pcm: Vec<i16> = (0..MAX_QUEUED_SAMPLES + 4).map(|i| i as i16).collect();
        input.write_pcm(&pcm);
        let queue = input.queue().samples.lock().unwrap();
        assert_eq!(queue.len(), MAX_QUEUED_SAMPLES);
        assert_eq!(queue.front(), Some(&4));
        drop(queue);
        assert_eq!(input.queue().overwritten_frames.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn unplugged_until_connected_and_unplugging_drops_the_audio() {
        let input = CaptureInput::new("Test Input", "test");
        assert!(!input.connected());
        input.set_connected(true);
        assert!(input.connected());
        input.write_pcm(&[1, 2]);
        input.set_connected(false);
        assert!(!input.connected());
        assert!(input.queue().samples.lock().unwrap().is_empty());
    }
}
