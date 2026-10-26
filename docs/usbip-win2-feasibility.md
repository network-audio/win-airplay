# usbip-win2 feasibility spike

## Verdict

**Implemented as an experimental backend; Windows playback remains unverified.** The unmodified, upstream-signed usbip-win2 package could replace our SysVAD package as the installed kernel component. It cannot make Win AirPlay driver-free: usbip-win2 installs a virtual USB controller driver and a USB filter driver. Our Rust process now emulates a USB Audio Class 1.0 device for each speaker and serves those devices over USB/IP on localhost. Windows should load its built-in USB audio class driver and create sound-picker outputs, subject to the Windows smoke test below.

This is a substantially different audio backend from SysVAD. Keep the current backend until Windows playback and lifecycle tests demonstrate that the USB/IP route works reliably.

## Current implementation

- `win-airplay run-usbip` discovers reachable AirPlay speakers and maintains one virtual USB speaker per receiver. The server binds only to `127.0.0.1:3240` and uses the installed usbip-win2 `usbip.exe` to attach and detach them. It does not set the Windows default output.
- The all-Rust USB/IP server handles device listing, import, enumeration, USB Audio 1.0 descriptors and controls, isochronous OUT PCM, and unplug on disappearance. It forwards 44.1 kHz stereo PCM to the existing in-process Nyx AirPlay sender. Windows master volume and mute map to AirPlay; channel gains apply to PCM. Receiver volume changes update the virtual device state and, when selected, the Windows endpoint control.
- `win-airplay usbip-probe` exports a fixed test speaker for a Windows enumeration test. The branch has a separate bundled-installer preview and a GitHub Actions Windows attach smoke job. The bundled upstream release is pinned by SHA-256 and includes its BSD license.
- Local protocol tests pass for device list/import, configuration descriptor, volume control, and isochronous PCM. Windows cross-target Clippy passes. No real Windows USB attachment or audible playback has passed yet.

## Evidence

- [usbip-win2](https://github.com/vadimgrn/usbip-win2) describes its UDE kernel driver as a USB/IP client, requires a protocol 1.1.1 server, and documents its Microsoft-signed releases. Its Windows installer also restarts USB 3.0 hubs. Release [0.9.8.1](https://github.com/vadimgrn/usbip-win2/releases/tag/v.0.9.8.1) is the candidate to test; verify the actual package signature on Windows before using it in our installer.
- The upstream source contains a localhost attach example in `userspace/wusbip/testing.txt` and has isochronous transfer handling in `drivers/ude/device_ioctl.cpp` and `drivers/ude/wsk_receive.cpp` ([source at inspected commit](https://github.com/vadimgrn/usbip-win2/tree/9ff1fe0c71979e5b26c69f5b8dd4b1eeef6b7578)). This establishes protocol support in the client, not working USB audio playback.
- [Microsoft's USB Audio class documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/usb-audio-class-system-driver--usbaudio-sys-) describes the in-box class driver for USB Audio 1.0 devices. The [USB Audio 1.0 specification](https://www.usb.org/sites/default/files/audio10.pdf) defines Feature Unit volume and mute controls and isochronous audio streaming.
- A [reported localhost virtual-device kernel crash](https://github.com/vadimgrn/usbip-win2/issues/172) affected an older usbip-win2 release. The maintainer says the relevant spinlock was removed in 0.9.7.8. This is a reason to stress-test the current signed release, not evidence that the current release is broken.

## Remaining parity checks

1. Confirm the current descriptors bind to Windows `usbaudio.sys` and expose a render endpoint for each speaker. A production USB VID/PID allocation is needed before general release; the prototype uses a test identifier.
2. Check audible playback, tray volume, mute, balance, incoming receiver volume, simultaneous speakers, default-device preservation, repeated attach/detach, and removal of a selected output. Tune USB timing and buffering based on measured latency and underruns.
3. Check the exact bundled installer on Windows with Test Mode off and Secure Boot on. Its nested usbip-win2 installer can require a reboot. Verify the installed driver catalog signature and installer upgrade/uninstall behavior. The preview uninstaller leaves the shared usbip-win2 installation in place.

## Go/no-go experiment on Windows

The `Build and test` workflow runs a Windows smoke job on this branch. It installs the pinned upstream release on an ephemeral Windows runner, starts `usbip-probe`, attaches to localhost, and looks for the PnP device. Then test the preview installer with a real AirPlay receiver on a Windows machine: confirm it appears in the sound picker and passes audible PCM. Verify volume, mute, balance, attach/detach, hot discovery, active-speaker loss, and long playback under load. Record latency and check for USB/IP or audio-stack errors. Finally test with Test Mode off and Secure Boot on. GitHub Actions can validate enumeration and packages, but cannot establish audible playback or reliability without a Windows hardware test host.

**Current status:** Rust server, AirPlay bridge, installer preview and protocol tests implemented; Windows attach and audible playback pending.
