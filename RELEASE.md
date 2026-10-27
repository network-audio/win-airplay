Win AirPlay adds discovered AirPlay 2 speakers to the Windows sound output picker. This update reports each virtual speaker’s playout delay through the USB Audio `bDelay` descriptor. USB Audio 1.0 can represent up to 255 ms; longer AirPlay delays are capped at that value. Video synchronization depends on the playback application and Windows audio stack, so this is a compatibility improvement rather than a guaranteed correction.

Download the installer for your Windows architecture (x64 or ARM64) below, run it, and approve the administrator prompt. The installer bundles the Win AirPlay controller and USBip client. It starts the controller at sign-in; no separate Nyx installation is needed. Restart Windows if prompted.

The controller runs in the background. Logs are at `%LOCALAPPDATA%\WinAirPlay\controller-usbip.log` and rotate at 1 MiB, retaining at most two files.
