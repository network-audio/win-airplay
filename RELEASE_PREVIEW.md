# Win AirPlay development preview

This installer adds AirPlay 2 speakers to the Windows sound output picker. It includes the Rust controller, the pinned usbip-win2 client and USB drivers, and Nyx Refrain linked into the controller. No separate Nyx installation is needed. Restart Windows if the installer requests it.

The controller discovers reachable speakers, presents each as a virtual USB audio output, and starts at sign-in for the installing user. Select a speaker in the Windows sound picker, then run `test-playback.ps1` from the install folder for a test tone. Logs are in `%LOCALAPPDATA%\WinAirPlay\controller-usbip.log` and rotate at 1 MiB, with at most two files.

This preview timestamps every controller log line and ignores an abrupt receiver-reported zero volume while the Windows slider is still well above zero. Receiver volume changes update the Windows slider without being sent back to the same speaker. Please check whether The Frame stays audible after its zero-volume events.

Windows CI has tested two simultaneous outputs, PCM input, endpoint volume and mute controls, and removal after detach. Audible playback to an AirPlay receiver and live disconnect behavior still need testing on a Windows machine. Please report the speaker model, Windows version, installation result, and controller log when testing.
