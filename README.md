# Win AirPlay

Win AirPlay adds discovered AirPlay 2 speakers to the standard Windows sound output picker. Select a speaker to stream audio to it; Windows volume, mute, and channel balance are passed through. The Rust controller links [Nyx Refrain](https://github.com/pStrikeZ/Nyx-Refrain) as a pinned submodule, so there is no separate sender to install. A SysVAD-based driver provides the Windows audio outputs.

## Install the preview

**Signing note:** The current driver is test-signed. Before installing, enable Windows Test Mode with `bcdedit /set testsigning on` in an elevated command prompt and restart. Secure Boot may block this setting. The installer does not change boot settings.

1. Download the [current Windows 11 preview installer](https://github.com/stakach/win-airplay/releases/tag/v0.1.0-alpha.4).
2. Run it as an administrator, then select an `AirPlay` output in the Windows sound picker.
3. To check playback, run `test-playback.ps1` from the install folder. Logs are at `%LOCALAPPDATA%\WinAirPlay\controller.log`.

Windows playback and disconnect behavior still need testing on a real Windows machine.

## Build

```powershell
git clone --recurse-submodules https://github.com/stakach/win-airplay.git
cd win-airplay
cargo test --locked
cargo build --release --locked
```

GitHub Actions builds the Windows driver and installer. The driver includes SysVAD code under the [Microsoft Public License](driver/MICROSOFT-PUBLIC-LICENSE.txt); the linked Nyx crates are MIT licensed.
