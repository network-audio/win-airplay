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
$restartDelaySeconds = 5

while ($true) {
    Write-RotatingLog -Path $log -Message "$(Get-Date -Format o) Starting Win AirPlay USB/IP"
    $startedAt = Get-Date
    & $controller run-usbip 2>&1 | ForEach-Object {
        Write-RotatingLog -Path $log -Message "$(Get-Date -Format o) $([string]$_)"
    }
    if (((Get-Date) - $startedAt).TotalMinutes -ge 2) {
        $restartDelaySeconds = 5
    }
    Write-RotatingLog -Path $log -Message "$(Get-Date -Format o) Controller exited; restarting in $restartDelaySeconds seconds"
    Start-Sleep -Seconds $restartDelaySeconds
    $restartDelaySeconds = [Math]::Min($restartDelaySeconds * 2, 60)
}
