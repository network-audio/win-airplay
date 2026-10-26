# usbip-win2 feasibility spike

## Verdict

**Possible in principle, but unproven for Win AirPlay.** The unmodified, upstream-signed usbip-win2 package could replace our SysVAD package as the installed kernel component. It cannot make Win AirPlay driver-free: usbip-win2 installs a virtual USB controller driver and a USB filter driver. Our Rust process would have to emulate a USB Audio Class 2.0 device for each speaker and serve those devices over USB/IP on localhost. Windows would then load its built-in `usbaudio2.sys` driver and create the sound-picker outputs.

This is a substantially different audio backend, not a drop-in substitute for SysVAD. Keep the current backend until a Windows playback spike demonstrates that the USB/IP route works reliably.

## Evidence

- [usbip-win2](https://github.com/vadimgrn/usbip-win2) describes its UDE kernel driver as a USB/IP client, requires a protocol 1.1.1 server, and documents its Microsoft-signed releases. Its Windows installer also restarts USB 3.0 hubs. Release [0.9.8.1](https://github.com/vadimgrn/usbip-win2/releases/tag/v.0.9.8.1) is the candidate to test; verify the actual package signature on Windows before using it in our installer.
- The upstream source contains a localhost attach example in `userspace/wusbip/testing.txt` and has isochronous transfer handling in `drivers/ude/device_ioctl.cpp` and `drivers/ude/wsk_receive.cpp` ([source at inspected commit](https://github.com/vadimgrn/usbip-win2/tree/9ff1fe0c71979e5b26c69f5b8dd4b1eeef6b7578)). This establishes protocol support in the client, not working USB audio playback.
- [Microsoft's USB Audio 2.0 documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/usb-2-0-audio-drivers) says a compliant device binds to the in-box `usbaudio2.sys` driver. Its Feature Unit supports volume and mute; its audio streaming interface uses isochronous transfers. Those controls would need to be translated to AirPlay receiver controls by our server.
- A [reported localhost virtual-device kernel crash](https://github.com/vadimgrn/usbip-win2/issues/172) affected an older usbip-win2 release. The maintainer says the relevant spinlock was removed in 0.9.7.8. This is a reason to stress-test the current signed release, not evidence that the current release is broken.

## Required Rust backend

1. Serve USB/IP device-list and import requests on loopback, with one stable USB identity per discovered speaker. Attach and detach each device as discovery changes; never change the Windows default output automatically.
2. Emulate USB enumeration, a compliant AudioControl and AudioStreaming descriptor set, clock and Feature Unit requests, stream alternate settings, and isochronous OUT transfers. Convert received PCM into the existing Nyx AirPlay sender path. Map Windows mute, master volume and channel volume to the existing receiver control and PCM gain logic; reflect receiver-side changes back where the class driver allows it.
3. Handle sender failures and speaker disappearance by removing only the corresponding virtual USB device. Reattach with a stable identity if that speaker returns, and check what Windows chooses when the active output disappears.
4. Bundle the exact, unmodified upstream signed driver package and license notices, or have the installer download a pinned release with a checked hash. Avoid rebuilding or altering its driver files, which would invalidate the upstream signature. Remove the SysVAD package only after the replacement passes Windows tests.

## Go/no-go experiment on Windows

Build a minimal Rust USB/IP server exposing one fixed-format stereo USB Audio 2.0 output. Install the signed upstream usbip-win2 release, attach to `localhost`, and confirm that the device appears in the Windows sound picker and passes audible PCM. Then verify Windows volume, mute, channel balance, repeated attach/detach, hot discovery, active-speaker loss, and long playback under load. Record latency and check for USB/IP or audio-stack errors. Finally test the exact bundled installer with Test Mode off and Secure Boot on. GitHub Actions can build and validate packages, but cannot establish audible playback or reliability without a Windows hardware test host.

**Current status:** static source and documentation review only; no USB/IP server, Windows attach, or audible playback test has been run.
