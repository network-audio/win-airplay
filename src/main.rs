use clap::{Parser, Subcommand};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::IpAddr;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

const SERVICE_TYPES: [&str; 2] = ["_airplay._tcp.local.", "_raop._tcp.local."];

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
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
struct Speaker {
    name: String,
    address: IpAddr,
    port: u16,
    service: String,
    model: Option<String>,
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
                    };
                    found.insert((name.clone(), address, *service), speaker);
                }
            }
        }
    }
    let _ = mdns.shutdown();
    Ok(found.into_values().collect())
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
    }
    Ok(())
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
}
