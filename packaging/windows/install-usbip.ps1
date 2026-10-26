param([ValidateSet('Install', 'Uninstall')][string]$Action)

$ErrorActionPreference = 'Stop'
Start-Transcript -Path (Join-Path $env:TEMP 'win-airplay-usbip-setup.log') -Force | Out-Null
$taskName = 'Win AirPlay USBIP'
$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $folder 'find-usbip.ps1')

if ($Action -eq 'Uninstall') {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    exit 0
}

$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = [Security.Principal.WindowsPrincipal]::new($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw 'Administrator permission is required. Please approve the Windows elevation prompt when starting the installer.'
}

$usbip = Find-UsbipExecutable
if (-not $usbip) {
    $upstream = Join-Path $folder 'USBip-0.9.8.1-x64.exe'
    $expected = '38cad6d4432b52d5bb9409d9ad03b72fdffc4ada4cd3a48fbeca1a2752a8518a'
    $stream = [System.IO.File]::OpenRead($upstream)
    try {
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try { $actual = ([System.BitConverter]::ToString($sha256.ComputeHash($stream))).Replace('-', '').ToLowerInvariant() }
        finally { $sha256.Dispose() }
    } finally { $stream.Dispose() }
    if ($actual -ne $expected) { throw 'The bundled USBip installer checksum does not match.' }
    $programFiles64 = if ($env:ProgramW6432) { $env:ProgramW6432 } else { ${env:ProgramFiles} }
    $installDir = Join-Path $programFiles64 'USBip'
    $upstreamLog = Join-Path $env:TEMP 'win-airplay-usbip-install.log'
    $arguments = @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-', '/COMPONENTS=main,client', "/DIR=`"$installDir`"", "/LOG=`"$upstreamLog`"")
    $process = Start-Process -FilePath $upstream -ArgumentList $arguments -Wait -PassThru
    if ($process.ExitCode -notin @(0, 3010)) { throw "USBip installer exited with $($process.ExitCode)." }
    $usbip = Find-UsbipExecutable
    if (-not $usbip) {
        throw "USBip setup exited with code $($process.ExitCode), but usbip.exe was not found. Check $upstreamLog and restart Windows if setup requested it."
    }
}
[System.IO.File]::WriteAllText((Join-Path $folder 'usbip-path.txt'), $usbip)

$launcher = Join-Path $folder 'run-usbip.ps1'
$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$taskAction = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$launcher`""
$taskTrigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName $taskName -Action $taskAction -Trigger $taskTrigger -Principal $taskPrincipal -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
