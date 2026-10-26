use clap::{Parser, Subcommand};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
#[cfg(any(windows, test))]
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

const SERVICE_TYPES: [&str; 2] = ["_airplay._tcp.local.", "_raop._tcp.local."];

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
    /// Stream system audio using the separately installed MIT-licensed nyxr sender.
    Stream {
        /// Speaker name, IP address, or IP:port.
        #[arg(long)]
        target: String,
        /// Capture from this Windows render endpoint by WASAPI loopback.
        #[arg(long)]
        endpoint: Option<String>,
        /// Path to the nyxr executable, or resolve it from PATH.
        #[arg(long, default_value = "nyxr")]
        sender: String,
        /// Send a test tone instead of system audio.
        #[arg(long)]
        test_tone: bool,
        /// Length of test tone in seconds.
        #[arg(long, default_value_t = 3)]
        duration: u32,
    },
    /// Keep one Windows output endpoint and sender for each discovered AirPlay device.
    Run {
        #[arg(long, default_value = "nyxr")]
        sender: String,
    },
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

fn sender_args(
    target: &str,
    endpoint: Option<&str>,
    test_tone: bool,
    duration: u32,
) -> Vec<String> {
    let mut args = vec![
        "stream".into(),
        "--target".into(),
        target.into(),
        "--audible".into(),
    ];
    if test_tone {
        args.extend([
            "--source".into(),
            "sine".into(),
            "--duration".into(),
            duration.to_string(),
        ]);
    } else {
        args.extend(["--source".into(), "wasapi".into()]);
        if let Some(endpoint) = endpoint {
            args.extend(["--device".into(), endpoint.into()]);
        }
    }
    args
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
            sender,
            test_tone,
            duration,
        } => {
            if !test_tone && !cfg!(windows) {
                return Err(
                    "WASAPI capture requires Windows; use --test-tone to test the sender elsewhere"
                        .into(),
                );
            }
            let args = sender_args(&target, endpoint.as_deref(), test_tone, duration);
            let status = Command::new(&sender)
                .args(&args)
                .status()
                .map_err(|e| format!("could not launch {sender}: {e}"))?;
            if !status.success() {
                return Err(format!("{sender} exited with {status}"));
            }
        }
        Action::Run { sender } => run_manager(sender)?,
    }
    Ok(())
}

#[cfg(windows)]
fn run_manager(sender: String) -> Result<(), String> {
    use std::process::Child;
    use windows_device::SoftwareDevice;

    struct Managed {
        _device: SoftwareDevice,
        endpoint: String,
        target: String,
        child: Option<Child>,
        last_spawn: Instant,
        missed_scans: u8,
    }

    windows_audio::initialize()?;
    let mut active: BTreeMap<String, Managed> = BTreeMap::new();
    loop {
        let speakers = discover(4)?;
        for speaker in speakers
            .into_iter()
            .filter(|speaker| speaker.service == SERVICE_TYPES[0])
        {
            let key = speaker_key(&speaker);
            let endpoint = endpoint_name(&speaker);
            let target = speaker.address.to_string();
            if let Some(managed) = active.get_mut(&key) {
                managed.missed_scans = 0;
                if managed.target != target {
                    if let Some(mut child) = managed.child.take() {
                        let _ = child.kill();
                        let _ = child.wait();
                    }
                }
                managed.target = target;
                continue;
            }
            match SoftwareDevice::create(&key, &endpoint) {
                Ok(device) => {
                    println!("Created output {endpoint} for {}", speaker.address);
                    active.insert(
                        key,
                        Managed {
                            _device: device,
                            endpoint,
                            target,
                            child: None,
                            last_spawn: Instant::now() - Duration::from_secs(10),
                            missed_scans: 0,
                        },
                    );
                }
                Err(error) => eprintln!("Could not create output for {}: {error}", speaker.name),
            }
        }

        let default_output = windows_audio::default_render_name().unwrap_or_default();
        for managed in active.values_mut() {
            managed.missed_scans = managed.missed_scans.saturating_add(1);
            let selected = default_output.contains(&managed.endpoint);
            if !selected {
                if let Some(mut child) = managed.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                continue;
            }
            if let Some(child) = managed.child.as_mut() {
                if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                    managed.child = None;
                }
            }
            if managed.missed_scans < 3
                && managed.child.is_none()
                && managed.last_spawn.elapsed() >= Duration::from_secs(5)
            {
                let mut args = sender_args(&managed.target, Some(&managed.endpoint), false, 0);
                args.push("--stats".into());
                match Command::new(&sender).args(args).spawn() {
                    Ok(child) => {
                        managed.child = Some(child);
                        managed.last_spawn = Instant::now();
                    }
                    Err(error) => eprintln!("Could not start {sender}: {error}"),
                }
            }
        }
        active.retain(|_, managed| {
            if managed.missed_scans < 3 {
                true
            } else {
                if let Some(mut child) = managed.child.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                println!("Removed output {}", managed.endpoint);
                false
            }
        });
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[cfg(not(windows))]
fn run_manager(_sender: String) -> Result<(), String> {
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
    fn sender_uses_selected_endpoint() {
        assert_eq!(
            sender_args("Kitchen", Some("Win AirPlay"), false, 3),
            [
                "stream",
                "--target",
                "Kitchen",
                "--audible",
                "--source",
                "wasapi",
                "--device",
                "Win AirPlay"
            ]
        );
    }

    #[test]
    fn test_tone_ignores_capture_endpoint() {
        assert_eq!(
            sender_args("Kitchen", Some("Win AirPlay"), true, 2),
            [
                "stream",
                "--target",
                "Kitchen",
                "--audible",
                "--source",
                "sine",
                "--duration",
                "2"
            ]
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
