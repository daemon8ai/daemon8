$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo       = "daemon8ai/daemon8"
$Binary     = "daemon8"
$Version    = if ($env:DAEMON8_VERSION) { $env:DAEMON8_VERSION } else { "latest" }
$InstallDir = if ($env:DAEMON8_INSTALL_DIR) { $env:DAEMON8_INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\daemon8" }
$Target     = "x86_64-pc-windows-msvc"
$ArchiveName = "$Binary-$Target.zip"

if ($Version -eq "latest") {
    $Url = "https://github.com/$Repo/releases/latest/download/$ArchiveName"
    $ChecksumsUrl = "https://github.com/$Repo/releases/latest/download/checksums.sha256"
} else {
    $Url = "https://github.com/$Repo/releases/download/$Version/$ArchiveName"
    $ChecksumsUrl = "https://github.com/$Repo/releases/download/$Version/checksums.sha256"
}

Write-Host ""
Write-Host "Daemon8 Installer" -ForegroundColor White
Write-Host ""

Write-Host "[1/4] Download" -ForegroundColor Cyan
Write-Host "  Platform: $Target" -ForegroundColor DarkGray
Write-Host "  Source:   $Url" -ForegroundColor DarkGray

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) "daemon8-install-$([System.IO.Path]::GetRandomFileName())"
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
$Archive = Join-Path $Tmp $ArchiveName

try {
    Invoke-WebRequest -Uri $Url -OutFile $Archive -UseBasicParsing
} catch {
    Write-Host "  ! Download failed for $Target." -ForegroundColor Red
    Write-Host "  ! No prebuilt binary may exist for this platform." -ForegroundColor Red
    Write-Host "  ! Install from source instead: cargo install daemon8" -ForegroundColor Red
    if ($Version -ne "latest") { Write-Host "  ! Version requested: $Version" -ForegroundColor Red }
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
    exit 1
}

Write-Host "  + Downloaded $ArchiveName" -ForegroundColor Green

Write-Host ""
Write-Host "[2/4] Verify" -ForegroundColor Cyan

$ChecksumsFile = Join-Path $Tmp "checksums.sha256"
$Verified = $false

try {
    Invoke-WebRequest -Uri $ChecksumsUrl -OutFile $ChecksumsFile -UseBasicParsing
    $ExpectedLine = Get-Content $ChecksumsFile | Where-Object { $_ -match $ArchiveName }
    if ($ExpectedLine) {
        $Expected = ($ExpectedLine -split '\s+')[0]
        $Actual = (Get-FileHash -Path $Archive -Algorithm SHA256).Hash.ToLower()
        if ($Expected -eq $Actual) {
            Write-Host "  + SHA-256 verified" -ForegroundColor Green
            $Verified = $true
        } else {
            Write-Host "  ! Checksum verification failed!" -ForegroundColor Red
            Write-Host "  ! Expected: $Expected" -ForegroundColor Red
            Write-Host "  ! Got:      $Actual" -ForegroundColor Red
            Write-Host "  ! The downloaded file may be corrupted. Aborting." -ForegroundColor Red
            Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
            exit 1
        }
    } else {
        Write-Host "  No checksum entry for $ArchiveName; skipping verification" -ForegroundColor DarkGray
    }
} catch {
    Write-Host "  Checksum file not available; skipping verification" -ForegroundColor DarkGray
}

Expand-Archive -Path $Archive -DestinationPath $Tmp -Force

Write-Host ""
Write-Host "[3/4] Install" -ForegroundColor Cyan

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
Write-Host "[4/4] Setup" -ForegroundColor Cyan
Write-Host ""
& (Join-Path $InstallDir "$Binary.exe") setup
