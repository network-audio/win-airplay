Win AirPlay adds discovered AirPlay 2 speakers to the Windows sound output picker. This update lets other applications use the USBip driver at the same time, such as [Inferno](https://github.com/network-audio/inferno)'s Windows sound cards for Dante networks: Win AirPlay serves its virtual speakers on a free local port and attaches and detaches only its own devices. The USB/IP server is now the shared [usbip-audio](https://github.com/network-audio/usbip-audio) crate. Uninstalling leaves the USBip driver installed for the other applications.

Download the installer for your Windows architecture (x64 or ARM64) below, run it, and approve the administrator prompt. The installer bundles the Win AirPlay controller and USBip client. It starts the controller at sign-in; no separate Nyx installation is needed. Restart Windows if prompted.

The controller runs in the background. Logs are at `%LOCALAPPDATA%\WinAirPlay\controller-usbip.log` and rotate at 1 MiB, retaining at most two files.
