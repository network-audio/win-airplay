$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $folder 'rotating-log.ps1')
$controller = Join-Path $folder 'win-airplay.exe'
$usbipPathFile = Join-Path $folder 'usbip-path.txt'
if (Test-Path -LiteralPath $usbipPathFile) {
    $env:WIN_AIRPLAY_USBIP_EXE = [System.IO.File]::ReadAllText($usbipPathFile).Trim()
}
$logFolder = Join-Path $env:LOCALAPPDATA 'WinAirPlay'
New-Item -ItemType Directory -Force $logFolder | Out-Null
$log = Join-Path $logFolder 'controller-usbip.log'

while ($true) {
    Write-RotatingLog -Path $log -Message "$(Get-Date -Format o) Starting Win AirPlay USB/IP"
    & $controller run-usbip 2>&1 | ForEach-Object { Write-RotatingLog -Path $log -Message ([string]$_) }
    Write-RotatingLog -Path $log -Message "$(Get-Date -Format o) Controller exited; restarting in five seconds"
    Start-Sleep -Seconds 5
}
