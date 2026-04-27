$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo       = "daemon8ai/daemon8"
$Binary     = "daemon8"
$Version    = if ($env:DAEMON8_VERSION) { $env:DAEMON8_VERSION } else { "latest" }
$InstallDir = if ($env:DAEMON8_INSTALL_DIR) { $env:DAEMON8_INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\daemon8" }
$Target     = "x86_64-pc-windows-msvc"

if ($Version -eq "latest") {
    $Url = "https://github.com/$Repo/releases/latest/download/$Binary-$Target.zip"
} else {
    $Url = "https://github.com/$Repo/releases/download/$Version/$Binary-$Target.zip"
}

Write-Host ""
Write-Host "Daemon8 Installer" -ForegroundColor White
Write-Host ""

Write-Host "[1/3] Download" -ForegroundColor Cyan
Write-Host "  Platform: $Target" -ForegroundColor DarkGray
Write-Host "  Source:   $Url" -ForegroundColor DarkGray

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) "daemon8-install-$([System.IO.Path]::GetRandomFileName())"
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
$Archive = Join-Path $Tmp "$Binary.zip"

try {
    Invoke-WebRequest -Uri $Url -OutFile $Archive -UseBasicParsing
} catch {
    Write-Host "  ! Download failed. Check your internet connection and that a release exists." -ForegroundColor Red
    if ($Version -ne "latest") { Write-Host "  ! Version requested: $Version" -ForegroundColor Red }
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
    exit 1
}

Expand-Archive -Path $Archive -DestinationPath $Tmp -Force
Write-Host "  + Downloaded $Binary" -ForegroundColor Green

Write-Host ""
Write-Host "[2/3] Install" -ForegroundColor Cyan

if (Test-Path (Join-Path $InstallDir "$Binary.exe")) {
    Write-Host "  Updating existing installation" -ForegroundColor DarkGray
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item (Join-Path $Tmp "$Binary.exe") (Join-Path $InstallDir "$Binary.exe") -Force

$LicenseSrc = Join-Path $Tmp "LICENSE"
if (Test-Path $LicenseSrc) {
    Copy-Item $LicenseSrc (Join-Path $InstallDir "LICENSE-daemon8") -Force
}

$CurrentPath = [Environment]::GetEnvironmentVariable("PATH", "User")
if ($CurrentPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("PATH", "$CurrentPath;$InstallDir", "User")
    $env:PATH += ";$InstallDir"
    Write-Host "  + Added $InstallDir to PATH" -ForegroundColor Green
    Write-Host "  Restart your terminal for PATH changes to take effect" -ForegroundColor DarkGray
}

Write-Host "  + Installed to $InstallDir\$Binary.exe" -ForegroundColor Green

Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "[3/3] Setup" -ForegroundColor Cyan
Write-Host ""
& (Join-Path $InstallDir "$Binary.exe") setup
