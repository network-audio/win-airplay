$ErrorActionPreference = 'Stop'
$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
$tone = Join-Path $folder 'test-tone.wav'
$log = Join-Path $env:LOCALAPPDATA 'WinAirPlay\controller.log'

Write-Host 'Select an AirPlay output in the Windows sound picker, then press Enter.'
Read-Host | Out-Null
Start-Sleep -Seconds 7

$player = New-Object System.Media.SoundPlayer $tone
for ($i = 0; $i -lt 3; $i++) {
    $player.PlaySync()
}
Start-Sleep -Seconds 2

$senders = @(Get-Process nyxr -ErrorAction SilentlyContinue)
Write-Host "Nyx sender processes: $($senders.Count)"
if (Test-Path $log) {
    Write-Host "Controller log: $log"
    Get-Content $log -Tail 30
} else {
    Write-Warning "Controller log not found at $log"
}
Write-Host 'The tone should have played on the selected receiver. Save the log when reporting a problem.'
