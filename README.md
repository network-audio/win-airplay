# Win AirPlay

Experimental Windows AirPlay 2 audio outputs. A small SysVAD-based kernel driver provides standard Windows render endpoints. The Rust controller discovers AirPlay receivers, creates one software PnP device per receiver, and runs a user-space [Nyx Refrain](https://github.com/pStrikeZ/Nyx-Refrain) sender for the output selected in the Windows tray.

## Status

The controller and driver compile in GitHub Actions. The Rust controller has unit tests on Linux and Windows, and local discovery found Kitchen, Living Room, and The Frame. A local Nyx test tone sent 364 AirPlay 2 packets to Kitchen with timing replies and no retransmit requests.

**Windows endpoint enumeration, playback, reconnect, and uninstall have not yet passed end-to-end tests.** The installer built by CI is a test-signed preview, not a production release. A preview installer will be published to GitHub Releases after its build and package checks pass. A full release requires Windows playback testing.

This development build does not require an EV certificate or Hardware Dev Center account. Windows x64 still requires the self-signed test driver to run in [Test Mode](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option). The user must enable Test Mode and restart before installing the preview; Secure Boot may need to be disabled in firmware. The installer does not change boot settings. Return the machine to its normal boot settings when testing is finished.

## What the controller does

- `win-airplay discover` lists `_airplay._tcp` and `_raop._tcp` services on the local network.
- `win-airplay run --sender path\to\nyxr.exe` keeps a software PnP device open for each discovered AirPlay receiver. The driver names its Windows output `AirPlay <speaker> [short ID]`.
- When a user selects one of these outputs as the default Windows render endpoint, the controller starts Nyx Refrain with WASAPI loopback capture from that endpoint and streams to the matching receiver. Switching back to a non-AirPlay output stops the sender.
- `win-airplay stream --target "Kitchen" --test-tone` asks Nyx to send a three-second tone, useful for transport testing without Windows audio capture.

The `run` command must run elevated because [SwDeviceCreate requires Administrator access](https://learn.microsoft.com/en-us/windows/win32/api/swdevice/nf-swdevice-swdevicecreate). The installer registers a highest-privilege interactive logon task for the installing user. It also imports the public test certificate into the local machine trust stores and stages the test-signed driver with `pnputil`.

## Development

```powershell
cargo test --locked
cargo build --release --locked
target\release\win-airplay.exe discover
```

GitHub Actions builds the Rust binary and WDK driver on Windows, creates an unsigned driver artifact, then builds a self-signed test package and an NSIS preview installer. It builds `nyxr.exe` from a pinned upstream commit and bundles its MIT license. The SysVAD-derived code retains Microsoft's Public License in `driver/MICROSOFT-PUBLIC-LICENSE.txt`.

The GitHub-hosted runners can compile and package the driver but cannot access speakers on the local network or verify audible playback. Windows playback verification remains required before a first release.
