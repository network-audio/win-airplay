# Win AirPlay

Win AirPlay adds discovered AirPlay 2 speakers to the standard Windows sound output picker. Select a speaker to send Windows audio to it. The controller keeps an output for each reachable speaker and removes it when the speaker disappears; it does not switch your default output.

<img width="300" alt="Windows sound output" src="https://github.com/user-attachments/assets/9803736c-e54c-436c-a61f-c6c389563071" />

## Install

1. Download the [Windows installer](https://github.com/stakach/win-airplay/releases/latest) for your architecture (x64 or ARM64).
2. Run it and approve the administrator prompt. It installs Win AirPlay and the bundled USBip client, then starts the controller at sign-in. Restart Windows if prompted.
3. Select an AirPlay speaker in the Windows sound picker. Win AirPlay runs in the background with no separate app window. For a playback check, run `test-playback.ps1` from the install folder. Logs are at `%LOCALAPPDATA%\WinAirPlay\controller-usbip.log` and rotate at 1 MiB, retaining at most two files.

## How it works

The Rust controller discovers speakers and serves a virtual USB audio device for each one over USB/IP on `127.0.0.1` (with the [usbip-audio](https://github.com/stakach/usbip-audio) crate, on a free port, so other applications can share the USB/IP driver). The bundled usbip-win2 client attaches those devices to Windows, which presents them as ordinary USB sound outputs. The controller forwards their PCM to the in-process [Nyx Refrain](https://github.com/pStrikeZ/Nyx-Refrain) AirPlay sender. It selects audio encoding from each speaker's mDNS codec advertisement and uses its AirPlay capability response to select timing, with a compatible fallback when capabilities are missing. Windows volume, mute, and channel balance controls are handled through the USB audio device. Receiver volume changes are mirrored to Windows when that speaker is the selected output.

The installer includes the controller, the pinned usbip-win2 installer and client, the Nyx code linked into the controller, and a playback test tone. No separate Nyx installation is needed. The [USB/IP implementation notes](docs/usbip-win2-feasibility.md) describe current testing and remaining hardware checks.

## Build

```powershell
git clone --recurse-submodules https://github.com/stakach/win-airplay.git
cd win-airplay
cargo test --locked
cargo build --release --locked
```

GitHub Actions builds the Windows installer and tests virtual speaker enumeration, PCM input, endpoint controls, and removal.
