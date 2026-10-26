$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $folder 'rotating-log.ps1')
$controller = Join-Path $folder 'win-airplay.exe'
$logFolder = Join-Path $env:LOCALAPPDATA 'WinAirPlay'
New-Item -ItemType Directory -Force $logFolder | Out-Null
$log = Join-Path $logFolder 'controller.log'

while ($true) {
    Write-RotatingLog -Path $log -Message "$(Get-Date -Format o) Starting Win AirPlay"
    & $controller run 2>&1 | ForEach-Object { Write-RotatingLog -Path $log -Message ([string]$_) }
    Write-RotatingLog -Path $log -Message "$(Get-Date -Format o) Controller exited; restarting in five seconds"
    Start-Sleep -Seconds 5
}
