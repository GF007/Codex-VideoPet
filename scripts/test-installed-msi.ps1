[CmdletBinding()]
param(
    [string]$MsiPath,
    [switch]$KeepInstalled
)

$ErrorActionPreference = 'Stop'
function Get-Sha256Hex([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        return (($sha256.ComputeHash($stream) | ForEach-Object { $_.ToString('X2') }) -join '')
    }
    finally {
        $sha256.Dispose()
        $stream.Dispose()
    }
}

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$cargoText = Get-Content -LiteralPath (Join-Path $projectRoot 'native\Cargo.toml') -Raw
$versionMatch = [regex]::Match($cargoText, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
    throw 'Unable to read native package version.'
}
if ([string]::IsNullOrWhiteSpace($MsiPath)) {
    $MsiPath = Join-Path $projectRoot "dist-native\CodexPet-$($versionMatch.Groups[1].Value)-x64.msi"
}
$MsiPath = (Resolve-Path -LiteralPath $MsiPath).Path
$installedRoot = Join-Path $env:LOCALAPPDATA 'Programs\CodexPet'
if (Test-Path -LiteralPath $installedRoot) {
    throw "Refusing to overwrite an existing Codex Pet installation: $installedRoot"
}
if (Test-NetConnection 127.0.0.1 -Port 48193 -InformationLevel Quiet -WarningAction SilentlyContinue) {
    throw 'Port 48193 is already in use; stop the existing Codex Pet before installed MSI QA.'
}

$qaRoot = Join-Path (Join-Path $projectRoot 'artifacts') ("qa-installed-msi-" + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Path $qaRoot | Out-Null
$installLog = Join-Path $qaRoot 'msi-install.log'
$uninstallLog = Join-Path $qaRoot 'msi-uninstall.log'
$isolatedLocalAppData = Join-Path $qaRoot 'LocalAppData'
New-Item -ItemType Directory -Path $isolatedLocalAppData | Out-Null
$hooksPath = Join-Path $env:USERPROFILE '.codex\hooks.json'
$hooksHashBefore = if (Test-Path -LiteralPath $hooksPath -PathType Leaf) {
    Get-Sha256Hex $hooksPath
}
else {
    $null
}

$installed = $false
$runtimeStarted = $false
$report = $null
try {
    $install = Start-Process msiexec.exe -ArgumentList @(
        '/i', $MsiPath, '/qn', '/norestart', '/l*vx', $installLog
    ) -Wait -PassThru
    if ($install.ExitCode -notin @(0, 3010)) {
        throw "MSI install failed with exit code $($install.ExitCode)."
    }
    $installed = $true

    $exePath = Join-Path $installedRoot 'CodexPet.exe'
    $runtimePath = Join-Path $installedRoot 'libmpv-2.dll'
    $dependencyManifestPath = Join-Path $installedRoot 'DEPENDENCY_LICENSES.json'
    foreach ($required in @(
        $exePath,
        $runtimePath,
        $dependencyManifestPath,
        (Join-Path $installedRoot 'LICENSE.LGPL-2.1.txt'),
        (Join-Path $installedRoot 'LICENSE.LGPL-3.0.txt')
    )) {
        if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
            throw "Installed payload is missing: $required"
        }
    }
    $dependencyManifest = Get-Content -LiteralPath $dependencyManifestPath -Raw | ConvertFrom-Json
    if ((Get-Item -LiteralPath $runtimePath).Length -ne [int64]$dependencyManifest.runtime_bytes -or
        (Get-Sha256Hex $runtimePath) -ne [string]$dependencyManifest.runtime_sha256) {
        throw 'Installed libmpv does not match the dependency manifest.'
    }
    $licenseProbe = Start-Process -FilePath $exePath -ArgumentList '--validate-libmpv-license' `
        -WindowStyle Hidden -Wait -PassThru
    if ($licenseProbe.ExitCode -ne 0) {
        throw "Installed libmpv probe failed with exit code $($licenseProbe.ExitCode)."
    }

    $previousLocalAppData = $env:LOCALAPPDATA
    $previousVideoRoot = $env:CODEX_PET_VIDEO_DIR
    $env:LOCALAPPDATA = $isolatedLocalAppData
    Remove-Item Env:CODEX_PET_VIDEO_DIR -ErrorAction SilentlyContinue
    try {
        $null = Start-Process -FilePath $exePath -ArgumentList '--from-setup' -WindowStyle Hidden -PassThru
        $runtimeStarted = $true
    }
    finally {
        $env:LOCALAPPDATA = $previousLocalAppData
        if ($null -eq $previousVideoRoot) {
            Remove-Item Env:CODEX_PET_VIDEO_DIR -ErrorAction SilentlyContinue
        }
        else {
            $env:CODEX_PET_VIDEO_DIR = $previousVideoRoot
        }
    }

    $health = $null
    $state = $null
    $video = $null
    for ($attempt = 0; $attempt -lt 120; $attempt++) {
        try {
            $health = Invoke-RestMethod 'http://127.0.0.1:48193/health' -TimeoutSec 1
            $state = Invoke-RestMethod 'http://127.0.0.1:48193/v1/state' -TimeoutSec 2
            $video = Invoke-RestMethod 'http://127.0.0.1:48193/v1/video-config' -TimeoutSec 2
            if ($health.ok -and $state.playback.ready -and $video.playbackReady) {
                break
            }
        }
        catch {
            $health = $null
        }
        Start-Sleep -Milliseconds 250
    }
    if ($null -eq $health -or -not $health.ok -or -not $state.playback.ready -or -not $video.playbackReady) {
        throw 'Installed runtime did not reach healthy video playback within 30 seconds.'
    }
    if ($state.playback.error -or $video.playbackError) {
        throw "Installed playback reported an error: $($state.playback.error) $($video.playbackError)"
    }
    if ([string]$state.playback.mpv_configuration -notmatch '(?:^|\s)-Dgpl=false(?:\s|$)') {
        throw 'Installed runtime did not report the expected LGPL mpv configuration.'
    }

    $second = Start-Process -FilePath $exePath -ArgumentList '--from-setup' -WindowStyle Hidden -Wait -PassThru
    if ($second.ExitCode -ne 0) {
        throw "Second-instance probe failed with exit code $($second.ExitCode)."
    }
    Start-Sleep -Milliseconds 500
    $processCount = @(Get-Process -Name 'CodexPet' -ErrorAction SilentlyContinue | Where-Object {
        $_.Path -eq $exePath
    }).Count
    if ($processCount -ne 1) {
        throw "Single-instance validation found $processCount installed Pet processes."
    }

    $report = [ordered]@{
        accepted = $true
        version = $versionMatch.Groups[1].Value
        install_exit = $install.ExitCode
        license_probe_exit = $licenseProbe.ExitCode
        health_ok = [bool]$health.ok
        playback_ready = [bool]$state.playback.ready
        video_playback_ready = [bool]$video.playbackReady
        active_player_instances = [int]$state.playback.active_player_instances
        mpv_api = [string]$state.playback.mpv_api
        mpv_lgpl = $true
        single_instance_count = $processCount
        isolated_user_data = $true
        qa_evidence = $qaRoot
    }
}
finally {
    if ($runtimeStarted -and (Test-Path -LiteralPath (Join-Path $installedRoot 'CodexPet.exe'))) {
        $shutdown = Start-Process -FilePath (Join-Path $installedRoot 'CodexPet.exe') `
            -ArgumentList '--shutdown' -WindowStyle Hidden -Wait -PassThru
        if ($shutdown.ExitCode -ne 0) {
            throw "Installed runtime shutdown failed with exit code $($shutdown.ExitCode)."
        }
        for ($attempt = 0; $attempt -lt 40; $attempt++) {
            $remaining = @(Get-Process -Name 'CodexPet' -ErrorAction SilentlyContinue | Where-Object {
                $_.Path -eq (Join-Path $installedRoot 'CodexPet.exe')
            }).Count
            if ($remaining -eq 0) {
                break
            }
            Start-Sleep -Milliseconds 250
        }
        if ($remaining -ne 0) {
            throw 'Installed runtime did not exit before uninstall.'
        }
        if ($null -ne $report) {
            $report['shutdown_exit'] = $shutdown.ExitCode
        }
    }
    if ($installed -and -not $KeepInstalled) {
        $uninstall = Start-Process msiexec.exe -ArgumentList @(
            '/x', $MsiPath, '/qn', '/norestart', 'SKIPINTEGRATIONCLEANUP=1', '/l*vx', $uninstallLog
        ) -Wait -PassThru
        if ($uninstall.ExitCode -notin @(0, 3010)) {
            throw "MSI uninstall failed with exit code $($uninstall.ExitCode)."
        }
        if ($null -ne $report) {
            $report['uninstall_exit'] = $uninstall.ExitCode
            $report['install_removed'] = -not (Test-Path -LiteralPath $installedRoot)
        }
        if (Test-Path -LiteralPath $installedRoot) {
            throw 'MSI uninstall left the installed application directory behind.'
        }
    }
    $hooksHashAfter = if (Test-Path -LiteralPath $hooksPath -PathType Leaf) {
        Get-Sha256Hex $hooksPath
    }
    else {
        $null
    }
    if ($hooksHashBefore -ne $hooksHashAfter) {
        throw 'Protected installed MSI QA changed hooks.json.'
    }
    if ($null -ne $report) {
        $report['hooks_unchanged'] = $true
    }
}

[pscustomobject]$report
