[CmdletBinding()]
param(
    [string]$ManifestPath,
    [string]$RuntimePath,
    [string]$ExecutablePath,
    [switch]$RequireRuntime
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if ([string]::IsNullOrWhiteSpace($ManifestPath)) {
    $ManifestPath = Join-Path $projectRoot 'native\third_party\mpv\DEPENDENCY_LICENSES.json'
}
$ManifestPath = (Resolve-Path -LiteralPath $ManifestPath).Path
$manifest = Get-Content -LiteralPath $ManifestPath -Raw | ConvertFrom-Json

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

if ($manifest.schema_version -ne 1 -or $manifest.component_id -ne 'codex-pet-libmpv-runtime') {
    throw 'Unsupported libmpv dependency manifest.'
}
if ($manifest.distribution_license -ne 'LGPL-3.0-or-later') {
    throw 'The libmpv runtime is not approved under the expected LGPL distribution license.'
}
if ($manifest.audit.status -ne 'artifact-configuration-verified' -or -not $manifest.audit.runtime_eligible) {
    throw 'The libmpv dependency manifest is not approved for local runtime or package QA.'
}
foreach ($required in @('-Dgpl=false', '-Dlibmpv=true', '-Dcplayer=false')) {
    if ($required -notin @($manifest.mpv.required_configuration)) {
        throw "The libmpv manifest is missing required configuration: $required"
    }
}
foreach ($forbidden in @('--enable-gpl', '--enable-nonfree', '--enable-libx264', '--enable-libx265', '--enable-librubberband')) {
    if ($forbidden -in @($manifest.ffmpeg.configure_args)) {
        throw "The FFmpeg manifest enables a forbidden option: $forbidden"
    }
}
foreach ($license in @($manifest.license_files)) {
    $licensePath = Join-Path (Split-Path -Parent $ManifestPath) ([string]$license)
    if (-not (Test-Path -LiteralPath $licensePath -PathType Leaf)) {
        throw "Required libmpv license file is missing: $licensePath"
    }
}
foreach ($value in @(
    [string]$manifest.mpv.source_url,
    [string]$manifest.ffmpeg.source_url,
    [string]$manifest.provenance.release_url,
    [string]$manifest.provenance.asset_url,
    [string]$manifest.provenance.build_workflow_run
)) {
    if (-not $value.StartsWith('https://', [StringComparison]::OrdinalIgnoreCase)) {
        throw "Dependency provenance must use HTTPS: $value"
    }
}

if ([string]::IsNullOrWhiteSpace($RuntimePath)) {
    $RuntimePath = Join-Path (Split-Path -Parent $ManifestPath) ([string]$manifest.runtime_file)
}
$runtimePresent = Test-Path -LiteralPath $RuntimePath -PathType Leaf
if ($RequireRuntime -and -not $runtimePresent) {
    throw "Required LGPL libmpv runtime is missing: $RuntimePath"
}
if ($runtimePresent) {
    $runtime = Get-Item -LiteralPath $RuntimePath
    if ($runtime.Length -ne [int64]$manifest.runtime_bytes) {
        throw 'libmpv runtime byte count does not match DEPENDENCY_LICENSES.json.'
    }
    $runtimeHash = Get-Sha256Hex $runtime.FullName
    if ($runtimeHash -ne [string]$manifest.runtime_sha256) {
        throw 'libmpv runtime SHA-256 does not match DEPENDENCY_LICENSES.json.'
    }
    if (-not [string]::IsNullOrWhiteSpace($ExecutablePath)) {
        if (-not (Test-Path -LiteralPath $ExecutablePath -PathType Leaf)) {
            throw "libmpv validator executable is missing: $ExecutablePath"
        }
        $previousRuntime = $env:CODEX_PET_MPV_DLL
        $env:CODEX_PET_MPV_DLL = $runtime.FullName
        try {
            $probe = Start-Process -FilePath $ExecutablePath -ArgumentList '--validate-libmpv-license' -WindowStyle Hidden -PassThru
            if (-not $probe.WaitForExit(30000)) {
                Stop-Process -Id $probe.Id -Force -ErrorAction SilentlyContinue
                throw 'libmpv runtime license probe timed out after 30 seconds.'
            }
            if ($probe.ExitCode -ne 0) {
                throw "libmpv runtime license probe failed with exit code $($probe.ExitCode)."
            }
        }
        finally {
            if ($null -eq $previousRuntime) {
                Remove-Item Env:CODEX_PET_MPV_DLL -ErrorAction SilentlyContinue
            }
            else {
                $env:CODEX_PET_MPV_DLL = $previousRuntime
            }
        }
    }
}

[pscustomobject]@{
    Manifest = $ManifestPath
    DistributionLicense = [string]$manifest.distribution_license
    AuditStatus = [string]$manifest.audit.status
    PublicRedistributionEligible = [bool]$manifest.audit.public_redistribution_eligible
    RuntimePresent = $runtimePresent
    RuntimeVerified = $runtimePresent
    SourceOnly = -not $runtimePresent
}
