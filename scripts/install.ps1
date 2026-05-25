$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo       = "daemon8ai/daemon8"
$Binary     = "daemon8"
$Version    = if ($env:DAEMON8_VERSION) { $env:DAEMON8_VERSION } else { "latest" }
$InstallDir = if ($env:DAEMON8_INSTALL_DIR) { $env:DAEMON8_INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\daemon8" }
$Target     = "x86_64-pc-windows-msvc"
$ArchiveName = "$Binary-$Target.zip"

function Resolve-Daemon8Version {
    if ($Version -ne "latest") {
        return $Version
    }

    $Headers = @{ "User-Agent" = "daemon8-installer" }

    try {
        $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers $Headers -UseBasicParsing
    } catch {
        $Releases = @(Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases?per_page=1" -Headers $Headers -UseBasicParsing)
        if ($Releases.Count -eq 0) {
            throw "Could not resolve the latest daemon8 release. Set DAEMON8_VERSION to a tag, for example: DAEMON8_VERSION=v0.5.0-alpha.2"
        }
        $Release = $Releases[0]
    }

    if (-not $Release.tag_name) {
        throw "Could not resolve the latest daemon8 release. Set DAEMON8_VERSION to a tag, for example: DAEMON8_VERSION=v0.5.0-alpha.2"
    }

    return $Release.tag_name
}

Write-Host ""
Write-Host "Daemon8 Installer" -ForegroundColor White
Write-Host ""

if ($env:DAEMON8_INSTALLER_SELF_TEST -eq "1") {
    Write-Host "  Self-test: no network, no install" -ForegroundColor DarkGray
    exit 0
}

$ResolvedVersion = Resolve-Daemon8Version
$Url = "https://github.com/$Repo/releases/download/$ResolvedVersion/$ArchiveName"
$ChecksumsUrl = "https://github.com/$Repo/releases/download/$ResolvedVersion/checksums.sha256"

Write-Host "[1/4] Download" -ForegroundColor Cyan
Write-Host "  Platform: $Target" -ForegroundColor DarkGray
Write-Host "  Version:  $ResolvedVersion" -ForegroundColor DarkGray
Write-Host "  Source:   $Url" -ForegroundColor DarkGray

$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) "daemon8-install-$([System.IO.Path]::GetRandomFileName())"
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
$Archive = Join-Path $Tmp $ArchiveName

try {
    Invoke-WebRequest -Uri $Url -OutFile $Archive -UseBasicParsing
} catch {
    Write-Host "  ! Download failed for $Target." -ForegroundColor Red
    Write-Host "  ! No prebuilt binary may exist for this platform." -ForegroundColor Red
    Write-Host "  ! Install from a checked-out source tree instead: cargo install --path crates/daemon" -ForegroundColor Red
    if ($Version -ne "latest") { Write-Host "  ! Version requested: $Version" -ForegroundColor Red }
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
    exit 1
}

Write-Host "  + Downloaded $ArchiveName" -ForegroundColor Green

Write-Host ""
Write-Host "[2/4] Verify" -ForegroundColor Cyan

$ChecksumsFile = Join-Path $Tmp "checksums.sha256"

try {
    Invoke-WebRequest -Uri $ChecksumsUrl -OutFile $ChecksumsFile -UseBasicParsing
    $ExpectedLine = Get-Content $ChecksumsFile | Where-Object {
        $Parts = $_ -split '\s+'
        $Parts.Length -ge 2 -and $Parts[1] -eq $ArchiveName
    } | Select-Object -First 1
    if ($ExpectedLine) {
        $Expected = ($ExpectedLine -split '\s+')[0]
        $Actual = (Get-FileHash -Path $Archive -Algorithm SHA256).Hash.ToLower()
        if ($Expected -eq $Actual) {
            Write-Host "  + SHA-256 verified" -ForegroundColor Green
        } else {
            Write-Host "  ! Checksum verification failed!" -ForegroundColor Red
            Write-Host "  ! Expected: $Expected" -ForegroundColor Red
            Write-Host "  ! Got:      $Actual" -ForegroundColor Red
            Write-Host "  ! The downloaded file may be corrupted. Aborting." -ForegroundColor Red
            Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
            exit 1
        }
    } else {
        Write-Host "  ! No checksum entry for $ArchiveName. Aborting." -ForegroundColor Red
        Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
        exit 1
    }
} catch {
    Write-Host "  ! Checksum file not available. Aborting." -ForegroundColor Red
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
    exit 1
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
Write-Host "[4/4] Service" -ForegroundColor Cyan
Write-Host ""
$Daemon8Exe = Join-Path $InstallDir "$Binary.exe"
& $Daemon8Exe service install
if ($LASTEXITCODE -ne 0) {
    Write-Host "  ! Service install failed. Try again with: $Daemon8Exe service install" -ForegroundColor Red
    exit $LASTEXITCODE
}
