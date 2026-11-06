param([Parameter(Mandatory = $true)][string]$InstallDir)

$ErrorActionPreference = 'Stop'
$controller = [System.IO.Path]::GetFullPath((Join-Path $InstallDir 'win-airplay.exe'))
$launcher = [System.IO.Path]::GetFullPath((Join-Path $InstallDir 'run-usbip.ps1'))
$comparison = [System.StringComparison]::OrdinalIgnoreCase

Stop-ScheduledTask -TaskName 'Win AirPlay USBIP' -ErrorAction SilentlyContinue

# The scheduled task runs a PowerShell restart loop. Stop it before stopping
# the controller, or it can launch a new copy while the installer replaces it.
$processes = @(Get-CimInstance Win32_Process -Filter "Name = 'powershell.exe' OR Name = 'pwsh.exe' OR Name = 'win-airplay.exe'")
$launchers = @($processes | Where-Object {
    $_.Name -ne 'win-airplay.exe' -and
    $_.CommandLine -and
    $_.CommandLine.IndexOf($launcher, $comparison) -ge 0
})
foreach ($process in $launchers) {
    Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
}

$controllers = @($processes | Where-Object {
    $_.Name -eq 'win-airplay.exe' -and
    $_.ExecutablePath -and
    [System.IO.Path]::GetFullPath($_.ExecutablePath).Equals($controller, $comparison)
})
foreach ($process in $controllers) {
    Stop-Process -Id $process.ProcessId -Force -ErrorAction SilentlyContinue
}

foreach ($attempt in 1..20) {
    $remaining = @($controllers | Where-Object { Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue })
    if ($remaining.Count -eq 0) { exit 0 }
    Start-Sleep -Milliseconds 100
}
throw 'The previous Win AirPlay controller did not stop before upgrade.'
