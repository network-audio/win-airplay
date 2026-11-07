//! AirPlay 2 receiver feeding the virtual USB capture endpoint.

use crate::usbip_audio::CaptureInput;
use log::{Level, LevelFilter, Log, Metadata, Record};
use mdns_sd::{ServiceDaemon, ServiceInfo};
use openairplay2::{AudioSink, Event, Identity, Receiver};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PORT: u16 = 7000;
const SERVICE: &str = "_airplay._tcp.local.";
const CAPTURE_HEADROOM: Duration = Duration::from_millis(80);
const MDNS_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(20);

struct ReceiverLogger;
static RECEIVER_LOGGER: ReceiverLogger = ReceiverLogger;

impl Log for ReceiverLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.target().starts_with("openairplay2::")
            && (metadata.level() <= Level::Warn
                || (metadata.level() <= Level::Debug
                    && (metadata.target() == "openairplay2::server"
                        || metadata.target() == "openairplay2::pairing"
                        || metadata.target() == "openairplay2::session")))
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let message = record.args().to_string();
        // The receiver's request logger also emits pairing headers and bodies.
        // Keep only the protocol steps and errors in the user's rotating log.
        if message.contains("]   ") {
            return;
        }
        if record.level() == Level::Debug
            && record.target() == "openairplay2::session"
            && !message.starts_with("SETUP phase 2: type=")
            && !message.starts_with("buffered audio connected")
            && !message.starts_with("buffered audio disconnected")
        {
            return;
        }
        eprintln!("AirPlay input {}: {message}", record.level());
    }

    fn flush(&self) {}
}

fn receiver_name() -> String {
    let computer = std::env::var("COMPUTERNAME")
        .ok()
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            std::env::var("HOSTNAME")
                .ok()
                .filter(|name| !name.trim().is_empty())
        })
        .unwrap_or_else(|| "Win AirPlay".into());
    format!("{} Input", computer.trim())
}

struct UsbInputSink {
    input: Arc<CaptureInput>,
    rate: u32,
    channels: u8,
    next_due: Instant,
    gain: Arc<AtomicU32>,
}

impl AudioSink for UsbInputSink {
    fn write(&mut self, samples: &[i16]) {
        if self.rate != 44_100 || self.channels != 2 {
            return;
        }
        let gain = f32::from_bits(self.gain.load(Ordering::Relaxed));
        if gain >= 0.999 {
            self.input.write_pcm(samples);
        } else {
            let scaled: Vec<i16> = samples
                .iter()
                .map(|sample| (f32::from(*sample) * gain) as i16)
                .collect();
            self.input.write_pcm(&scaled);
        }
        let frames = samples.len() / usize::from(self.channels);
        let duration = Duration::from_secs_f64(frames as f64 / f64::from(self.rate));
        // Build a small PCM cushion before pacing the sender. USB capture
        // requests and AirPlay decoder writes run on independent clocks.
        let earliest = Instant::now().checked_sub(CAPTURE_HEADROOM).unwrap();
        self.next_due = self.next_due.max(earliest) + duration;
        std::thread::sleep(self.next_due.saturating_duration_since(Instant::now()));
    }

    fn flush(&mut self) {
        self.input.clear();
        let now = Instant::now();
        self.next_due = now.checked_sub(CAPTURE_HEADROOM).unwrap_or(now);
    }
}

pub fn start(input: Arc<CaptureInput>) -> Result<String, String> {
    if log::set_logger(&RECEIVER_LOGGER).is_ok() {
        log::set_max_level(LevelFilter::Debug);
    }
    let name = receiver_name();
    let home = std::env::var_os("LOCALAPPDATA")
        .ok_or("LOCALAPPDATA is unavailable for the AirPlay receiver identity")?;
    let folder = std::path::PathBuf::from(home).join("WinAirPlay");
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let identity = Identity::load_or_create(&folder.join("receiver-identity"))
        .map_err(|error| format!("AirPlay receiver identity: {error}"))?;
    let key = identity.public_key();
    let mac = [key[0] & 0xfe | 0x02, key[1], key[2], key[3], key[4], key[5]];
    let receiver = Receiver::builder()
        .name(name.clone())
        .port(PORT)
        .identity(identity)
        .mac(mac)
        .advertise(false)
        .build()
        .map_err(|error| error.to_string())?;
    let device_id = receiver.config().device_id();
    let txt = receiver.txt_records();
    let hostname = format!("win-airplay-{:02x}{:02x}.local.", mac[4], mac[5]);
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let active = Arc::new(AtomicBool::new(true));
    let receiver_active = active.clone();
    std::thread::Builder::new()
        .name("airplay-input-receiver".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("Could not start AirPlay input runtime: {error}");
                    return;
                }
            };
            runtime.block_on(async move {
                let (events, mut received) = tokio::sync::mpsc::unbounded_channel();
                let event_input = input.clone();
                let gain = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
                let event_gain = gain.clone();
                tokio::spawn(async move {
                    while let Some(event) = received.recv().await {
                        match event {
                            Event::SessionStarted { rate, channels, peer } => {
                                event_gain.store(1.0_f32.to_bits(), Ordering::Relaxed);
                                println!("AirPlay input stream from {peer}: {rate} Hz, {channels} channels");
                                event_input.set_connected(true);
                            }
                            Event::Volume { db } => {
                                let linear = if db <= -100.0 { 0.0 } else { 10.0_f32.powf(db / 20.0) };
                                event_gain.store(linear.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                            }
                            Event::SessionEnded => {
                                event_input.set_connected(false);
                                println!("AirPlay input stream ended");
                            }
                            Event::Flushed => event_input.clear(),
                            _ => {}
                        }
                    }
                    event_input.set_connected(false);
                });
                let sink_input = input.clone();
                let sink_gain = gain.clone();
                if let Err(error) = receiver
                    .run_with_ready(
                        move |rate, channels| {
                            Box::new(UsbInputSink {
                                input: sink_input.clone(),
                                rate,
                                channels,
                                next_due: {
                                    let now = Instant::now();
                                    now.checked_sub(CAPTURE_HEADROOM).unwrap_or(now)
                                },
                                gain: sink_gain.clone(),
                            })
                        },
                        events,
                        move || { let _ = ready_tx.send(()); },
                    )
                    .await
                {
                    eprintln!("AirPlay input receiver stopped: {error}");
                    input.set_connected(false);
                }
                receiver_active.store(false, Ordering::Release);
            });
        })
        .map_err(|error| error.to_string())?;
    ready_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|error| format!("AirPlay input listener did not start on port {PORT}: {error}"))?;
    println!("AirPlay input advertising {name} on TCP port {PORT}");
    std::thread::Builder::new()
        .name("airplay-input-mdns".into())
        .spawn(move || advertise(&name, &hostname, PORT, txt, active))
        .map_err(|error| error.to_string())?;
    Ok(device_id)
}

fn advertise(name: &str, hostname: &str, port: u16, records: Vec<String>, active: Arc<AtomicBool>) {
    let mdns = match ServiceDaemon::new() {
        Ok(mdns) => mdns,
        Err(error) => {
            eprintln!("AirPlay input mDNS unavailable: {error}");
            return;
        }
    };
    let properties: HashMap<String, String> = records
        .iter()
        .filter_map(|record| record.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    let mut previous = Vec::<IpAddr>::new();
    let mut last_announcement: Option<Instant> = None;
    let fullname = format!("{name}.{SERVICE}");
    while active.load(Ordering::Acquire) {
        let mut current: Vec<IpAddr> = super::network_addresses()
            .unwrap_or_default()
            .into_iter()
            .map(|(_, address)| address)
            .collect();
        current.sort();
        current.dedup();
        let addresses_changed = current != previous;
        if addresses_changed
            || last_announcement.is_none_or(|at| at.elapsed() >= MDNS_ANNOUNCE_INTERVAL)
        {
            if !current.is_empty() {
                match ServiceInfo::new(
                    SERVICE,
                    name,
                    hostname,
                    current.as_slice(),
                    port,
                    properties.clone(),
                ) {
                    Ok(info) => {
                        if let Err(error) = mdns.register(info) {
                            eprintln!("Could not advertise AirPlay input: {error}");
                        } else {
                            if addresses_changed {
                                println!("AirPlay input mDNS addresses: {current:?}");
                            }
                            previous = current;
                            last_announcement = Some(Instant::now());
                        }
                    }
                    Err(error) => eprintln!("AirPlay input mDNS record: {error}"),
                }
            } else if !previous.is_empty() {
                if let Err(error) = mdns.unregister(&fullname) {
                    eprintln!("Could not withdraw AirPlay input: {error}");
                } else {
                    previous.clear();
                    last_announcement = None;
                }
            }
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    if !previous.is_empty() {
        let _ = mdns.unregister(&fullname);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};

    struct NullSink;

    impl AudioSink for NullSink {
        fn write(&mut self, _: &[i16]) {}
        fn flush(&mut self) {}
    }

    #[test]
    fn receiver_accepts_ipv4_on_windows() {
        let temporary = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = temporary.local_addr().unwrap().port();
        drop(temporary);
        let receiver = Receiver::builder()
            .name("AirPlay IPv4 test")
            .port(port)
            .identity(Identity::generate())
            .mac([2, 0, 0, 0, 0, 1])
            .advertise(false)
            .build()
            .unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let (events, _received) = tokio::sync::mpsc::unbounded_channel();
        let server = runtime.spawn(receiver.run(|_, _| Box::new(NullSink), events));
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        for _ in 0..40 {
            if TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_ok() {
                server.abort();
                return;
            }
            if server.is_finished() {
                panic!("AirPlay receiver stopped before accepting IPv4");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        server.abort();
        panic!("AirPlay receiver did not accept an IPv4 connection");
    }
}
