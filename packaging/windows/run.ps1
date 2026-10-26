$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
$controller = Join-Path $folder 'win-airplay.exe'
$logFolder = Join-Path $env:LOCALAPPDATA 'WinAirPlay'
New-Item -ItemType Directory -Force $logFolder | Out-Null
$log = Join-Path $logFolder 'controller.log'

while ($true) {
    "$(Get-Date -Format o) Starting Win AirPlay" | Out-File -FilePath $log -Append
    & $controller run *>> $log
    "$(Get-Date -Format o) Controller exited; restarting in five seconds" | Out-File -FilePath $log -Append
    Start-Sleep -Seconds 5
}
