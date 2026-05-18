$ErrorActionPreference = 'Stop'

$CommandPath = $MyInvocation.MyCommand.Path
$LocalInstaller = if ($CommandPath) {
    Join-Path (Split-Path -Parent $CommandPath) "scripts/install.ps1"
} else {
    $null
}
$RemoteInstaller = if ($env:DAEMON8_INSTALLER_SCRIPT_URL) {
    $env:DAEMON8_INSTALLER_SCRIPT_URL
} else {
    "https://raw.githubusercontent.com/daemon8ai/daemon8/main/scripts/install.ps1"
}

if ($LocalInstaller -and (Test-Path $LocalInstaller)) {
    & $LocalInstaller @args
    exit $LASTEXITCODE
}

if ($env:DAEMON8_INSTALLER_SELF_TEST -eq "1") {
    Write-Host "installer fallback: $RemoteInstaller"
    exit 0
}

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) "daemon8-install-$([System.Guid]::NewGuid()).ps1"
try {
    Invoke-WebRequest -Uri $RemoteInstaller -OutFile $Tmp -UseBasicParsing
    & $Tmp @args
    exit $LASTEXITCODE
} finally {
    Remove-Item -Force $Tmp -ErrorAction SilentlyContinue
}
