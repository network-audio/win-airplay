param([ValidateSet('Install', 'Uninstall')][string]$Action)

$ErrorActionPreference = 'Stop'
$taskName = 'Win AirPlay'

if ($Action -eq 'Uninstall') {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    exit 0
}

$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
$driver = Join-Path $folder 'driver\ComponentizedAudioSample.inf'
$launcher = Join-Path $folder 'run.ps1'

if (-not (Test-Path (Join-Path $folder 'driver\sysvad.cat'))) {
    throw 'The test-signed driver catalog is missing.'
}

$boot = (& bcdedit.exe /enum '{current}' 2>&1) -join "`n"
if ($boot -notmatch '(?im)^testsigning\s+Yes\s*$') {
    throw 'Windows Test Mode must be enabled before installing this development driver.'
}

$certificate = Join-Path $folder 'driver\WinAirPlayTest.cer'
Import-Certificate -FilePath $certificate -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
Import-Certificate -FilePath $certificate -CertStoreLocation Cert:\LocalMachine\TrustedPublisher | Out-Null

$output = & pnputil.exe /add-driver $driver /install 2>&1
if ($LASTEXITCODE -ne 0) {
    throw "Driver installation failed: $($output -join [Environment]::NewLine)"
}

$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$taskAction = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$launcher`""
$taskTrigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName $taskName -Action $taskAction -Trigger $taskTrigger -Principal $taskPrincipal -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
