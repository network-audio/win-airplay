param([ValidateSet('Install', 'Uninstall')][string]$Action)

$ErrorActionPreference = 'Stop'
$taskName = 'Win AirPlay USBIP'
$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
$usbip = Join-Path ${env:ProgramFiles} 'USBip\usbip.exe'

if ($Action -eq 'Uninstall') {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    exit 0
}

if (-not (Test-Path $usbip)) {
    $upstream = Join-Path $folder 'USBip-0.9.8.1-x64.exe'
    $expected = '38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a'
    $actual = (Get-FileHash -Algorithm SHA256 $upstream).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { throw 'The bundled USBip installer checksum does not match.' }
    $process = Start-Process -FilePath $upstream -ArgumentList '/VERYSILENT','/SUPPRESSMSGBOXES','/NORESTART','/SP-','/COMPONENTS=main,client' -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010)) { throw "USBip installer exited with $($process.ExitCode)." }
    if (-not (Test-Path $usbip)) { throw 'USBip did not install usbip.exe.' }
}

$launcher = Join-Path $folder 'run-usbip.ps1'
$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$taskAction = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$launcher`""
$taskTrigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName $taskName -Action $taskAction -Trigger $taskTrigger -Principal $taskPrincipal -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
