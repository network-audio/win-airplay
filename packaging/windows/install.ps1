param([ValidateSet('Install', 'Uninstall')][string]$Action)

$ErrorActionPreference = 'Stop'
$taskName = 'Win AirPlay'
$folder = Split-Path -Parent $MyInvocation.MyCommand.Path
$driverFolder = Join-Path $folder 'driver'

if ($Action -eq 'Uninstall') {
    Stop-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
    $publishedFile = Join-Path $driverFolder 'published-name.txt'
    if (Test-Path $publishedFile) {
        $published = (Get-Content $publishedFile -Raw).Trim()
        if ($published -match '^oem\d+\.inf$') {
            & pnputil.exe /delete-driver $published /uninstall /force | Out-Null
        }
    }
    $certificate = Join-Path $driverFolder 'WinAirPlayTest.cer'
    if (Test-Path $certificate) {
        $thumbprint = (Get-PfxCertificate -FilePath $certificate).Thumbprint
        Remove-Item "Cert:\LocalMachine\Root\$thumbprint" -ErrorAction SilentlyContinue
        Remove-Item "Cert:\LocalMachine\TrustedPublisher\$thumbprint" -ErrorAction SilentlyContinue
    }
    exit 0
}

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
$published = [regex]::Matches(($output -join "`n"), 'oem\d+\.inf', 'IgnoreCase') | Select-Object -Last 1
if ($published) {
    $published.Value | Set-Content (Join-Path $driverFolder 'published-name.txt')
}

$user = [Security.Principal.WindowsIdentity]::GetCurrent().Name
$taskAction = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$launcher`""
$taskTrigger = New-ScheduledTaskTrigger -AtLogOn -User $user
$taskPrincipal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName $taskName -Action $taskAction -Trigger $taskTrigger -Principal $taskPrincipal -Force | Out-Null
Start-ScheduledTask -TaskName $taskName
