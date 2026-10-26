$ErrorActionPreference = 'Stop'
$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
$tone = Join-Path $folder 'test-tone.wav'
$log = Join-Path $env:LOCALAPPDATA 'WinAirPlay\controller.log'

Write-Host 'Select an AirPlay output in the Windows sound picker, then press Enter.'
Read-Host | Out-Null
Start-Sleep -Seconds 7

$player = New-Object System.Media.SoundPlayer $tone
Write-Host 'The tone will repeat for about 18 seconds. Adjust Windows volume, mute, and channel balance while it plays.'
for ($i = 0; $i -lt 6; $i++) {
    $player.PlaySync()
}
Start-Sleep -Seconds 2

$controllers = @(Get-Process win-airplay -ErrorAction SilentlyContinue)
Write-Host "Win AirPlay controller processes: $($controllers.Count)"
if (Test-Path $log) {
    Write-Host "Controller log: $log"
    Get-Content $log -Tail 30
} else {
    Write-Warning "Controller log not found at $log"
}
Write-Host 'The tone should have played on the selected receiver and followed Windows volume and mute. Save the log when reporting a problem.'
