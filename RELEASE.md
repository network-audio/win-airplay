Win AirPlay 1.2.0 adds an AirPlay 2 input to Windows. Select **Win AirPlay Input** as an AirPlay destination on a phone or Mac, then choose its microphone in a Windows app. The input appears unplugged until an AirPlay session starts and returns to unplugged when it ends. The receiver handles buffered AAC at 44.1 kHz stereo, with sender volume applied to captured audio.

Existing AirPlay speaker outputs continue to work. The x64 and ARM64 installers include the controller and USBip client; no separate AirPlay receiver or Nyx installation is needed. GitHub Actions verified speaker playback, microphone state changes, WASAPI capture, and installer setup on both architectures.

Download the installer for your Windows architecture below, run it, and approve the administrator prompt. Restart Windows if prompted. Logs are at `%LOCALAPPDATA%\WinAirPlay\controller-usbip.log` and rotate at 1 MiB, retaining at most two files.
