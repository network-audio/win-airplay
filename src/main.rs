use clap::{Parser, Subcommand};
use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
#[cfg(any(windows, test))]
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::process::ExitCode;
use std::time::{Duration, Instant};

const SERVICE_TYPES: [&str; 2] = ["_airplay._tcp.local.", "_raop._tcp.local."];

#[cfg(windows)]
mod airplay;
mod usbip_audio;
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
    /// Use usbip-win2 and a Rust virtual USB audio server for the outputs.
    RunUsbip,
    /// Export one virtual USB speaker for a Windows USB/IP enumeration check.
    #[command(hide = true)]
    UsbipProbe {
        #[arg(long, default_value_t = 90)]
        seconds: u64,
        #[arg(long)]
        expect_pcm: bool,
        #[arg(long)]
        two_speakers: bool,
    },
    /// Send a WASAPI test tone directly to a named output without selecting it.
    #[command(hide = true)]
    UsbipRenderTest {
        #[arg(long)]
        endpoint: String,
    },
    /// Check Windows master, channel, and mute controls for a named USB output.
    #[command(hide = true)]
    UsbipControlsTest {
        #[arg(long)]
        endpoint: String,
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
    codecs: Vec<u8>,
    low_latency_audio: bool,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AudioEncoding {
    Pcm,
    Alac,
    CompressedAlac,
}

#[cfg(any(windows, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct StreamProfile {
    encoding: AudioEncoding,
    latency_frames: i32,
    start_lead_ms: u32,
}

#[cfg(any(windows, test))]
impl Default for StreamProfile {
    fn default() -> Self {
        Self {
            encoding: AudioEncoding::Alac,
            latency_frames: 66_150,
            start_lead_ms: 1_000,
        }
    }
}

#[cfg(any(windows, test))]
impl StreamProfile {
    fn prefill_ms(self) -> u32 {
        // Low-latency receivers need packets and the first sync before their
        // playout time. This changes network lead, not playback latency.
        if self.latency_frames < 0 {
            200
        } else {
            0
        }
    }
}

#[cfg(any(windows, test))]
impl Speaker {
    fn stream_profile(&self) -> StreamProfile {
        let encoding = if self.codecs.contains(&1) {
            AudioEncoding::CompressedAlac
        } else if self.codecs.contains(&0) {
            AudioEncoding::Pcm
        } else {
            // AirPlay-only advertisements can omit `cn`; retain the tested fallback.
            AudioEncoding::Alac
        };
        if self.low_latency_audio {
            // /info explicitly advertises a low-latency audio stream format.
            StreamProfile {
                encoding,
                latency_frames: -1_764, // -40 ms at 44.1 kHz
                start_lead_ms: 300,
            }
        } else {
            StreamProfile {
                encoding,
                ..Default::default()
            }
        }
    }
}

fn parse_codecs(value: Option<&str>) -> Vec<u8> {
    let mut codecs: Vec<u8> = value
        .unwrap_or_default()
        .split(',')
        .filter_map(|part| part.trim().parse().ok())
        .collect();
    codecs.sort_unstable();
    codecs.dedup();
    codecs
}

fn info_has_low_latency_audio(info: &plist::Value) -> bool {
    info.as_dictionary()
        .and_then(|info| info.get("supportedFormats"))
        .and_then(plist::Value::as_dictionary)
        .and_then(|formats| formats.get("lowLatencyAudioStream"))
        .and_then(plist::Value::as_unsigned_integer)
        .is_some_and(|formats| formats != 0)
}

fn probe_low_latency_audio(address: IpAddr, port: u16) -> bool {
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(
        &SocketAddr::new(address, port),
        Duration::from_millis(400),
    ) else {
        return false;
    };
    let timeout = Some(Duration::from_millis(400));
    if stream.set_read_timeout(timeout).is_err() || stream.set_write_timeout(timeout).is_err() {
        return false;
    }
    if stream
        .write_all(b"GET /info RTSP/1.0\r\nCSeq: 1\r\nContent-Length: 0\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut reader = BufReader::new(stream);
    let mut status = String::new();
    if reader.read_line(&mut status).is_err() || !status.starts_with("RTSP/1.0 200 ") {
        return false;
    }
    let mut body_len = None;
    for _ in 0..64 {
        let mut line = String::new();
        if !matches!(reader.read_line(&mut line), Ok(1..=8192)) {
            return false;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                body_len = value.trim().parse::<usize>().ok();
            }
        }
    }
    let Some(body_len) = body_len.filter(|length| *length <= 64 * 1024) else {
        return false;
    };
    let mut body = vec![0; body_len];
    if reader.read_exact(&mut body).is_err() {
        return false;
    }
    plist::Value::from_reader(std::io::Cursor::new(body))
        .is_ok_and(|info| info_has_low_latency_audio(&info))
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
                let codecs = parse_codecs(info.get_property_val_str("cn"));
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
                        codecs: codecs.clone(),
                        low_latency_audio: false,
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
    let raop_codecs: BTreeMap<_, _> = speakers
        .iter()
        .filter(|speaker| speaker.service == SERVICE_TYPES[1])
        .map(|speaker| (speaker.address, speaker.codecs.clone()))
        .collect();
    Ok(speakers
        .into_iter()
        .map(|mut speaker| {
            if speaker.codecs.is_empty() {
                if let Some(codecs) = raop_codecs.get(&speaker.address) {
                    speaker.codecs = codecs.clone();
                }
            }
            if speaker.service == SERVICE_TYPES[0] {
                speaker.low_latency_audio = probe_low_latency_audio(speaker.address, speaker.port);
            }
            speaker
        })
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

#[cfg(any(windows, test))]
fn is_receiver_volume_echo(last: Option<(f32, Instant)>, scalar: f32, mute_changed: bool) -> bool {
    !mute_changed
        && last.is_some_and(|(reported, at)| {
            at.elapsed() < Duration::from_secs(3) && (reported - scalar).abs() < 0.02
        })
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
        Action::RunUsbip => run_usbip_manager()?,
        Action::UsbipProbe {
            seconds,
            expect_pcm,
            two_speakers,
        } => {
            let server = usbip_audio::Server::start(3240).map_err(|error| error.to_string())?;
            let streams = server.add("0123456789abcdef", "Win AirPlay USB Probe");
            let second =
                two_speakers.then(|| server.add("fedcba9876543210", "Win AirPlay USB Probe Two"));
            println!(
                "USB/IP probe busid={} port={}",
                streams.busid,
                server.port()
            );
            if let Some(second) = &second {
                println!("USB/IP second probe busid={}", second.busid);
            }
            if expect_pcm {
                let deadline = Instant::now() + Duration::from_secs(seconds);
                let mut received = [0_usize; 2];
                let mut nonzero = [0_usize; 2];
                while Instant::now() < deadline {
                    for (index, source) in [Some(&streams), second.as_ref()].into_iter().enumerate()
                    {
                        if let Some(stream) = source {
                            while let Some(packet) = stream.pcm.pop_timeout(Duration::ZERO) {
                                received[index] += packet.len();
                                nonzero[index] += packet.iter().filter(|&&byte| byte != 0).count();
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                for index in 0..if two_speakers { 2 } else { 1 } {
                    if nonzero[index] == 0 {
                        return Err(format!(
                            "No nonzero PCM reached virtual USB audio endpoint {} ({} bytes total)",
                            index + 1,
                            received[index]
                        ));
                    }
                    println!(
                        "USB/IP speaker {} received {} PCM bytes ({} nonzero)",
                        index + 1,
                        received[index],
                        nonzero[index]
                    );
                }
            } else {
                std::thread::sleep(Duration::from_secs(seconds));
            }
            server.remove(&streams.busid);
            if let Some(second) = &second {
                server.remove(&second.busid);
            }
        }
        Action::UsbipRenderTest { endpoint } => {
            #[cfg(windows)]
            {
                windows_audio::initialize()?;
                windows_audio::play_test_tone(&endpoint, Duration::from_secs(3))?;
            }
            #[cfg(not(windows))]
            {
                let _ = endpoint;
                return Err("WASAPI render testing requires Windows".into());
            }
        }
        Action::UsbipControlsTest { endpoint } => {
            #[cfg(windows)]
            {
                windows_audio::initialize()?;
                windows_audio::test_endpoint_controls(&endpoint)?;
            }
            #[cfg(not(windows))]
            {
                let _ = endpoint;
                return Err("Windows endpoint control testing requires Windows".into());
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn run_manual_stream(target: SocketAddr, endpoint: Option<String>) -> Result<(), String> {
    let endpoint = endpoint.unwrap_or_else(|| "AirPlay".into());
    let stream = airplay::StreamHandle::start(
        target,
        endpoint,
        StreamProfile::default(),
        airplay::Volume {
            scalar: 0.5,
            muted: false,
        },
        vec![1.0, 1.0],
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
        profile: StreamProfile,
        stream: Option<airplay::StreamHandle>,
        remote_volume: Option<mpsc::Receiver<f32>>,
        last_volume: Option<airplay::Volume>,
        last_channel_gains: Vec<f32>,
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
                let profile = speaker.stream_profile();
                if let Some(managed) = active.get_mut(&key) {
                    managed.missed_scans = 0;
                    if managed.target != target {
                        if let Some(stream) = managed.stream.take() {
                            stream.stop();
                        }
                        managed.remote_volume = None;
                    }
                    managed.target = target;
                    managed.profile = profile;
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
                                profile,
                                stream: None,
                                remote_volume: None,
                                last_volume: None,
                                last_channel_gains: Vec::new(),
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
                managed.last_channel_gains.clear();
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
                managed.last_channel_gains.clear();
            }
            if managed.missed_scans < 3
                && managed.stream.is_none()
                && managed.last_spawn.elapsed() >= Duration::from_secs(5)
            {
                let volume = default_output.as_ref().expect("selected output").volume;
                let gains = default_output
                    .as_ref()
                    .expect("selected output")
                    .channel_gains
                    .clone();
                let (remote_volume_tx, remote_volume_rx) = mpsc::channel();
                managed.stream = Some(airplay::StreamHandle::start(
                    managed.target,
                    managed.endpoint.clone(),
                    managed.profile,
                    volume,
                    gains.clone(),
                    Some(remote_volume_tx),
                ));
                managed.remote_volume = Some(remote_volume_rx);
                managed.last_volume = Some(volume);
                managed.last_channel_gains = gains;
                managed.last_spawn = Instant::now();
            } else if let (Some(stream), Some(output)) = (&managed.stream, &default_output) {
                let balance_changed = managed.last_channel_gains.len()
                    != output.channel_gains.len()
                    || managed
                        .last_channel_gains
                        .iter()
                        .zip(&output.channel_gains)
                        .any(|(old, new)| (old - new).abs() >= 0.005);
                if balance_changed {
                    stream.set_channel_gains(output.channel_gains.clone());
                    managed.last_channel_gains = output.channel_gains.clone();
                }
                let mut remote_applied = false;
                if let Some(remote) = &managed.remote_volume {
                    while let Ok(scalar) = remote.try_recv() {
                        if let Err(error) =
                            windows_audio::set_render_volume(&managed.endpoint, scalar)
                        {
                            eprintln!("Could not mirror speaker volume to Windows: {error}");
                        } else {
                            managed.last_volume = Some(airplay::Volume {
                                scalar,
                                muted: output.volume.muted,
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

#[cfg(windows)]
fn usbip_executable() -> std::path::PathBuf {
    std::env::var_os("WIN_AIRPLAY_USBIP_EXE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(r"C:\Program Files\USBip\usbip.exe"))
}

#[cfg(windows)]
fn usbip_attach(port: u16, busid: &str) -> Result<u32, String> {
    let output = std::process::Command::new(usbip_executable())
        .args([
            "--tcp-port",
            &port.to_string(),
            "attach",
            "--remote",
            "127.0.0.1",
            "--bus-id",
            busid,
            "--terse",
        ])
        .output()
        .map_err(|error| format!("usbip-win2 attach failed to start: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "usbip-win2 attach failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<u32>()
        .map_err(|error| format!("usbip-win2 returned an invalid port: {error}"))
}

#[cfg(windows)]
fn usbip_detach(port: u32) {
    if let Err(error) = std::process::Command::new(usbip_executable())
        .args(["detach", "--port", &port.to_string()])
        .status()
    {
        eprintln!("Could not detach USB/IP port {port}: {error}");
    }
}

#[cfg(windows)]
fn run_usbip_manager() -> Result<(), String> {
    use std::sync::mpsc;
    use std::sync::Arc;

    struct Managed {
        endpoint: String,
        busid: String,
        target: SocketAddr,
        profile: StreamProfile,
        pcm: Arc<usbip_audio::PcmQueue>,
        volume_events: mpsc::Receiver<usbip_audio::Volume>,
        slider_volume: airplay::Volume,
        usb_muted: bool,
        channel_gains: [f32; 2],
        stream: Option<airplay::StreamHandle>,
        remote_volume: Option<mpsc::Receiver<f32>>,
        last_receiver_volume: Option<(f32, Instant)>,
        port: Option<u32>,
        last_attach: Instant,
        last_spawn: Instant,
        missed_scans: u8,
    }

    windows_audio::initialize()?;
    let server =
        usbip_audio::Server::start(3240).map_err(|error| format!("USB/IP server: {error}"))?;
    let (discovery_tx, discovery_rx) = mpsc::channel();
    std::thread::spawn(move || {
        // Keep one mDNS browser alive so a speaker remains known between
        // announcements. Recreating a browser for every scan produced empty
        // snapshots and detached every Windows output at once.
        let mdns = match ServiceDaemon::new() {
            Ok(mdns) => mdns,
            Err(error) => {
                eprintln!("AirPlay discovery failed: {error}");
                return;
            }
        };
        let browser = match mdns.browse(SERVICE_TYPES[0]) {
            Ok(browser) => browser,
            Err(error) => {
                eprintln!("AirPlay browse failed: {error}");
                return;
            }
        };
        let raop_browser = match mdns.browse(SERVICE_TYPES[1]) {
            Ok(browser) => Some(browser),
            Err(error) => {
                eprintln!("RAOP codec discovery unavailable: {error}");
                None
            }
        };
        let mut known: BTreeMap<String, Vec<Speaker>> = BTreeMap::new();
        let mut known_raop: BTreeMap<String, Vec<(IpAddr, Vec<u8>)>> = BTreeMap::new();
        let mut next_report = Instant::now();
        loop {
            match browser.recv_timeout(Duration::from_millis(100)) {
                Ok(ServiceEvent::ServiceResolved(info)) => {
                    let fullname = info.get_fullname().to_owned();
                    let name = instance_name(&fullname, SERVICE_TYPES[0]);
                    let speakers = info
                        .get_addresses()
                        .iter()
                        .copied()
                        .filter(|address| address.is_ipv4())
                        .map(|address| Speaker {
                            name: name.clone(),
                            address,
                            port: info.get_port(),
                            service: SERVICE_TYPES[0].to_owned(),
                            model: info.get_property_val_str("model").map(str::to_owned),
                            device_id: info.get_property_val_str("deviceid").map(str::to_owned),
                            codecs: parse_codecs(info.get_property_val_str("cn")),
                            low_latency_audio: probe_low_latency_audio(address, info.get_port()),
                        })
                        .collect();
                    known.insert(fullname, speakers);
                }
                Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                    known.remove(&fullname);
                }
                _ => {}
            }
            if let Some(raop_browser) = &raop_browser {
                match raop_browser.recv_timeout(Duration::from_millis(50)) {
                    Ok(ServiceEvent::ServiceResolved(info)) => {
                        let codecs = parse_codecs(info.get_property_val_str("cn"));
                        let addresses = info
                            .get_addresses()
                            .iter()
                            .copied()
                            .filter(|address| address.is_ipv4())
                            .map(|address| (address, codecs.clone()))
                            .collect();
                        known_raop.insert(info.get_fullname().to_owned(), addresses);
                    }
                    Ok(ServiceEvent::ServiceRemoved(_, fullname)) => {
                        known_raop.remove(&fullname);
                    }
                    _ => {}
                }
            }
            if Instant::now() >= next_report {
                let raop_codecs: BTreeMap<IpAddr, Vec<u8>> =
                    known_raop.values().flatten().cloned().collect();
                let reachable: Vec<Speaker> = known
                    .values()
                    .flatten()
                    .map(|speaker| {
                        let mut speaker = speaker.clone();
                        if speaker.codecs.is_empty() {
                            if let Some(codecs) = raop_codecs.get(&speaker.address) {
                                speaker.codecs = codecs.clone();
                            }
                        }
                        speaker
                    })
                    .filter(|speaker| {
                        std::net::TcpStream::connect_timeout(
                            &SocketAddr::new(speaker.address, speaker.port),
                            Duration::from_millis(400),
                        )
                        .is_ok()
                    })
                    .collect();
                if discovery_tx.send(reachable).is_err() {
                    break;
                }
                next_report = Instant::now() + Duration::from_secs(5);
            }
        }
        let _ = mdns.shutdown();
    });
    let mut active: BTreeMap<String, Managed> = BTreeMap::new();
    let mut last_volume_check = Instant::now() - Duration::from_secs(1);
    loop {
        while let Ok(speakers) = discovery_rx.try_recv() {
            for managed in active.values_mut() {
                managed.missed_scans = managed.missed_scans.saturating_add(1);
            }
            for speaker in speakers {
                let key = speaker_key(&speaker);
                let target = SocketAddr::new(speaker.address, speaker.port);
                let profile = speaker.stream_profile();
                if let Some(managed) = active.get_mut(&key) {
                    managed.missed_scans = 0;
                    if managed.target != target {
                        if let Some(stream) = managed.stream.take() {
                            stream.stop();
                        }
                        managed.remote_volume = None;
                        managed.last_receiver_volume = None;
                        managed.target = target;
                    }
                    managed.profile = profile;
                    continue;
                }
                let endpoint = endpoint_name(&speaker);
                let streams = server.add(&key, &endpoint);
                let port = match usbip_attach(server.port(), &streams.busid) {
                    Ok(port) => Some(port),
                    Err(error) => {
                        eprintln!("Could not attach {endpoint}: {error}");
                        None
                    }
                };
                println!("USB output {endpoint} for {target}");
                active.insert(
                    key,
                    Managed {
                        endpoint,
                        busid: streams.busid,
                        target,
                        profile,
                        pcm: streams.pcm,
                        volume_events: streams.volume,
                        slider_volume: airplay::Volume {
                            scalar: 1.0,
                            muted: false,
                        },
                        usb_muted: false,
                        channel_gains: [1.0, 1.0],
                        stream: None,
                        remote_volume: None,
                        last_receiver_volume: None,
                        port,
                        last_attach: Instant::now(),
                        last_spawn: Instant::now() - Duration::from_secs(10),
                        missed_scans: 0,
                    },
                );
            }
            active.retain(|_, managed| {
                if managed.missed_scans < 3 {
                    return true;
                }
                if let Some(stream) = managed.stream.take() {
                    stream.stop();
                }
                if let Some(port) = managed.port {
                    usbip_detach(port);
                }
                server.remove(&managed.busid);
                println!("Removed USB output {}", managed.endpoint);
                false
            });
        }
        let check_volume = last_volume_check.elapsed() >= Duration::from_secs(2);
        for managed in active.values_mut() {
            if managed.port.is_none() && managed.last_attach.elapsed() >= Duration::from_secs(5) {
                managed.last_attach = Instant::now();
                match usbip_attach(server.port(), &managed.busid) {
                    Ok(port) => managed.port = Some(port),
                    Err(error) => eprintln!("Could not attach {}: {error}", managed.endpoint),
                }
            }
            let mut volume_changed = false;
            while let Ok(volume) = managed.volume_events.try_recv() {
                volume_changed = true;
                if managed.usb_muted != volume.muted {
                    eprintln!(
                        "USB audio mute changed: {} muted={}",
                        managed.endpoint, volume.muted
                    );
                    managed.usb_muted = volume.muted;
                }
                managed.channel_gains = volume.channels;
                if let Some(stream) = &managed.stream {
                    stream.set_channel_gains(volume.channels.to_vec());
                }
            }
            if let Some(remote) = &managed.remote_volume {
                while let Ok(scalar) = remote.try_recv() {
                    eprintln!(
                        "AirPlay receiver volume event: {} volume={scalar:.3}",
                        managed.endpoint
                    );
                    if let Err(error) = windows_audio::set_render_volume(&managed.endpoint, scalar)
                    {
                        eprintln!("Could not mirror speaker volume to Windows: {error}");
                    } else {
                        managed.slider_volume.scalar = scalar;
                        managed.last_receiver_volume = Some((scalar, Instant::now()));
                    }
                }
            }
            // Refresh the slider when a stream starts. A receiver-origin update
            // can trigger Windows notifications, but must not be sent back to
            // the same receiver as a new volume command.
            if managed.stream.is_some() && (check_volume || volume_changed) {
                if let Ok(volume) = windows_audio::render_output_volume(&managed.endpoint) {
                    if (managed.slider_volume.scalar - volume.scalar).abs() >= 0.005
                        || managed.slider_volume.muted != volume.muted
                    {
                        let mute_changed = managed.slider_volume.muted != volume.muted;
                        if mute_changed {
                            eprintln!(
                                "Windows endpoint mute changed: {} muted={} volume={:.3}",
                                managed.endpoint, volume.muted, volume.scalar
                            );
                        }
                        managed.slider_volume = volume;
                        let receiver_echo = is_receiver_volume_echo(
                            managed.last_receiver_volume,
                            volume.scalar,
                            mute_changed,
                        );
                        if !receiver_echo {
                            if let Some(stream) = &managed.stream {
                                stream.set_volume(volume);
                            }
                        }
                    }
                }
            }
            let inactive = managed
                .pcm
                .idle_for()
                .is_none_or(|age| age >= Duration::from_secs(10));
            if managed
                .stream
                .as_ref()
                .is_some_and(|stream| stream.is_finished())
                || inactive
            {
                if let Some(stream) = managed.stream.take() {
                    stream.stop();
                    println!(
                        "Stopped AirPlay stream for {} (idle={inactive})",
                        managed.endpoint
                    );
                }
                managed.remote_volume = None;
                managed.last_receiver_volume = None;
            }
            if managed.port.is_some()
                && managed.stream.is_none()
                && managed.pcm.has_data()
                && managed
                    .pcm
                    .idle_for()
                    .is_some_and(|age| age < Duration::from_secs(5))
                && managed.last_spawn.elapsed() >= Duration::from_secs(5)
            {
                if let Ok(volume) = windows_audio::render_output_volume(&managed.endpoint) {
                    managed.slider_volume = volume;
                }
                let (remote_tx, remote_rx) = mpsc::channel();
                println!("Starting AirPlay stream for {}", managed.endpoint);
                managed.stream = Some(airplay::StreamHandle::start_usbip(
                    managed.target,
                    managed.pcm.clone(),
                    managed.profile,
                    managed.slider_volume,
                    managed.channel_gains.to_vec(),
                    Some(remote_tx),
                ));
                managed.remote_volume = Some(remote_rx);
                managed.last_receiver_volume = None;
                managed.last_spawn = Instant::now();
            }
        }
        if check_volume {
            last_volume_check = Instant::now();
        }
        // With no AirPlay session, discovery and endpoint controls only need
        // occasional checks. Avoid waking the manager 20 times per second.
        let idle = active.values().all(|managed| managed.stream.is_none());
        std::thread::sleep(Duration::from_millis(if idle { 250 } else { 50 }));
    }
}

#[cfg(not(windows))]
fn run_usbip_manager() -> Result<(), String> {
    Err("USB/IP audio endpoint attachment requires Windows".into())
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
    fn receiver_volume_echo_does_not_hide_windows_mute() {
        let recent = Some((0.6, Instant::now()));
        assert!(is_receiver_volume_echo(recent, 0.61, false));
        assert!(!is_receiver_volume_echo(recent, 0.61, true));
        assert!(!is_receiver_volume_echo(recent, 0.3, false));
    }

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
            codecs: vec![0, 1],
            low_latency_audio: false,
        };
        let key = speaker_key(&speaker);
        speaker.address = "192.0.2.2".parse().unwrap();
        speaker.device_id = Some("aa:bb:cc:dd:ee:ff".into());
        assert_eq!(speaker_key(&speaker), key);
        assert_eq!(key.len(), 32);
    }

    #[test]
    fn mdns_codec_list_selects_capabilities_independent_of_model() {
        let mut speaker = Speaker {
            name: "Any speaker".into(),
            address: "192.0.2.1".parse().unwrap(),
            port: 7000,
            service: SERVICE_TYPES[0].into(),
            model: None,
            device_id: None,
            codecs: parse_codecs(Some("0,1,2,3")),
            low_latency_audio: true,
        };
        assert_eq!(
            speaker.stream_profile().encoding,
            AudioEncoding::CompressedAlac
        );
        assert_eq!(speaker.stream_profile().latency_frames, -1_764);
        assert_eq!(speaker.stream_profile().start_lead_ms, 300);
        assert_eq!(speaker.stream_profile().prefill_ms(), 200);
        speaker.codecs = vec![0];
        assert_eq!(speaker.stream_profile().encoding, AudioEncoding::Pcm);
        speaker.low_latency_audio = false;
        speaker.model = Some("Unrelated model".into());
        speaker.codecs = vec![0, 1];
        assert_eq!(
            speaker.stream_profile().encoding,
            AudioEncoding::CompressedAlac
        );
        assert_eq!(speaker.stream_profile().latency_frames, 66_150);
        assert_eq!(speaker.stream_profile().prefill_ms(), 0);
        speaker.model = Some("Another model".into());
        speaker.codecs.clear();
        assert_eq!(speaker.stream_profile(), StreamProfile::default());
        assert_eq!(parse_codecs(Some("1,0,1,broken")), vec![0, 1]);
    }

    #[test]
    fn low_latency_requires_explicit_info_support() {
        let mut formats = plist::Dictionary::new();
        formats.insert(
            "lowLatencyAudioStream".into(),
            plist::Value::Integer(1.into()),
        );
        let mut info = plist::Dictionary::new();
        info.insert("supportedFormats".into(), plist::Value::Dictionary(formats));
        let info = plist::Value::Dictionary(info);
        assert!(info_has_low_latency_audio(&info));
        assert!(!info_has_low_latency_audio(&plist::Value::Dictionary(
            plist::Dictionary::new()
        )));
    }
}
