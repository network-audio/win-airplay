//! A USB Audio Class 2.0 capture function backed by an AirPlay PCM queue.

use super::{be32, string_descriptor, Device, DeviceState, EPIPE};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

#[cfg_attr(not(windows), allow(dead_code))]
const MAX_PCM_BYTES: usize = 44_100 * 2 * 2 / 2;
const SAMPLE_RATE: u32 = 44_100;

#[derive(Default)]
pub struct CaptureInput {
    connected: AtomicBool,
    changed: AtomicBool,
    notification: Mutex<()>,
    notification_ready: Condvar,
    pcm: Mutex<VecDeque<u8>>,
    packet_clock: Mutex<u32>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl CaptureInput {
    pub fn set_connected(&self, connected: bool) {
        if self.connected.swap(connected, Ordering::AcqRel) != connected {
            eprintln!("USB input connector state changed: connected={connected}");
            self.changed.store(true, Ordering::Release);
            self.notification_ready.notify_all();
            if !connected {
                self.pcm.lock().unwrap().clear();
            }
        }
    }

    pub fn write_pcm(&self, samples: &[i16]) {
        let mut pcm = self.pcm.lock().unwrap();
        for sample in samples {
            pcm.extend(sample.to_le_bytes());
        }
        while pcm.len() > MAX_PCM_BYTES {
            pcm.pop_front();
        }
    }

    pub fn clear(&self) {
        self.pcm.lock().unwrap().clear();
    }

    fn read(&self, len: usize) -> Vec<u8> {
        let mut pcm = self.pcm.lock().unwrap();
        (0..len).map(|_| pcm.pop_front().unwrap_or(0)).collect()
    }

    fn next_packet_bytes(&self) -> usize {
        let mut remainder = self.packet_clock.lock().unwrap();
        *remainder += SAMPLE_RATE;
        let frames = *remainder / 1000;
        *remainder %= 1000;
        frames as usize * 4
    }

    pub(super) fn take_change(&self) -> bool {
        self.changed.swap(false, Ordering::AcqRel)
    }

    pub(super) fn wait_for_change(&self) {
        let guard = self.notification.lock().unwrap();
        if !self.changed.load(Ordering::Acquire) {
            let _ = self
                .notification_ready
                .wait_timeout(guard, Duration::from_millis(250));
        }
    }
}

pub(super) fn handle_submit(
    input: &CaptureInput,
    state: &Mutex<DeviceState>,
    ep: u32,
    direction: u32,
    length: usize,
    iso: &[u8],
) -> (i32, Vec<u8>, usize, Vec<u8>) {
    if ep != 1 || direction != 1 || iso.is_empty() {
        return (EPIPE, Vec::new(), 0, Vec::new());
    }
    state.lock().unwrap().alternate = 1;
    let mut reply = iso.to_vec();
    let mut data = Vec::new();
    for entry in reply.as_chunks_mut::<16>().0 {
        let packet_len = (be32(&entry[4..8]) as usize)
            .min(input.next_packet_bytes())
            .min(length.saturating_sub(data.len()));
        data.extend(input.read(packet_len));
        entry[8..12].copy_from_slice(&(packet_len as u32).to_be_bytes());
        entry[12..16].fill(0);
    }
    let actual = data.len();
    (0, data, actual, reply)
}

pub(super) fn control(
    input: &CaptureInput,
    state: &Mutex<DeviceState>,
    device: &Device,
    setup: [u8; 8],
    _payload: &[u8],
) -> Result<Vec<u8>, i32> {
    let request_type = setup[0];
    let request = setup[1];
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    let length = u16::from_le_bytes([setup[6], setup[7]]) as usize;
    let mut state = state.lock().unwrap();
    let data = match (request_type, request) {
        (0x80, 6) => match (value >> 8) as u8 {
            1 => device_descriptor(),
            2 => configuration_descriptor(),
            3 => string_descriptor(device, value as u8)?,
            _ => return Err(EPIPE),
        },
        (0x00, 5) => Vec::new(),
        (0x00, 9) if value <= 1 => {
            state.configuration = value as u8;
            Vec::new()
        }
        (0x80, 8) => vec![state.configuration],
        (0x01, 11) if index == 1 && value <= 1 => {
            state.alternate = value as u8;
            Vec::new()
        }
        (0x81, 10) if index == 1 => vec![state.alternate],
        (0x80, 0) | (0x81, 0) | (0x82, 0) => vec![0, 0],
        (0x02, 1) if index == 0x81 || index == 0x82 => Vec::new(),
        // Clock Source 4: a fixed 44.1-kHz clock.
        (0xa1, 0x01) if index == 0x0400 && value == 0x0100 => SAMPLE_RATE.to_le_bytes().to_vec(),
        (0xa1, 0x02) if index == 0x0400 && value == 0x0100 => {
            let mut range = vec![1, 0];
            range.extend(SAMPLE_RATE.to_le_bytes());
            range.extend(SAMPLE_RATE.to_le_bytes());
            range.extend(0_u32.to_le_bytes());
            range
        }
        // Terminal 1 Connector Control: all channels connected during an
        // AirPlay session, none otherwise.
        (0xa1, 0x01) if index == 0x0100 && value == 0x0200 => {
            let connected = input.connected.load(Ordering::Acquire);
            eprintln!("USB input connector GET_CUR: connected={connected}");
            if connected {
                vec![2, 3, 0, 0, 0, 0]
            } else {
                vec![0; 6]
            }
        }
        _ => return Err(EPIPE),
    };
    Ok(data[..data.len().min(length)].to_vec())
}

fn device_descriptor() -> Vec<u8> {
    vec![
        18, 1, 0, 2, 0xef, 2, 1, 64, 0xfe, 0xca, 2, 0x40, 0, 1, 1, 2, 3, 1,
    ]
}

fn configuration_descriptor() -> Vec<u8> {
    let mut out = vec![
        9, 2, 0, 0, 2, 1, 0, 0x80, 50, // configuration
        8, 11, 0, 2, 1, 0, 0x20, 0, // interface association
        9, 4, 0, 0, 1, 1, 1, 0x20, 0, // AudioControl plus interrupt IN
        9, 0x24, 1, 0, 2, 3, 46, 0, 0, // AC header, microphone function
        8, 0x24, 0x0a, 4, 1, 1, 0, 0, // Clock Source 4, fixed, readable
        17, 0x24, 2, 1, 1, 2, 0, 4, 2, 3, 0, 0, 0, 0, 4, 0, 0, // Mic Input Terminal
        12, 0x24, 3, 3, 1, 1, 0, 1, 4, 0, 0, 0, // USB-streaming Output Terminal
        7, 5, 0x82, 3, 6, 0, 8, // class notification IN
        9, 4, 1, 0, 0, 1, 2, 0x20, 0, // AudioStreaming alt 0
        9, 4, 1, 1, 1, 1, 2, 0x20, 0, // AudioStreaming alt 1
        16, 0x24, 1, 3, 0, 1, 1, 0, 0, 0, 2, 3, 0, 0, 0, 0, // AS PCM stereo
        6, 0x24, 2, 1, 2, 16, // 16-bit Type I format
        7, 5, 0x81, 5, 180, 0, 4, // asynchronous isochronous IN, 1 ms
        8, 0x25, 1, 0, 0, 0, 0, 0, // class-specific endpoint
    ];
    let length = out.len() as u16;
    out[2..4].copy_from_slice(&length.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_has_uac2_capture_and_connector_control() {
        let config = configuration_descriptor();
        assert_eq!(
            u16::from_le_bytes([config[2], config[3]]) as usize,
            config.len()
        );
        assert!(config
            .windows(17)
            .any(|part| part == [17, 0x24, 2, 1, 1, 2, 0, 4, 2, 3, 0, 0, 0, 0, 4, 0, 0]));
        assert!(config
            .windows(7)
            .any(|part| part == [7, 5, 0x81, 5, 180, 0, 4]));
    }

    #[test]
    fn pcm_queue_returns_samples_then_silence() {
        let input = CaptureInput::default();
        input.write_pcm(&[0x1234]);
        assert_eq!(input.read(4), [0x34, 0x12, 0, 0]);
    }

    #[test]
    fn input_clock_produces_exactly_441_frames_per_10ms() {
        let input = CaptureInput::default();
        let bytes: usize = (0..10).map(|_| input.next_packet_bytes()).sum();
        assert_eq!(bytes, 441 * 4);
    }

    #[test]
    fn uac2_clock_and_connector_requests_use_get_cur_and_get_range() {
        let server = crate::usbip_audio::Server::start(0).unwrap();
        let streams = server.add_input("capture-test", "Test Input");
        let device = server
            .devices
            .read()
            .unwrap()
            .get(&streams.busid)
            .unwrap()
            .clone();
        let range = control(
            &streams.input,
            &device.state,
            &device,
            [0xa1, 2, 0, 1, 0, 4, 255, 0],
            &[],
        )
        .unwrap();
        assert_eq!(range.len(), 14);
        let disconnected = control(
            &streams.input,
            &device.state,
            &device,
            [0xa1, 1, 0, 2, 0, 1, 6, 0],
            &[],
        )
        .unwrap();
        assert_eq!(disconnected, [0; 6]);
        streams.input.set_connected(true);
        let connected = control(
            &streams.input,
            &device.state,
            &device,
            [0xa1, 1, 0, 2, 0, 1, 6, 0],
            &[],
        )
        .unwrap();
        assert_eq!(connected, [2, 3, 0, 0, 0, 0]);
    }
}
