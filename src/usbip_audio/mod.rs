//! A loopback-only USB/IP server exporting one USB Audio 1.0 speaker per receiver.
//! The USB/IP framing follows https://www.kernel.org/doc/html/latest/usb/usbip_protocol.html.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread;
use std::time::Duration;
use std::time::Instant;

const VERSION: u16 = 0x0111;
const MAX_TRANSFER: usize = 1024 * 1024;
const MAX_ISO_PACKETS: usize = 1024;
const EPIPE: i32 = -32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    pub scalar: f32,
    pub muted: bool,
    pub channels: [f32; 2],
}

impl Default for Volume {
    fn default() -> Self {
        Self {
            scalar: 0.5,
            muted: false,
            channels: [1.0, 1.0],
        }
    }
}

struct Device {
    busid: String,
    devnum: u32,
    name: String,
    serial: String,
    active: AtomicBool,
    state: Mutex<DeviceState>,
    pcm: Arc<PcmQueue>,
    volume_events: SyncSender<Volume>,
}

#[derive(Default)]
struct DeviceState {
    configuration: u8,
    alternate: u8,
    volume: Volume,
}

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
        let mut packets = self.packets.lock().unwrap();
        if packets.len() == 64 {
            packets.pop_front();
        }
        packets.push_back(packet);
        *self.last_input.lock().unwrap() = Some(Instant::now());
        self.ready.notify_one();
    }

    #[cfg(windows)]
    pub fn has_data(&self) -> bool {
        !self.packets.lock().unwrap().is_empty()
    }

    #[cfg(windows)]
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
}

pub struct Server {
    devices: Arc<RwLock<BTreeMap<String, Arc<Device>>>>,
    stop: Arc<AtomicBool>,
    listener: Option<thread::JoinHandle<()>>,
    port: u16,
    next_devnum: AtomicU32,
}

impl Server {
    pub fn start(port: u16) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let devices: Arc<RwLock<BTreeMap<String, Arc<Device>>>> =
            Arc::new(RwLock::new(BTreeMap::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let worker_devices = devices.clone();
        let worker_stop = stop.clone();
        let worker = thread::Builder::new()
            .name("airplay-usbip".into())
            .spawn(move || {
                while !worker_stop.load(Ordering::Relaxed) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let devices = worker_devices.clone();
                            let stop = worker_stop.clone();
                            let _ = thread::Builder::new()
                                .name("airplay-usbip-client".into())
                                .spawn(move || {
                                    if let Err(error) = serve_client(stream, &devices, &stop) {
                                        if error.kind() != io::ErrorKind::UnexpectedEof
                                            && error.kind() != io::ErrorKind::ConnectionReset
                                        {
                                            eprintln!("USB/IP client ended: {error}");
                                        }
                                    }
                                });
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(30))
                        }
                        Err(error) => eprintln!("USB/IP accept failed: {error}"),
                    }
                }
            })?;
        Ok(Self {
            devices,
            stop,
            listener: Some(worker),
            port,
            next_devnum: AtomicU32::new(1),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn add(&self, key: &str, name: &str) -> DeviceStreams {
        let busid = format!("1-{}", &key[..key.len().min(16)]);
        let devnum = self.next_devnum.fetch_add(1, Ordering::Relaxed);
        let pcm = Arc::new(PcmQueue::default());
        let (volume_tx, volume) = mpsc::sync_channel(16);
        let device = Arc::new(Device {
            busid: busid.clone(),
            devnum,
            name: name.to_owned(),
            serial: key.to_owned(),
            active: AtomicBool::new(true),
            state: Mutex::new(DeviceState::default()),
            pcm: pcm.clone(),
            volume_events: volume_tx,
        });
        if let Some(previous) = self.devices.write().unwrap().insert(busid.clone(), device) {
            previous.active.store(false, Ordering::Release);
        }
        DeviceStreams { busid, pcm, volume }
    }

    pub fn remove(&self, busid: &str) {
        if let Some(device) = self.devices.write().unwrap().remove(busid) {
            device.active.store(false, Ordering::Release);
        }
    }

    #[cfg(windows)]
    pub fn set_volume(&self, busid: &str, volume: Volume) {
        if let Some(device) = self.devices.read().unwrap().get(busid) {
            device.state.lock().unwrap().volume = volume;
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.listener.take() {
            let _ = worker.join();
        }
        for device in self.devices.write().unwrap().values() {
            device.active.store(false, Ordering::Release);
        }
    }
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}
fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().unwrap())
}
fn fixed(out: &mut Vec<u8>, text: &str, width: usize) {
    let mut field = vec![0; width];
    let length = text.len().min(width.saturating_sub(1));
    field[..length].copy_from_slice(&text.as_bytes()[..length]);
    out.extend_from_slice(&field);
}

fn device_info(device: &Device, interfaces: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(320);
    fixed(
        &mut out,
        &format!("/virtual/win-airplay/{}", device.busid),
        256,
    );
    fixed(&mut out, &device.busid, 32);
    put_u32(&mut out, 1); // busnum
    put_u32(&mut out, device.devnum);
    put_u32(&mut out, 2); // full speed
    put_u16(&mut out, 0xcafe); // prototype VID; production requires an assigned VID/PID
    put_u16(&mut out, 0x4001);
    put_u16(&mut out, 0x0100);
    out.extend_from_slice(&[0, 0, 0, 1, 1, 2]);
    if interfaces {
        out.extend_from_slice(&[1, 1, 0, 0, 1, 2, 0, 0]);
    }
    out
}

fn response_header(code: u16, status: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    put_u16(&mut out, VERSION);
    put_u16(&mut out, code);
    put_u32(&mut out, status);
    out
}

fn serve_client(
    mut stream: TcpStream,
    devices: &Arc<RwLock<BTreeMap<String, Arc<Device>>>>,
    stop: &AtomicBool,
) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(500)))?;
    stream.set_nodelay(true)?;
    let mut preamble = [0; 8];
    read_exact_active(&mut stream, &mut preamble, stop, None)?;
    if u16::from_be_bytes([preamble[0], preamble[1]]) != VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unsupported USB/IP version",
        ));
    }
    match u16::from_be_bytes([preamble[2], preamble[3]]) {
        0x8005 => {
            let snapshot = devices
                .read()
                .unwrap()
                .values()
                .filter(|d| d.active.load(Ordering::Acquire))
                .cloned()
                .collect::<Vec<_>>();
            let mut out = response_header(5, 0);
            put_u32(&mut out, snapshot.len() as u32);
            for device in snapshot {
                out.extend_from_slice(&device_info(&device, true));
            }
            stream.write_all(&out)
        }
        0x8003 => {
            let mut field = [0; 32];
            read_exact_active(&mut stream, &mut field, stop, None)?;
            let end = field.iter().position(|&byte| byte == 0).unwrap_or(32);
            let busid =
                std::str::from_utf8(&field[..end]).map_err(|_| io::ErrorKind::InvalidData)?;
            let device = devices.read().unwrap().get(busid).cloned();
            let mut out = response_header(3, u32::from(device.is_none()));
            if let Some(device) = device {
                out.extend_from_slice(&device_info(&device, false));
                stream.write_all(&out)?;
                serve_imported(&mut stream, &device, stop)
            } else {
                stream.write_all(&out)
            }
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unknown USB/IP operation",
        )),
    }
}

fn read_exact_active(
    stream: &mut TcpStream,
    bytes: &mut [u8],
    stop: &AtomicBool,
    device: Option<&Device>,
) -> io::Result<()> {
    let mut offset = 0;
    while offset < bytes.len() {
        if stop.load(Ordering::Acquire) || device.is_some_and(|d| !d.active.load(Ordering::Acquire))
        {
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        match stream.read(&mut bytes[offset..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(count) => offset += count,
            Err(error)
                if error.kind() == io::ErrorKind::WouldBlock
                    || error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn serve_imported(stream: &mut TcpStream, device: &Device, stop: &AtomicBool) -> io::Result<()> {
    loop {
        let mut header = [0; 48];
        read_exact_active(stream, &mut header, stop, Some(device))?;
        let command = be32(&header[0..4]);
        let seq = be32(&header[4..8]);
        let direction = be32(&header[12..16]);
        let ep = be32(&header[16..20]);
        match command {
            1 => {
                let length = be32(&header[24..28]) as usize;
                let packets = be32(&header[32..36]);
                if length > MAX_TRANSFER
                    || (packets != u32::MAX && packets as usize > MAX_ISO_PACKETS)
                    || direction > 1
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "oversized USB/IP transfer",
                    ));
                }
                let mut payload = if direction == 0 {
                    vec![0; length]
                } else {
                    Vec::new()
                };
                if !payload.is_empty() {
                    read_exact_active(stream, &mut payload, stop, Some(device))?;
                }
                let mut iso = if packets == u32::MAX {
                    Vec::new()
                } else {
                    vec![0; packets as usize * 16]
                };
                if !iso.is_empty() {
                    read_exact_active(stream, &mut iso, stop, Some(device))?;
                }
                let setup: [u8; 8] = header[40..48].try_into().unwrap();
                let (status, data, actual, iso_reply) =
                    handle_submit(device, ep, direction, setup, length, &payload, &iso);
                let mut out = Vec::with_capacity(48 + data.len() + iso_reply.len());
                put_u32(&mut out, 3);
                put_u32(&mut out, seq);
                put_u32(&mut out, 0);
                put_u32(&mut out, 0);
                put_u32(&mut out, 0);
                put_u32(&mut out, status as u32);
                put_u32(&mut out, actual as u32);
                put_u32(&mut out, be32(&header[28..32]));
                put_u32(&mut out, packets);
                put_u32(&mut out, 0);
                out.extend_from_slice(&[0; 8]);
                out.extend_from_slice(&data);
                out.extend_from_slice(&iso_reply);
                stream.write_all(&out)?;
            }
            2 => {
                let mut out = Vec::with_capacity(48);
                put_u32(&mut out, 4);
                put_u32(&mut out, seq);
                out.extend_from_slice(&[0; 40]);
                stream.write_all(&out)?;
            }
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "unknown USB/IP URB",
                ))
            }
        }
    }
}

fn handle_submit(
    device: &Device,
    ep: u32,
    direction: u32,
    setup: [u8; 8],
    length: usize,
    payload: &[u8],
    iso: &[u8],
) -> (i32, Vec<u8>, usize, Vec<u8>) {
    if ep == 0 {
        return match control(device, setup, payload) {
            Ok(mut data) => {
                data.truncate(length);
                let actual = if direction == 0 {
                    payload.len()
                } else {
                    data.len()
                };
                (0, data, actual, Vec::new())
            }
            Err(status) => (status, Vec::new(), 0, Vec::new()),
        };
    }
    if ep != 1 || direction != 0 || iso.is_empty() || device.state.lock().unwrap().alternate != 1 {
        return (EPIPE, Vec::new(), 0, Vec::new());
    }
    let mut reply = iso.to_vec();
    let mut compact = Vec::with_capacity(payload.len());
    let mut actual = 0usize;
    let mut cursor = 0usize;
    for entry in reply.as_chunks_mut::<16>().0 {
        let packet_len = be32(&entry[4..8]) as usize;
        if cursor
            .checked_add(packet_len)
            .is_none_or(|end| end > payload.len())
        {
            return (EPIPE, Vec::new(), 0, Vec::new());
        }
        compact.extend_from_slice(&payload[cursor..cursor + packet_len]);
        cursor += packet_len;
        entry[8..12].copy_from_slice(&(packet_len as u32).to_be_bytes());
        entry[12..16].fill(0);
        actual += packet_len;
    }
    if !compact.is_empty() {
        device.pcm.push(compact);
    }
    (0, Vec::new(), actual, reply)
}

fn control(device: &Device, setup: [u8; 8], payload: &[u8]) -> Result<Vec<u8>, i32> {
    let request_type = setup[0];
    let request = setup[1];
    let value = u16::from_le_bytes([setup[2], setup[3]]);
    let index = u16::from_le_bytes([setup[4], setup[5]]);
    let length = u16::from_le_bytes([setup[6], setup[7]]) as usize;
    let mut state = device.state.lock().unwrap();
    match (request_type, request) {
        (0x80, 6) => {
            let descriptor = match (value >> 8) as u8 {
                1 => device_descriptor(),
                2 => configuration_descriptor(),
                3 => string_descriptor(device, value as u8)?,
                _ => return Err(EPIPE),
            };
            Ok(descriptor[..descriptor.len().min(length)].to_vec())
        }
        (0x00, 5) => Ok(Vec::new()), // SET_ADDRESS is virtualized by the host controller
        (0x00, 9) if value <= 1 => {
            state.configuration = value as u8;
            Ok(Vec::new())
        }
        (0x80, 8) => Ok(vec![state.configuration]),
        (0x01, 11) if index == 1 && value <= 1 => {
            state.alternate = value as u8;
            Ok(Vec::new())
        }
        (0x81, 10) if index == 1 => Ok(vec![state.alternate]),
        (0x80, 0) | (0x81, 0) | (0x82, 0) => Ok(vec![0, 0]), // GET_STATUS
        (0x21, 1) if index == 0x0200 => {
            let channel = value as u8;
            let selector = (value >> 8) as u8;
            match (selector, channel) {
                (1, 0) if !payload.is_empty() => state.volume.muted = payload[0] != 0,
                (2, 0..=2) if payload.len() >= 2 => {
                    let db = i16::from_le_bytes([payload[0], payload[1]]) as f32 / 256.0;
                    let scalar = (10.0_f32).powf(db / 20.0).clamp(0.0, 1.0);
                    if channel == 0 {
                        state.volume.scalar = scalar;
                    } else {
                        state.volume.channels[channel as usize - 1] = scalar;
                    }
                }
                _ => return Err(EPIPE),
            }
            let _ = device.volume_events.try_send(state.volume);
            Ok(Vec::new())
        }
        (0xa1, 0x81..=0x84) if index == 0x0200 => {
            let channel = value as u8;
            let selector = (value >> 8) as u8;
            match (selector, request, channel) {
                (1, 0x81, 0) => Ok(vec![u8::from(state.volume.muted)]),
                (2, 0x81, 0..=2) => {
                    let scalar = if channel == 0 {
                        state.volume.scalar
                    } else {
                        state.volume.channels[channel as usize - 1]
                    };
                    let db = if scalar <= 0.001 {
                        -96.0
                    } else {
                        20.0 * scalar.log10()
                    };
                    Ok(((db.clamp(-96.0, 0.0) * 256.0) as i16)
                        .to_le_bytes()
                        .to_vec())
                }
                (2, 0x82, 0..=2) => Ok((-96_i16 * 256).to_le_bytes().to_vec()),
                (2, 0x83, 0..=2) => Ok(0_i16.to_le_bytes().to_vec()),
                (2, 0x84, 0..=2) => Ok(256_i16.to_le_bytes().to_vec()),
                _ => Err(EPIPE),
            }
        }
        _ => Err(EPIPE),
    }
}

fn device_descriptor() -> Vec<u8> {
    vec![
        18, 1, 0x00, 0x02, 0, 0, 0, 64, 0xfe, 0xca, 0x01, 0x40, 0x00, 0x01, 1, 2, 3, 1,
    ]
}

fn configuration_descriptor() -> Vec<u8> {
    let mut out = vec![
        9, 2, 0, 0, 2, 1, 0, 0x80, 50, // configuration
        9, 4, 0, 0, 0, 1, 1, 0, 0, // AudioControl interface
        9, 0x24, 1, 0, 1, 40, 0, 1, 1, // AC header, streaming interface 1
        12, 0x24, 2, 1, 0x01, 0x01, 0, 2, 3, 0, 0, 0, // USB-streaming input terminal
        10, 0x24, 6, 2, 1, 1, 3, 2, 2, 0, // mute/volume Feature Unit
        9, 0x24, 3, 3, 0x01, 0x03, 0, 2, 0, // speaker output terminal
        9, 4, 1, 0, 0, 1, 2, 0, 0, // AudioStreaming alt 0
        9, 4, 1, 1, 1, 1, 2, 0, 0, // AudioStreaming alt 1
        7, 0x24, 1, 1, 1, 1, 0, // AS general, PCM
        11, 0x24, 2, 1, 2, 2, 16, 1, 0x44, 0xac, 0, // 44.1 kHz stereo 16-bit
        9, 5, 1, 0x0d, 180, 0, 1, 0, 0, // synchronous isochronous OUT
        7, 0x25, 1, 0, 0, 0, 0, // class-specific endpoint
    ];
    let length = out.len() as u16;
    out[2..4].copy_from_slice(&length.to_le_bytes());
    out
}

fn string_descriptor(device: &Device, index: u8) -> Result<Vec<u8>, i32> {
    if index == 0 {
        return Ok(vec![4, 3, 0x09, 0x04]);
    }
    let value = match index {
        1 => "Win AirPlay",
        2 => &device.name,
        3 => &device.serial,
        _ => return Err(EPIPE),
    };
    let utf16 = value.encode_utf16().take(126).collect::<Vec<_>>();
    let mut out = vec![(utf16.len() * 2 + 2) as u8, 3];
    for unit in utf16 {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_are_well_formed() {
        let config = configuration_descriptor();
        assert_eq!(
            u16::from_le_bytes([config[2], config[3]]) as usize,
            config.len()
        );
        assert_eq!(config[4], 2);
        let mut offset = 0;
        while offset < config.len() {
            assert!(config[offset] >= 2);
            offset += config[offset] as usize;
        }
        assert_eq!(offset, config.len());
    }

    #[test]
    fn device_list_and_import_are_protocol_sized() {
        let server = Server::start(0).unwrap();
        let streams = server.add("0123456789abcdef", "Kitchen");
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
        let streams = server.add("0123456789abcdef", "Kitchen");
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
        assert_eq!(length, configuration_descriptor().len());
        let mut config = vec![0; length];
        client.read_exact(&mut config).unwrap();
        assert_eq!(config, configuration_descriptor());
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
