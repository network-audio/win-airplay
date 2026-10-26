# Win AirPlay

Experimental Windows AirPlay 2 audio outputs. A small SysVAD-based kernel driver provides standard Windows render endpoints. The Rust controller discovers AirPlay receivers, creates one software PnP device per receiver, and streams from the selected output using [Nyx Refrain](https://github.com/pStrikeZ/Nyx-Refrain) Rust crates linked into `win-airplay.exe`. Nyx is a pinned Git submodule; there is no separate sender executable to install.

## Status

The controller and driver compile in GitHub Actions. The Rust controller has unit tests on Linux and Windows, and local discovery found Kitchen, Living Room, and The Frame. A local Nyx test tone sent 364 AirPlay 2 packets to Kitchen with timing replies and no retransmit requests.

**Windows endpoint enumeration, playback, volume, mute, reconnect, and uninstall have not yet passed end-to-end tests.** The installer built by CI is a test-signed preview, not a production release. A full release requires Windows playback testing.

This development build does not require an EV certificate or Hardware Dev Center account. Windows x64 still requires the self-signed test driver to run in [Test Mode](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option). The user must enable Test Mode and restart before installing the preview; Secure Boot may need to be disabled in firmware. The installer does not change boot settings. Return the machine to its normal boot settings when testing is finished.

## What the controller does

- `win-airplay discover` lists `_airplay._tcp` and `_raop._tcp` services on the local network.
- `win-airplay run` continuously discovers speakers and keeps a software PnP device open for each AirPlay 2 receiver. The driver names its Windows output `AirPlay <speaker> [short ID]`.
- When a user selects one of these outputs as the default Windows render endpoint, the controller starts an in-process AirPlay session with WASAPI loopback capture from that endpoint. Switching back to a non-AirPlay output stops the session. Discovery alone does not request a default-output change, although Windows may choose a newly added output itself.
- The controller polls the selected Windows endpoint's volume and mute state and sends changes to the receiver over the active AirPlay session. Because WASAPI loopback normally taps before endpoint volume, the receiver volume command is required for the tray controls to affect loudness.
- After three consecutive discovery scans without a speaker, the controller closes its software device and removes its output. A selected speaker's stream stops when it disappears; Windows then selects an available output according to its own audio policy.
- `win-airplay stream --target "Kitchen" --test-tone` sends a three-second tone through the linked AirPlay crates on Windows.

The `run` command must run elevated because [SwDeviceCreate requires Administrator access](https://learn.microsoft.com/en-us/windows/win32/api/swdevice/nf-swdevice-swdevicecreate). The installer registers a highest-privilege interactive logon task for the installing user. It also imports the public test certificate into the local machine trust stores and stages the test-signed driver with `pnputil`.

## Development

```powershell
cargo test --locked
cargo build --release --locked
target\release\win-airplay.exe discover
```

Clone with `git clone --recurse-submodules`, or run `git submodule update --init` after cloning. GitHub Actions builds the Rust binary and WDK driver on Windows, creates an unsigned driver artifact, then builds a self-signed test package and an NSIS preview installer. The linked Nyx crates retain their MIT license in the installer. The SysVAD-derived code retains Microsoft's Public License in `driver/MICROSOFT-PUBLIC-LICENSE.txt`.

The GitHub-hosted runners can compile and package the driver but cannot access speakers on the local network or verify audible playback. Windows playback verification remains required before a first release.
