# Win AirPlay development preview

This installer includes a self-signed test driver, Rust controller, and Nyx Refrain sender. It is for a disposable Windows 11 test machine running Test Mode. Windows endpoint enumeration and audible playback have not yet been verified.

Before installing, [enable Windows Test Mode](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option) from an elevated command prompt and restart. Secure Boot may need to be disabled in firmware. The installer does not change boot settings. The driver targets Windows 11 22H2 or later. Windows may display a Test Mode watermark.

The installer registers an elevated interactive logon task for the installing user. Logs are written to `%LOCALAPPDATA%\WinAirPlay\controller.log`. Uninstall from Windows Apps after testing. Restore the original boot settings when finished.

For a playback check, select an AirPlay output in the Windows sound picker and run `test-playback.ps1` from the installed folder in PowerShell. It plays a short tone through the selected Windows output and prints the recent controller log. The check still needs a person to confirm whether the receiver played the tone.

This is an engineering preview, not the first stable release. Report which speakers appear in the output picker, whether selecting one routes playback, and any installation errors with the controller log and `%windir%\inf\setupapi.dev.log`.
