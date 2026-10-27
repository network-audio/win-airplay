//! One virtual USB Audio 1.0 speaker per receiver, served over USB/IP by the shared
//! [usbip-audio](https://github.com/stakach/usbip-audio) crate.

use std::collections::VecDeque;
use std::io;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use std::time::Instant;
use usbip_audio::uac1::{Uac1Speaker, Uac1SpeakerConfig, Uac1SpeakerStreams};
use usbip_audio::MonotonicClock;

pub use usbip_audio::uac1::Volume;

const MAX_QUEUED_PCM_PACKETS: usize = 24;
const ACTIVITY_SAMPLE_THRESHOLD: i16 = 64;
/// Prototype id; production requires an assigned VID/PID
const VENDOR_ID: u16 = 0xcafe;
const PRODUCT_ID: u16 = 0x4001;

#[cfg_attr(not(windows), allow(dead_code))]
pub struct DeviceStreams {
    pub busid: String,
    pub pcm: Arc<PcmQueue>,
    pub volume: Receiver<Volume>,
}

#[derive(Default)]
pub struct PcmQueue {
    packets: Mutex<VecDeque<Vec<u8>>>,
    ready: Condvar,
    last_input: Mutex<Option<Instant>>,
}

impl PcmQueue {
    fn push(&self, packet: Vec<u8>) {
        // Windows can send tiny dither samples when no application is playing.
        // Keep quiet packets during an active stream, but do not let them keep
        // an idle AirPlay session alive indefinitely.
        let active = packet.as_chunks::<2>().0.iter().any(|sample| {
            i16::from_le_bytes(*sample).unsigned_abs() > ACTIVITY_SAMPLE_THRESHOLD as u16
        });
        let mut last_input = self.last_input.lock().unwrap();
        let restarting =
            active && last_input.is_none_or(|time| time.elapsed() >= Duration::from_secs(1));
        if active {
            *last_input = Some(Instant::now());
        } else if last_input.is_none_or(|time| time.elapsed() >= Duration::from_secs(10)) {
            return;
        }
        drop(last_input);
        let mut packets = self.packets.lock().unwrap();
        if restarting {
            packets.clear();
        }
        if packets.len() == MAX_QUEUED_PCM_PACKETS {
            packets.pop_front();
        }
        packets.push_back(packet);
        self.ready.notify_one();
    }

    #[cfg(any(windows, test))]
    pub fn has_data(&self) -> bool {
        !self.packets.lock().unwrap().is_empty()
    }

    #[cfg(any(windows, test))]
    pub fn idle_for(&self) -> Option<Duration> {
        self.last_input.lock().unwrap().map(|time| time.elapsed())
    }

    pub fn pop_timeout(&self, timeout: Duration) -> Option<Vec<u8>> {
        let mut packets = self.packets.lock().unwrap();
        if packets.is_empty() {
            packets = self.ready.wait_timeout(packets, timeout).unwrap().0;
        }
        packets.pop_front()
    }

    #[cfg(any(windows, test))]
    pub fn clear(&self) {
        self.packets.lock().unwrap().clear();
    }
}

struct SpeakerStreams {
    pcm: Arc<PcmQueue>,
    volume_events: SyncSender<Volume>,
}

impl Uac1SpeakerStreams for SpeakerStreams {
    fn pcm(&self, data: &[u8]) {
        self.pcm.push(data.to_vec());
    }
    fn volume(&self, volume: Volume) {
        let _ = self.volume_events.try_send(volume);
    }
}

pub struct Server {
    inner: usbip_audio::Server,
}

impl Server {
    /// Listens on `port` of the loopback interface, 0 for any free port, so that other applications using
    /// the usbip-win2 driver can run their own servers alongside.
    pub fn start(port: u16) -> io::Result<Self> {
        let inner = usbip_audio::Server::start(port, Arc::new(MonotonicClock::new()))?;
        Ok(Self { inner })
    }

    pub fn port(&self) -> u16 {
        self.inner.port()
    }

    pub fn add(&self, key: &str, name: &str, delay_ms: u8) -> DeviceStreams {
        let busid = format!("1-{}", &key[..key.len().min(16)]);
        let pcm = Arc::new(PcmQueue::default());
        let (volume_events, volume) = mpsc::sync_channel(16);
        let config = Uac1SpeakerConfig {
            vendor_id: VENDOR_ID,
            product_id: PRODUCT_ID,
            manufacturer: "Win AirPlay".into(),
            product: name.to_owned(),
            serial: key.to_owned(),
            delay_ms,
        };
        let streams = SpeakerStreams { pcm: pcm.clone(), volume_events };
        self.inner.add(&busid, Arc::new(Uac1Speaker::new(config, streams)));
        DeviceStreams { busid, pcm, volume }
    }

    pub fn remove(&self, busid: &str) {
        self.inner.remove(busid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{Ipv4Addr, TcpStream};

    fn put_u32(out: &mut Vec<u8>, value: u32) {
        out.extend_from_slice(&value.to_be_bytes());
    }

    fn be32(bytes: &[u8]) -> u32 {
        u32::from_be_bytes(bytes[..4].try_into().unwrap())
    }

    fn fixed(out: &mut Vec<u8>, text: &str, width: usize) {
        out.extend_from_slice(text.as_bytes());
        out.resize(out.len() + width - text.len(), 0);
    }

    #[test]
    fn idle_silence_does_not_trigger_a_stream() {
        let queue = PcmQueue::default();
        queue.push(vec![0; 1764]);
        queue.push(vec![1, 0, 2, 0]);
        assert!(!queue.has_data());
        assert!(queue.idle_for().is_none());
        queue.push(vec![100, 0, 0, 0]);
        assert!(queue.has_data());
    }

    #[test]
    fn pcm_queue_keeps_recent_audio_only() {
        let queue = PcmQueue::default();
        for index in 0..(MAX_QUEUED_PCM_PACKETS + 4) {
            queue.push(vec![100, 0, index as u8, 0]);
        }
        assert_eq!(queue.packets.lock().unwrap().len(), MAX_QUEUED_PCM_PACKETS);
        assert_eq!(queue.pop_timeout(Duration::ZERO).unwrap()[2], 4);
    }

    #[test]
    fn clearing_stale_pcm_preserves_input_activity() {
        let queue = PcmQueue::default();
        queue.push(vec![100, 0, 0, 0]);
        queue.clear();
        assert!(!queue.has_data());
        assert!(queue.idle_for().is_some());
        queue.push(vec![100, 0, 1, 0]);
        assert_eq!(queue.pop_timeout(Duration::ZERO), Some(vec![100, 0, 1, 0]));
    }

    #[test]
    fn device_list_and_import_are_protocol_sized() {
        let server = Server::start(0).unwrap();
        let streams = server.add("0123456789abcdef", "Kitchen", 180);
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port())).unwrap();
        client.write_all(&[1, 0x11, 0x80, 5, 0, 0, 0, 0]).unwrap();
        let mut header = [0; 12];
        client.read_exact(&mut header).unwrap();
        assert_eq!(&header[..4], &[1, 0x11, 0, 5]);
        assert_eq!(be32(&header[8..12]), 1);
        let mut info = [0; 320];
        client.read_exact(&mut info).unwrap();
        assert!(info[256..288].starts_with(streams.busid.as_bytes()));
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port())).unwrap();
        let mut request = vec![1, 0x11, 0x80, 3, 0, 0, 0, 0];
        fixed(&mut request, &streams.busid, 32);
        client.write_all(&request).unwrap();
        let mut reply = [0; 8];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(reply, [1, 0x11, 0, 3, 0, 0, 0, 0]);
        server.remove(&streams.busid);
    }

    #[test]
    fn control_volume_and_iso_pcm_round_trip() {
        let server = Server::start(0).unwrap();
        let streams = server.add("0123456789abcdef", "Kitchen", 180);
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, server.port())).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = vec![1, 0x11, 0x80, 3, 0, 0, 0, 0];
        fixed(&mut request, &streams.busid, 32);
        client.write_all(&request).unwrap();
        let mut import = [0; 320];
        client.read_exact(&mut import).unwrap();
        assert_eq!(&import[..8], &[1, 0x11, 0, 3, 0, 0, 0, 0]);

        // GET_DESCRIPTOR(Configuration), then SET_CONFIGURATION and SET_INTERFACE.
        let get_config = submit(1, 1, 0, [0x80, 6, 0, 2, 0, 0, 255, 0], &[], u32::MAX, &[]);
        client.write_all(&get_config).unwrap();
        let mut reply = [0; 48];
        client.read_exact(&mut reply).unwrap();
        assert_eq!(be32(&reply[20..24]), 0);
        let length = be32(&reply[24..28]) as usize;
        assert_eq!(length, 110, "the whole configuration descriptor");
        let mut config = vec![0; length];
        client.read_exact(&mut config).unwrap();
        assert_eq!(&config[76..83], &[7, 0x24, 1, 1, 180, 1, 0], "bDelay");
        for (sequence, setup) in [[0, 9, 1, 0, 0, 0, 0, 0], [1, 11, 1, 0, 1, 0, 0, 0]]
            .into_iter()
            .enumerate()
        {
            client
                .write_all(&submit(
                    sequence as u32 + 2,
                    0,
                    0,
                    setup,
                    &[],
                    u32::MAX,
                    &[],
                ))
                .unwrap();
            client.read_exact(&mut reply).unwrap();
            assert_eq!(be32(&reply[20..24]), 0);
        }

        let volume = (-6_i16 * 256).to_le_bytes();
        client
            .write_all(&submit(
                4,
                0,
                0,
                [0x21, 1, 0, 2, 0, 2, 2, 0],
                &volume,
                u32::MAX,
                &[],
            ))
            .unwrap();
        client.read_exact(&mut reply).unwrap();
        assert_eq!(be32(&reply[20..24]), 0);
        let event = streams.volume.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!((event.scalar - 0.5).abs() < 0.01);

        let mut iso = Vec::new();
        for (offset, length) in [(0_u32, 4_u32), (4, 4)] {
            put_u32(&mut iso, offset);
            put_u32(&mut iso, length);
            iso.extend_from_slice(&[0; 8]);
        }
        let pcm = [0_u8, 0, 0xff, 0x7f, 0, 0, 0xff, 0x7f];
        client
            .write_all(&submit(5, 0, 1, [0; 8], &pcm, 2, &iso))
            .unwrap();
        client.read_exact(&mut reply).unwrap();
        assert_eq!(be32(&reply[20..24]), 0);
        assert_eq!(be32(&reply[24..28]), 8);
        let mut iso_reply = [0; 32];
        client.read_exact(&mut iso_reply).unwrap();
        assert_eq!(be32(&iso_reply[8..12]), 4);
        assert_eq!(
            streams.pcm.pop_timeout(Duration::from_secs(2)).unwrap(),
            pcm
        );

        // A short Windows isochronous transfer can contain fewer data bytes
        // than the sum of the requested per-packet lengths.
        let short_pcm = &pcm[..6];
        client
            .write_all(&submit(6, 0, 1, [0; 8], short_pcm, 2, &iso))
            .unwrap();
        client.read_exact(&mut reply).unwrap();
        assert_eq!(be32(&reply[20..24]), 0);
        assert_eq!(be32(&reply[24..28]), 6);
        client.read_exact(&mut iso_reply).unwrap();
        assert_eq!(be32(&iso_reply[8..12]), 4);
        assert_eq!(be32(&iso_reply[24..28]), 2);
        assert_eq!(
            streams.pcm.pop_timeout(Duration::from_secs(2)).unwrap(),
            short_pcm
        );

        // usbip-win2 may handle SET_INTERFACE itself. Audio URBs must still
        // reach the PCM queue when the server last observed alternate 0.
        client
            .write_all(&submit(
                7,
                0,
                0,
                [1, 11, 0, 0, 1, 0, 0, 0],
                &[],
                u32::MAX,
                &[],
            ))
            .unwrap();
        client.read_exact(&mut reply).unwrap();
        assert_eq!(be32(&reply[20..24]), 0);
        client
            .write_all(&submit(8, 0, 1, [0; 8], &pcm, 2, &iso))
            .unwrap();
        client.read_exact(&mut reply).unwrap();
        assert_eq!(be32(&reply[20..24]), 0);
        client.read_exact(&mut iso_reply).unwrap();
        assert_eq!(
            streams.pcm.pop_timeout(Duration::from_secs(2)).unwrap(),
            pcm
        );
    }

    fn submit(
        seq: u32,
        direction: u32,
        ep: u32,
        setup: [u8; 8],
        payload: &[u8],
        packets: u32,
        iso: &[u8],
    ) -> Vec<u8> {
        let mut out = Vec::new();
        for value in [
            1,
            seq,
            0x0001_0001,
            direction,
            ep,
            0,
            if direction == 1 {
                u16::from_le_bytes([setup[6], setup[7]]) as u32
            } else {
                payload.len() as u32
            },
            0,
            packets,
            1,
        ] {
            put_u32(&mut out, value);
        }
        out.extend_from_slice(&setup);
        out.extend_from_slice(payload);
        out.extend_from_slice(iso);
        out
    }
}
