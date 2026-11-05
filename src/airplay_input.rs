//! AirPlay 2 receiver feeding the virtual USB capture endpoint.

use crate::usbip_audio::CaptureInput;
use mdns_sd::{ServiceDaemon, ServiceInfo};
use openairplay2::{AudioSink, Event, Identity, Receiver};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

const NAME: &str = "Win AirPlay Input";
const PORT: u16 = 7000;
const SERVICE: &str = "_airplay._tcp.local.";

struct UsbInputSink {
    input: Arc<CaptureInput>,
    rate: u32,
    channels: u8,
    next_due: Instant,
}

impl AudioSink for UsbInputSink {
    fn write(&mut self, samples: &[i16]) {
        if self.rate != 44_100 || self.channels != 2 {
            return;
        }
        self.input.write_pcm(samples);
        let frames = samples.len() / usize::from(self.channels);
        let duration = Duration::from_secs_f64(frames as f64 / f64::from(self.rate));
        self.next_due = self.next_due.max(Instant::now()) + duration;
        std::thread::sleep(self.next_due.saturating_duration_since(Instant::now()));
    }

    fn flush(&mut self) {
        self.input.clear();
        self.next_due = Instant::now();
    }
}

pub fn start(input: Arc<CaptureInput>) -> Result<String, String> {
    let home = std::env::var_os("LOCALAPPDATA")
        .ok_or("LOCALAPPDATA is unavailable for the AirPlay receiver identity")?;
    let folder = std::path::PathBuf::from(home).join("WinAirPlay");
    std::fs::create_dir_all(&folder).map_err(|error| error.to_string())?;
    let identity = Identity::load_or_create(&folder.join("receiver-identity"))
        .map_err(|error| format!("AirPlay receiver identity: {error}"))?;
    let key = identity.public_key();
    let mac = [key[0] & 0xfe | 0x02, key[1], key[2], key[3], key[4], key[5]];
    let receiver = Receiver::builder()
        .name(NAME)
        .port(PORT)
        .identity(identity)
        .mac(mac)
        .advertise(false)
        .build()
        .map_err(|error| error.to_string())?;
    let device_id = receiver.config().device_id();
    let txt = receiver.txt_records();
    let hostname = format!("win-airplay-{:02x}{:02x}.local.", mac[4], mac[5]);
    std::thread::Builder::new()
        .name("airplay-input-mdns".into())
        .spawn(move || advertise(NAME, &hostname, PORT, txt))
        .map_err(|error| error.to_string())?;
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
                tokio::spawn(async move {
                    while let Some(event) = received.recv().await {
                        match event {
                            Event::SessionStarted { rate, channels, peer } => {
                                println!("AirPlay input stream from {peer}: {rate} Hz, {channels} channels");
                                event_input.set_connected(true);
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
                if let Err(error) = receiver
                    .run(
                        move |rate, channels| {
                            Box::new(UsbInputSink {
                                input: sink_input.clone(),
                                rate,
                                channels,
                                next_due: Instant::now(),
                            })
                        },
                        events,
                    )
                    .await
                {
                    eprintln!("AirPlay input receiver stopped: {error}");
                    input.set_connected(false);
                }
            });
        })
        .map_err(|error| error.to_string())?;
    Ok(device_id)
}

fn advertise(name: &str, hostname: &str, port: u16, records: Vec<String>) {
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
    let fullname = format!("{name}.{SERVICE}");
    loop {
        let mut current: Vec<IpAddr> = super::network_addresses()
            .unwrap_or_default()
            .into_iter()
            .map(|(_, address)| address)
            .collect();
        current.sort();
        current.dedup();
        if current != previous {
            if !previous.is_empty() {
                let _ = mdns.unregister(&fullname);
            }
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
                        }
                    }
                    Err(error) => eprintln!("AirPlay input mDNS record: {error}"),
                }
            }
            previous = current;
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}
