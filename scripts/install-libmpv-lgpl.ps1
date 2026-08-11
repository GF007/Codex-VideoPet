[CmdletBinding()]
param(
    [string]$DestinationPath
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$manifestPath = Join-Path $projectRoot 'native\third_party\mpv\DEPENDENCY_LICENSES.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ([string]::IsNullOrWhiteSpace($DestinationPath)) {
    $DestinationPath = Join-Path $projectRoot 'native\third_party\mpv\libmpv-2.dll'
}
$destinationDirectory = Split-Path -Parent ([IO.Path]::GetFullPath($DestinationPath))
New-Item -ItemType Directory -Path $destinationDirectory -Force | Out-Null

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

$tempParent = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\')
$tempRoot = Join-Path $tempParent ("CodexPet-libmpv-" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $tempRoot | Out-Null
try {
    $archivePath = Join-Path $tempRoot ([string]$manifest.provenance.asset_name)
    Invoke-WebRequest -UseBasicParsing -Uri ([string]$manifest.provenance.asset_url) -OutFile $archivePath
    if ((Get-Sha256Hex $archivePath) -ne [string]$manifest.provenance.asset_sha256) {
        throw 'Downloaded libmpv archive SHA-256 does not match DEPENDENCY_LICENSES.json.'
    }
    $extractRoot = Join-Path $tempRoot 'extracted'
    New-Item -ItemType Directory -Path $extractRoot | Out-Null
    & tar.exe -xf $archivePath -C $extractRoot
    if ($LASTEXITCODE -ne 0) {
        throw "Unable to extract the pinned libmpv archive (tar exit $LASTEXITCODE)."
    }
    $runtime = Join-Path $extractRoot ([string]$manifest.runtime_file)
    if (-not (Test-Path -LiteralPath $runtime -PathType Leaf)) {
        throw 'The pinned archive does not contain libmpv-2.dll.'
    }
    if ((Get-Item -LiteralPath $runtime).Length -ne [int64]$manifest.runtime_bytes -or
        (Get-Sha256Hex $runtime) -ne [string]$manifest.runtime_sha256) {
        throw 'Extracted libmpv runtime does not match DEPENDENCY_LICENSES.json.'
    }

    $stagedPath = "$DestinationPath.tmp.$PID"
    Copy-Item -LiteralPath $runtime -Destination $stagedPath
    Move-Item -LiteralPath $stagedPath -Destination $DestinationPath -Force

    $arguments = @{
        ManifestPath = $manifestPath
        RuntimePath = $DestinationPath
        RequireRuntime = $true
    }
    & (Join-Path $PSScriptRoot 'test-libmpv-license.ps1') @arguments
}
finally {
    $resolvedTemp = [IO.Path]::GetFullPath($tempRoot)
    if ($resolvedTemp.StartsWith($tempParent + '\', [StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $resolvedTemp)) {
        Remove-Item -LiteralPath $resolvedTemp -Recurse -Force
    }
}
