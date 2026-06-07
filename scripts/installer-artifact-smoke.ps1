$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$RootDir = Resolve-Path (Join-Path $PSScriptRoot '..')
Set-Location $RootDir

function Fail {
    param([string] $Message)

    throw "installer-artifact-smoke: $Message"
}

$WorkspaceVersion = $null
$InWorkspacePackage = $false
foreach ($Line in Get-Content 'Cargo.toml') {
    if ($Line -match '^\[workspace\.package\]') {
        $InWorkspacePackage = $true
        continue
    }

    if ($InWorkspacePackage -and $Line -match '^\[') {
        break
    }

    if ($InWorkspacePackage -and $Line -match '^\s*version\s*=\s*"([^"]+)"') {
        $WorkspaceVersion = $Matches[1]
        break
    }
}

if (-not $WorkspaceVersion) {
    Fail 'workspace package version not found'
}

if (-not (Get-Command python -ErrorAction SilentlyContinue)) {
    Fail 'python is required to serve local artifacts'
}

$Target = 'x86_64-pc-windows-msvc'
$Tmp = Join-Path ([System.IO.Path]::GetTempPath()) "daemon8-artifact-smoke-$([System.IO.Path]::GetRandomFileName())"
$Artifacts = Join-Path $Tmp 'artifacts'
$InstallDir = Join-Path $Tmp 'install'
$PackageDir = Join-Path $Tmp 'package'
$Server = $null

try {
    New-Item -ItemType Directory -Force -Path $Artifacts, $InstallDir, $PackageDir | Out-Null

    cargo build --release --target $Target -p daemon8

    Copy-Item "target\$Target\release\daemon8.exe" $PackageDir -Force
    Copy-Item 'LICENSE' $PackageDir -Force

    $ArchiveName = "daemon8-$Target.zip"
    $ArchivePath = Join-Path $Artifacts $ArchiveName
    Compress-Archive -Path (Join-Path $PackageDir 'daemon8.exe'), (Join-Path $PackageDir 'LICENSE') -DestinationPath $ArchivePath -Force

    $Hash = (Get-FileHash -Path $ArchivePath -Algorithm SHA256).Hash.ToLower()
    Set-Content -Path (Join-Path $Artifacts 'checksums.sha256') -Value "$Hash  $ArchiveName"

    $Listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Parse('127.0.0.1'), 0)
    $Listener.Start()
    $Port = $Listener.LocalEndpoint.Port
    $Listener.Stop()

    $Server = Start-Process python -ArgumentList @('-m', 'http.server', "$Port", '--bind', '127.0.0.1', '--directory', $Artifacts) -PassThru -WindowStyle Hidden

    for ($i = 0; $i -lt 50; $i++) {
        try {
            Invoke-WebRequest -Uri "http://127.0.0.1:$Port/checksums.sha256" -UseBasicParsing | Out-Null
            break
        } catch {
            Start-Sleep -Milliseconds 100
        }
    }

    Invoke-WebRequest -Uri "http://127.0.0.1:$Port/checksums.sha256" -UseBasicParsing | Out-Null

    $env:DAEMON8_RELEASE_BASE_URL = "http://127.0.0.1:$Port"
    $env:DAEMON8_INSTALLER_SKIP_SERVICE = '1'
    $env:DAEMON8_INSTALL_DIR = $InstallDir
    ./scripts/install.ps1 | Out-Null

    $InstalledVersion = & (Join-Path $InstallDir 'daemon8.exe') --version
    if ($InstalledVersion -ne "daemon8 $WorkspaceVersion") {
        Fail "unexpected installed version: $InstalledVersion"
    }

    Write-Host "installer-artifact-smoke: ok ($WorkspaceVersion $Target)"
} finally {
    Remove-Item Env:\DAEMON8_RELEASE_BASE_URL -ErrorAction SilentlyContinue
    Remove-Item Env:\DAEMON8_INSTALLER_SKIP_SERVICE -ErrorAction SilentlyContinue
    Remove-Item Env:\DAEMON8_INSTALL_DIR -ErrorAction SilentlyContinue

    if ($Server) {
        Stop-Process -Id $Server.Id -ErrorAction SilentlyContinue
    }

    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}
