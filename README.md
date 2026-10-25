# Win AirPlay

An early Windows AirPlay 2 output project. The Rust controller discovers local receivers and can launch the [Nyx Refrain](https://github.com/pStrikeZ/Nyx-Refrain) sender. A reduced Microsoft SysVAD endpoint is included as the first Windows audio endpoint prototype.

## Current status

- `win-airplay discover` finds `_airplay._tcp` and `_raop._tcp` services. It has been tested on the local network with Kitchen, Living Room, and The Frame.
- `win-airplay stream --target "Kitchen"` starts Nyx Refrain with WASAPI loopback capture of the Windows default output. Add `--endpoint "Win AirPlay"` to capture the prototype virtual endpoint once installed.
- `win-airplay stream --target "Kitchen" --test-tone` requests a three second test tone from Nyx Refrain.
- The virtual endpoint source is adapted from Microsoft's [SysVAD sample](https://github.com/microsoft/Windows-driver-samples/tree/main/audio/sysvad), licensed under the Microsoft Public License in `driver/MICROSOFT-PUBLIC-LICENSE.txt`. It currently exposes one render endpoint called **Win AirPlay**, rather than a separate endpoint for each discovered speaker.
- The endpoint package is a development driver. GitHub Actions can build it, but Windows requires signing and installation on a test PC. The CI runner cannot verify endpoint enumeration, loopback capture, or actual playback.

The AirPlay sender is an external program and is not bundled here. Install/build `nyxr` from Nyx Refrain and put it on `PATH`, or pass `--sender C:\\path\\to\\nyxr.exe`. Its source is MIT licensed. The controller launches `nyxr` as a child process, so its AirPlay pairing store and device compatibility follow Nyx Refrain.

## Build and use

```powershell
cargo build --release
target\release\win-airplay.exe discover
target\release\win-airplay.exe stream --target "Kitchen"
target\release\win-airplay.exe stream --target "Kitchen" --test-tone
```

For the prototype endpoint, build the driver with the Windows Driver Kit and install the resulting `ComponentizedAudioSample.inf` on a Windows test machine following the SysVAD sample's driver signing and installation instructions. The hardware ID in this fork is `Root\WinAirPlay`. Select **Win AirPlay** in Windows Sound output, then run:

```powershell
win-airplay stream --target "Kitchen" --endpoint "Win AirPlay"
```

## Architecture and remaining work

Windows creates tray output choices from audio endpoints registered by a driver. [AudioEndpointBuilder examines kernel streaming device interfaces](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/audio-endpoint-builder-algorithm), so a pure user space process cannot register a normal render endpoint. The SysVAD component supplies that small system-facing endpoint; the Rust controller and sender run in user space. The sender uses [WASAPI loopback](https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording) to capture the mixed audio from it.

The primary remaining feature is **one stable output endpoint per AirPlay 2 device**, with live discovery, removal, and automatic routing when an endpoint is selected. This requires dynamic endpoint management in the driver and a long-running Rust service. The current controller requires an explicit target. We also need Windows hardware testing, driver signing, pairing UX, reconnect handling, and verification across receiver models before this can be installed as a normal driver.
