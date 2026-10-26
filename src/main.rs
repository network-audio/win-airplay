use clap::{Parser, Subcommand};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
#[cfg(any(windows, test))]
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::time::{Duration, Instant};

const SERVICE_TYPES: [&str; 2] = ["_airplay._tcp.local.", "_raop._tcp.local."];

#[cfg(windows)]
mod airplay;
#[cfg(windows)]
mod windows_audio;
#[cfg(windows)]
mod windows_device;

#[derive(Parser)]
#[command(version, about = "Discover AirPlay speakers and send Windows audio")]
struct Cli {
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Discover local AirPlay and RAOP speakers.
    Discover {
        #[arg(long, default_value_t = 4)]
        seconds: u64,
        #[arg(long)]
        json: bool,
    },
    /// Stream audio using the integrated AirPlay sender.
    Stream {
        /// Speaker name, IP address, or IP:port.
        #[arg(long)]
        target: String,
        /// Capture from this Windows render endpoint by WASAPI loopback.
        #[arg(long)]
        endpoint: Option<String>,
        /// Send a test tone instead of system audio.
        #[arg(long)]
        test_tone: bool,
        /// Length of test tone in seconds.
        #[arg(long, default_value_t = 3)]
        duration: u32,
    },
    /// Keep one Windows output endpoint for each discovered AirPlay device.
    Run,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct Speaker {
    name: String,
    address: IpAddr,
    port: u16,
    service: String,
    model: Option<String>,
    device_id: Option<String>,
}

fn instance_name(fullname: &str, service: &str) -> String {
    fullname
        .strip_suffix(service)
        .unwrap_or(fullname)
        .trim_end_matches('.')
        .to_string()
}

fn discover(seconds: u64) -> Result<Vec<Speaker>, String> {
    let mdns = ServiceDaemon::new().map_err(|e| e.to_string())?;
    let browsers = SERVICE_TYPES
        .iter()
        .map(|service| mdns.browse(service).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let until = Instant::now() + Duration::from_secs(seconds);
    let mut found = BTreeMap::new();

    while Instant::now() < until {
        for (service, browser) in SERVICE_TYPES.iter().zip(&browsers) {
            if let Ok(ServiceEvent::ServiceResolved(info)) =
                browser.recv_timeout(Duration::from_millis(100))
            {
                let name = instance_name(info.get_fullname(), service);
                let model = info.get_property_val_str("model").map(str::to_owned);
                let device_id = info.get_property_val_str("deviceid").map(str::to_owned);
                for &address in info.get_addresses() {
                    if !address.is_ipv4() {
                        continue;
                    }
                    let speaker = Speaker {
                        name: name.clone(),
                        address,
                        port: info.get_port(),
                        service: service.to_string(),
                        model: model.clone(),
                        device_id: device_id.clone(),
                    };
                    found.insert((name.clone(), address, *service), speaker);
                }
            }
        }
    }
    let _ = mdns.shutdown();
    let speakers: Vec<_> = found.into_values().collect();
    let airplay_addresses: std::collections::BTreeSet<_> = speakers
        .iter()
        .filter(|speaker| speaker.service == SERVICE_TYPES[0])
        .map(|speaker| speaker.address)
        .collect();
    Ok(speakers
        .into_iter()
        .filter(|speaker| {
            speaker.service == SERVICE_TYPES[0] || !airplay_addresses.contains(&speaker.address)
        })
        .collect())
}

#[cfg(any(windows, test))]
fn speaker_key(speaker: &Speaker) -> String {
    let identity = speaker
        .device_id
        .clone()
        .unwrap_or_else(|| format!("{}@{}", speaker.name, speaker.address));
    let digest = Sha256::digest(identity.to_ascii_lowercase().as_bytes());
    digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(windows)]
fn endpoint_name(speaker: &Speaker) -> String {
    let name: String = speaker.name.chars().take(160).collect();
    let key = speaker_key(speaker);
    format!("AirPlay {name} [{}]", &key[..8])
}

fn resolve_target(target: &str) -> Result<SocketAddr, String> {
    if let Ok(address) = target.parse::<SocketAddr>() {
        return Ok(address);
    }
    if let Ok(address) = target.parse::<IpAddr>() {
        return Ok(SocketAddr::new(address, 7000));
    }
    discover(4)?
        .into_iter()
        .find(|speaker| speaker.name.eq_ignore_ascii_case(target))
        .map(|speaker| SocketAddr::new(speaker.address, speaker.port))
        .ok_or_else(|| format!("AirPlay speaker {target:?} was not found"))
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        Action::Discover { seconds, json } => {
            let speakers = discover(seconds)?;
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&speakers).map_err(|e| e.to_string())?
                );
            } else if speakers.is_empty() {
                println!("No AirPlay speakers found on the local network.");
            } else {
                for speaker in speakers {
                    println!(
                        "{}\t{}:{}\t{}",
                        speaker.name, speaker.address, speaker.port, speaker.service
                    );
                }
            }
        }
        Action::Stream {
            target,
            endpoint,
            test_tone,
            duration,
        } => {
            let target = resolve_target(&target)?;
            if test_tone {
                #[cfg(windows)]
                {
                    airplay::test_tone(target, Duration::from_secs(duration.into()))?;
                    return Ok(());
                }
                #[cfg(not(windows))]
                {
                    let _ = duration;
                    return Err("integrated AirPlay transport currently builds for Windows".into());
                }
            }
            run_manual_stream(target, endpoint)?;
        }
        Action::Run => run_manager()?,
    }
    Ok(())
}

#[cfg(windows)]
fn run_manual_stream(target: SocketAddr, endpoint: Option<String>) -> Result<(), String> {
    let endpoint = endpoint.unwrap_or_else(|| "AirPlay".into());
    let stream = airplay::StreamHandle::start(
        target,
        endpoint,
        airplay::Volume {
            scalar: 0.5,
            muted: false,
        },
        None,
    );
    while !stream.is_finished() {
        std::thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

#[cfg(not(windows))]
fn run_manual_stream(_target: SocketAddr, _endpoint: Option<String>) -> Result<(), String> {
    Err("WASAPI capture requires Windows".into())
}

#[cfg(windows)]
fn run_manager() -> Result<(), String> {
    use std::sync::mpsc;
    use windows_device::SoftwareDevice;

    struct Managed {
        _device: SoftwareDevice,
        endpoint: String,
        target: SocketAddr,
        stream: Option<airplay::StreamHandle>,
        remote_volume: Option<mpsc::Receiver<f32>>,
        last_volume: Option<airplay::Volume>,
        last_spawn: Instant,
        missed_scans: u8,
    }

    windows_audio::initialize()?;
    let (discovery_tx, discovery_rx) = mpsc::channel();
    std::thread::spawn(move || loop {
        match discover(4) {
            Ok(speakers) => {
                let reachable: Vec<_> = speakers
                    .into_iter()
                    .filter(|speaker| {
                        let address = SocketAddr::new(speaker.address, speaker.port);
                        std::net::TcpStream::connect_timeout(&address, Duration::from_millis(400))
                            .is_ok()
                    })
                    .collect();
                if discovery_tx.send(reachable).is_err() {
                    break;
                }
            }
            Err(error) => eprintln!("AirPlay discovery failed: {error}"),
        }
        std::thread::sleep(Duration::from_secs(1));
    });
    let mut active: BTreeMap<String, Managed> = BTreeMap::new();
    loop {
        while let Ok(speakers) = discovery_rx.try_recv() {
            for managed in active.values_mut() {
                managed.missed_scans = managed.missed_scans.saturating_add(1);
            }
            for speaker in speakers
                .into_iter()
                .filter(|speaker| speaker.service == SERVICE_TYPES[0])
            {
                let key = speaker_key(&speaker);
                let endpoint = endpoint_name(&speaker);
                let target = SocketAddr::new(speaker.address, speaker.port);
                if let Some(managed) = active.get_mut(&key) {
                    managed.missed_scans = 0;
                    if managed.target != target {
                        if let Some(stream) = managed.stream.take() {
                            stream.stop();
                        }
                        managed.remote_volume = None;
                    }
                    managed.target = target;
                    continue;
                }
                match SoftwareDevice::create(&key, &endpoint) {
                    Ok(device) => {
                        println!("Created output {endpoint} for {target}");
                        active.insert(
                            key,
                            Managed {
                                _device: device,
                                endpoint,
                                target,
                                stream: None,
                                remote_volume: None,
                                last_volume: None,
                                last_spawn: Instant::now() - Duration::from_secs(10),
                                missed_scans: 0,
                            },
                        );
                    }
                    Err(error) => {
                        eprintln!("Could not create output for {}: {error}", speaker.name)
                    }
                }
            }
            active.retain(|_, managed| {
                if managed.missed_scans < 3 {
                    true
                } else {
                    if let Some(stream) = managed.stream.take() {
                        stream.stop();
                    }
                    managed.remote_volume = None;
                    println!("Removed output {}", managed.endpoint);
                    false
                }
            });
        }

        let default_output = windows_audio::default_render_output().ok();
        for managed in active.values_mut() {
            let selected = default_output
                .as_ref()
                .is_some_and(|output| output.name.contains(&managed.endpoint));
            if !selected {
                if let Some(stream) = managed.stream.take() {
                    stream.stop();
                }
                managed.remote_volume = None;
                managed.last_volume = None;
                continue;
            }
            if managed
                .stream
                .as_ref()
                .is_some_and(|stream| stream.is_finished())
            {
                managed.stream = None;
                managed.remote_volume = None;
                managed.last_volume = None;
            }
            if managed.missed_scans < 3
                && managed.stream.is_none()
                && managed.last_spawn.elapsed() >= Duration::from_secs(5)
            {
                let volume = default_output.as_ref().expect("selected output").volume;
                let (remote_volume_tx, remote_volume_rx) = mpsc::channel();
                managed.stream = Some(airplay::StreamHandle::start(
                    managed.target,
                    managed.endpoint.clone(),
                    volume,
                    Some(remote_volume_tx),
                ));
                managed.remote_volume = Some(remote_volume_rx);
                managed.last_volume = Some(volume);
                managed.last_spawn = Instant::now();
            } else if let (Some(stream), Some(output)) = (&managed.stream, &default_output) {
                let mut remote_applied = false;
                if let Some(remote) = &managed.remote_volume {
                    while let Ok(scalar) = remote.try_recv() {
                        if let Err(error) =
                            windows_audio::set_default_volume_if_name(&managed.endpoint, scalar)
                        {
                            eprintln!("Could not mirror speaker volume to Windows: {error}");
                        } else {
                            managed.last_volume = Some(airplay::Volume {
                                scalar,
                                muted: scalar <= 0.0,
                            });
                            remote_applied = true;
                        }
                    }
                }
                if remote_applied {
                    continue;
                }
                let changed = managed.last_volume.is_none_or(|last| {
                    last.muted != output.volume.muted
                        || (last.scalar - output.volume.scalar).abs() >= 0.005
                });
                if changed {
                    stream.set_volume(output.volume);
                    managed.last_volume = Some(output.volume);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(not(windows))]
fn run_manager() -> Result<(), String> {
    Err("dynamic audio endpoints require Windows".into())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_target_uses_airplay_port() {
        assert_eq!(
            resolve_target("192.0.2.1").unwrap().to_string(),
            "192.0.2.1:7000"
        );
    }

    #[test]
    fn removes_dns_service_suffix() {
        assert_eq!(
            instance_name("Kitchen._airplay._tcp.local.", SERVICE_TYPES[0]),
            "Kitchen"
        );
    }

    #[test]
    fn stable_speaker_key_uses_device_id() {
        let mut speaker = Speaker {
            name: "Kitchen".into(),
            address: "192.0.2.1".parse().unwrap(),
            port: 7000,
            service: SERVICE_TYPES[0].into(),
            model: None,
            device_id: Some("AA:BB:CC:DD:EE:FF".into()),
        };
        let key = speaker_key(&speaker);
        speaker.address = "192.0.2.2".parse().unwrap();
        speaker.device_id = Some("aa:bb:cc:dd:ee:ff".into());
        assert_eq!(speaker_key(&speaker), key);
        assert_eq!(key.len(), 32);
    }
}
