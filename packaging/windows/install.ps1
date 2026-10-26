param([ValidateSet('Install', 'Uninstall')][string]$Action)

$ErrorActionPreference = 'Stop'
$taskName = 'Win AirPlay'

if ($Action -eq 'Uninstall') {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    exit 0
}

$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
$driver = Join-Path $folder 'driver\ComponentizedAudioSample.inf'
$controller = Join-Path $folder 'win-airplay.exe'
$sender = Join-Path $folder 'nyxr.exe'

if (-not (Test-Path (Join-Path $folder 'driver\sysvad.cat'))) {
    throw 'A Microsoft-signed driver catalog is required before installation.'
}

$output = & pnputil.exe /add-driver $driver /install 2>&1
if ($LASTEXITCODE -ne 0) {
    throw "Driver installation failed: $($output -join [Environment]::NewLine)"
}

$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$taskAction = New-ScheduledTaskAction -Execute $controller -Argument "run --sender `"$sender`""
$taskTrigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName $taskName -Action $taskAction -Trigger $taskTrigger -Principal $taskPrincipal -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
