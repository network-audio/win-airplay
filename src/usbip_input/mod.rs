//! USB Audio 2 microphone exported over a loopback USB/IP connection.
//! Speaker outputs use the shared usbip-audio crate; this server handles the
//! microphone's class notification endpoint, which needs pending interrupt URBs.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::{Duration, Instant};

mod input;
pub use input::CaptureInput;

const VERSION: u16 = 0x0111;
const MAX_TRANSFER: usize = 1024 * 1024;
const MAX_ISO_PACKETS: usize = 1024;
const EPIPE: i32 = -32;
static URB_ERRORS_LOGGED: AtomicU32 = AtomicU32::new(0);

struct Device {
    busid: String,
    devnum: u32,
    name: String,
    serial: String,
    active: AtomicBool,
    state: Mutex<DeviceState>,
    input: Option<Arc<CaptureInput>>,
}

#[derive(Default)]
struct DeviceState {
    configuration: u8,
    alternate: u8,
}

pub struct InputStreams {
    pub busid: String,
    pub input: Arc<CaptureInput>,
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
                            if worker_stop.load(Ordering::Acquire) {
                                break;
                            }
                            let devices = worker_devices.clone();
                            let stop = worker_stop.clone();
                            let _ = thread::Builder::new()
                                .name("airplay-usbip-client".into())
                                .spawn(move || {
                                    if let Err(error) = serve_client(stream, &devices, &stop) {
                                        if error.kind() != io::ErrorKind::UnexpectedEof
                                            && error.kind() != io::ErrorKind::ConnectionReset
                                            && error.kind() != io::ErrorKind::ConnectionAborted
                                        {
                                            eprintln!("USB/IP client ended: {error}");
                                        }
                                    }
                                });
                        }
                        Err(error) => {
                            eprintln!("USB/IP accept failed: {error}");
                            thread::sleep(Duration::from_millis(100));
                        }
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

    pub fn add_input(&self, key: &str, name: &str) -> InputStreams {
        let busid = format!("1-{}", &key[..key.len().min(16)]);
        let devnum = self.next_devnum.fetch_add(1, Ordering::Relaxed);
        let input = Arc::new(CaptureInput::default());
        let device = Arc::new(Device {
            busid: busid.clone(),
            devnum,
            name: name.to_owned(),
            serial: key.to_owned(),
            active: AtomicBool::new(true),
            state: Mutex::new(DeviceState::default()),
            input: Some(input.clone()),
        });
        if let Some(previous) = self.devices.write().unwrap().insert(busid.clone(), device) {
            previous.active.store(false, Ordering::Release);
        }
        InputStreams { busid, input }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake the blocking accept so shutdown can join the listener promptly.
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
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
    put_u32(&mut out, 1);
    put_u32(&mut out, device.devnum);
    put_u32(&mut out, 3); // high speed
    put_u16(&mut out, 0xcafe);
    put_u16(&mut out, 0x4002);
    put_u16(&mut out, 0x0100);
    out.extend_from_slice(&[0xef, 2, 1, 1, 1, 2]);
    if interfaces {
        out.extend_from_slice(&[1, 1, 0x20, 0, 1, 2, 0x20, 0]);
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
    // Keep client reads blocking even if the listener mode changes later.
    stream.set_nonblocking(false)?;
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
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                // Keep the loop bounded even if a socket unexpectedly becomes
                // nonblocking after it was accepted.
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn next_iso_deadline(previous: Instant, now: Instant, packets: u32) -> Instant {
    // Preserve the USB audio clock across URBs. Restarting each 10 ms period
    // from `now` also adds processing time to every period, starving AirPlay.
    // Limit catch-up to 20 ms after an unusually long scheduling pause.
    let oldest = now.checked_sub(Duration::from_millis(20)).unwrap_or(now);
    previous.max(oldest) + Duration::from_millis(u64::from(packets.min(100)))
}

struct ImportedSession(Arc<AtomicBool>);
impl Drop for ImportedSession {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn serve_imported(stream: &mut TcpStream, device: &Device, stop: &AtomicBool) -> io::Result<()> {
    let mut next_audio_due = std::time::Instant::now();
    let session = ImportedSession(Arc::new(AtomicBool::new(true)));
    let writer = Arc::new(Mutex::new(stream.try_clone()?));
    let pending: Arc<Mutex<BTreeMap<u32, Arc<AtomicBool>>>> = Arc::new(Mutex::new(BTreeMap::new()));
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
                if let Some(input) = device
                    .input
                    .as_ref()
                    .filter(|_| ep == 2 && direction == 1 && iso.is_empty())
                {
                    eprintln!("USB input notification request queued: seq={seq} length={length}");
                    let input = input.clone();
                    let writer = writer.clone();
                    let pending = pending.clone();
                    let alive = session.0.clone();
                    let cancelled = Arc::new(AtomicBool::new(false));
                    pending.lock().unwrap().insert(seq, cancelled.clone());
                    thread::Builder::new()
                        .name("airplay-usbip-notification".into())
                        .spawn(move || {
                            while alive.load(Ordering::Acquire)
                                && !cancelled.load(Ordering::Acquire)
                            {
                                // usbip-win2 can submit multiple interrupt reads. Complete
                                // the oldest one first; a later completion can remain behind
                                // an earlier pending URB in the Windows host stack.
                                let first = pending.lock().unwrap().keys().next().copied();
                                if first == Some(seq) && input.take_change() {
                                    eprintln!("USB input connector change notification: seq={seq}");
                                    let event = [0, 1, 0, 2, 0, 1];
                                    let size = length.min(event.len());
                                    let mut out = Vec::with_capacity(48 + size);
                                    for value in [3, seq, 0, 0, 0, 0, size as u32, 0, 0, 0] {
                                        put_u32(&mut out, value);
                                    }
                                    out.extend_from_slice(&[0; 8]);
                                    out.extend_from_slice(&event[..size]);
                                    if !cancelled.load(Ordering::Acquire) {
                                        let _ = writer.lock().unwrap().write_all(&out);
                                    }
                                    break;
                                }
                                input.wait_for_change();
                            }
                            pending.lock().unwrap().remove(&seq);
                        })?;
                    continue;
                }
                let setup: [u8; 8] = header[40..48].try_into().unwrap();
                let (status, data, actual, iso_reply) =
                    handle_submit(device, ep, direction, setup, length, &payload, &iso);
                if status != 0 && URB_ERRORS_LOGGED.fetch_add(1, Ordering::Relaxed) < 20 {
                    let alternate = device.state.lock().unwrap().alternate;
                    let requested_iso_bytes: usize = iso
                        .as_chunks::<16>()
                        .0
                        .iter()
                        .map(|packet| be32(&packet[4..8]) as usize)
                        .sum();
                    eprintln!("USB/IP URB failed: ep={ep} direction={direction} setup={setup:02x?} length={length} packets={packets} alternate={alternate} requested_iso_bytes={requested_iso_bytes} status={status}");
                }
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
                if ep == 1 && packets > 0 && packets != u32::MAX {
                    let now = std::time::Instant::now();
                    next_audio_due = next_iso_deadline(next_audio_due, now, packets);
                    thread::sleep(next_audio_due.saturating_duration_since(now));
                }
                writer.lock().unwrap().write_all(&out)?;
            }
            2 => {
                let target = be32(&header[20..24]);
                if let Some(cancelled) = pending.lock().unwrap().remove(&target) {
                    cancelled.store(true, Ordering::Release);
                }
                let mut out = Vec::with_capacity(48);
                put_u32(&mut out, 4);
                put_u32(&mut out, seq);
                out.extend_from_slice(&[0; 40]);
                writer.lock().unwrap().write_all(&out)?;
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
    input::handle_submit(
        device.input.as_ref().unwrap(),
        &device.state,
        ep,
        direction,
        length,
        iso,
    )
}

fn control(device: &Device, setup: [u8; 8], payload: &[u8]) -> Result<Vec<u8>, i32> {
    input::control(
        device.input.as_ref().unwrap(),
        &device.state,
        device,
        setup,
        payload,
    )
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
