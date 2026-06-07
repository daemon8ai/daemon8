$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$Repo       = "daemon8ai/daemon8"
$Binary     = "daemon8"
$Version    = if ($env:DAEMON8_VERSION) { $env:DAEMON8_VERSION } else { "latest" }
$InstallDir = if ($env:DAEMON8_INSTALL_DIR) { $env:DAEMON8_INSTALL_DIR } else { "$env:LOCALAPPDATA\Programs\daemon8" }
$Target     = "x86_64-pc-windows-msvc"
$ArchiveName = "$Binary-$Target.zip"
$ReleaseBaseUrl = if ($env:DAEMON8_RELEASE_BASE_URL) { $env:DAEMON8_RELEASE_BASE_URL.TrimEnd("/") } else { "" }

function Install-Daemon8ScheduledTask {
    param([string] $Daemon8Exe)

    $TaskName = "daemon8-service"
    $UserId = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name
    $PowerShellExe = Join-Path $env:SystemRoot "System32\WindowsPowerShell\v1.0\powershell.exe"
    $EscapedDaemon8Exe = $Daemon8Exe.Replace("'", "''")
    $Command = "& '$EscapedDaemon8Exe' serve; exit `$LASTEXITCODE"
    $EncodedCommand = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($Command))
    $ActionArgs = "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -EncodedCommand $EncodedCommand"
    $Action = New-ScheduledTaskAction -Execute $PowerShellExe -Argument $ActionArgs
    $Trigger = New-ScheduledTaskTrigger -AtLogOn -User $UserId
    $Settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero) -RestartCount 10 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew
    $Principal = New-ScheduledTaskPrincipal -UserId $UserId -LogonType Interactive -RunLevel Limited

    foreach ($ExistingTask in @("Daemon8", "daemon8-user", "daemon8-service")) {
        try {
            Stop-ScheduledTask -TaskName $ExistingTask -ErrorAction SilentlyContinue
        } catch {
        }

        try {
            Get-ScheduledTask -TaskName $ExistingTask -ErrorAction SilentlyContinue | Unregister-ScheduledTask -Confirm:$false -ErrorAction Stop
        } catch {
        }
    }

    Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null
    Start-ScheduledTask -TaskName $TaskName
    Write-Host "  + Registered Task Scheduler task $TaskName" -ForegroundColor Green
}

function Wait-Daemon8Port {
    param([string] $Daemon8Exe)

    for ($i = 0; $i -lt 10; $i++) {
        $Client = $null
        try {
            $Client = [System.Net.Sockets.TcpClient]::new()
            $Connect = $Client.BeginConnect("127.0.0.1", 8888, $null, $null)
            if ($Connect.AsyncWaitHandle.WaitOne(500)) {
                $Client.EndConnect($Connect)
                Write-Host "  + Status: running on localhost:8888" -ForegroundColor Green
                return
            }
        } catch {
        } finally {
            if ($Client) { $Client.Close() }
        }

        Start-Sleep -Milliseconds 500
    }

    Write-Host "  ! Service task registered, but daemon8 did not answer on localhost:8888 yet." -ForegroundColor Yellow
    Write-Host "  ! Check status with: $Daemon8Exe status" -ForegroundColor Yellow
}

function Resolve-Daemon8Version {
    if ($ReleaseBaseUrl) {
        if ($Version -ne "latest") {
            return $Version
        }

        return "custom"
    }

    if ($Version -ne "latest") {
        return $Version
    }

    $Headers = @{ "User-Agent" = "daemon8-installer" }

    try {
        $Release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest" -Headers $Headers -UseBasicParsing
    } catch {
        $Releases = @(Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases?per_page=1" -Headers $Headers -UseBasicParsing)
        if ($Releases.Count -eq 0) {
            throw "Could not resolve the latest daemon8 release. Set DAEMON8_VERSION to a tag, for example: DAEMON8_VERSION=vX.Y.Z-alpha.N"
        }
        $Release = $Releases[0]
    }

    if (-not $Release.tag_name) {
        throw "Could not resolve the latest daemon8 release. Set DAEMON8_VERSION to a tag, for example: DAEMON8_VERSION=vX.Y.Z-alpha.N"
    }

    return $Release.tag_name
}

Write-Host ""
Write-Host "Daemon8 Installer" -ForegroundColor White
Write-Host ""

if ($env:DAEMON8_INSTALLER_SELF_TEST -eq "1") {
    Write-Host "  Self-test: no network, no install" -ForegroundColor DarkGray
    return
}

$ResolvedVersion = Resolve-Daemon8Version
if ($ReleaseBaseUrl) {
    $Url = "$ReleaseBaseUrl/$ArchiveName"
    $ChecksumsUrl = "$ReleaseBaseUrl/checksums.sha256"
} else {
    $Url = "https://github.com/$Repo/releases/download/$ResolvedVersion/$ArchiveName"
    $ChecksumsUrl = "https://github.com/$Repo/releases/download/$ResolvedVersion/checksums.sha256"
}

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
    throw "daemon8 installer failed during download"
}

Write-Host "  + Downloaded $ArchiveName" -ForegroundColor Green

Write-Host ""
Write-Host "[2/4] Verify" -ForegroundColor Cyan

$ChecksumsFile = Join-Path $Tmp "checksums.sha256"

try {
    Invoke-WebRequest -Uri $ChecksumsUrl -OutFile $ChecksumsFile -UseBasicParsing
} catch {
    Write-Host "  ! Checksum file not available. Aborting." -ForegroundColor Red
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
    throw "daemon8 installer failed checksum verification"
}

$ExpectedLine = Get-Content $ChecksumsFile | Where-Object {
    $Parts = $_ -split '\s+'
    $Parts.Length -ge 2 -and $Parts[1] -eq $ArchiveName
} | Select-Object -First 1

if (-not $ExpectedLine) {
    Write-Host "  ! No checksum entry for $ArchiveName. Aborting." -ForegroundColor Red
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
    throw "daemon8 installer failed checksum verification"
}

$Expected = ($ExpectedLine -split '\s+')[0]
$Actual = (Get-FileHash -Path $Archive -Algorithm SHA256).Hash.ToLower()
if ($Expected -ne $Actual) {
    Write-Host "  ! Checksum verification failed!" -ForegroundColor Red
    Write-Host "  ! Expected: $Expected" -ForegroundColor Red
    Write-Host "  ! Got:      $Actual" -ForegroundColor Red
    Write-Host "  ! The downloaded file may be corrupted. Aborting." -ForegroundColor Red
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
    throw "daemon8 installer failed checksum verification"
}

Write-Host "  + SHA-256 verified" -ForegroundColor Green

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
if ($env:DAEMON8_INSTALLER_SKIP_SERVICE -eq "1") {
    Write-Host "  + Service install skipped" -ForegroundColor Green
    return
}

$Daemon8Exe = Join-Path $InstallDir "$Binary.exe"
$ServiceInstallOutput = @(& $Daemon8Exe service install 2>&1)
if ($LASTEXITCODE -ne 0) {
    $ServiceText = ($ServiceInstallOutput | Out-String)
    if ($ServiceText -match "Access is denied") {
        Write-Host "  ! Built-in service registration was blocked by Windows Task Scheduler access control." -ForegroundColor Yellow
        Write-Host "  ! Registering daemon8 as a current-user startup task instead." -ForegroundColor Yellow
        try {
            Install-Daemon8ScheduledTask -Daemon8Exe $Daemon8Exe
            Wait-Daemon8Port -Daemon8Exe $Daemon8Exe
            return
        } catch {
            Write-Host "  ! Fallback service install failed: $_" -ForegroundColor Red
            Write-Host "  ! Browser control does not require Administrator." -ForegroundColor Red
            Write-Host "  ! Windows blocked daemon8 background startup registration for this user." -ForegroundColor Red
            Write-Host "  ! To run daemon8 now: $Daemon8Exe serve" -ForegroundColor Red
            Write-Host "  ! To install background startup, rerun this installer from PowerShell as Administrator or ask IT to allow current-user scheduled tasks." -ForegroundColor Red
            throw "daemon8 installer failed during service registration"
        }
    }

    if ($ServiceText.Trim()) {
        Write-Host $ServiceText.Trim()
    }
    Write-Host "  ! Service install failed." -ForegroundColor Red
    Write-Host "  ! To run daemon8 now: $Daemon8Exe serve" -ForegroundColor Red
    Write-Host "  ! Browser control does not require Administrator, but background startup registration may be blocked by Windows policy." -ForegroundColor Red
    throw "daemon8 installer failed during service registration"
}

$SuccessText = ($ServiceInstallOutput | Out-String).TrimEnd()
if ($SuccessText) {
    Write-Host $SuccessText
}
