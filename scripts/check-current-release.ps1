[CmdletBinding()]
param(
    [switch]$AllowUnsignedQa
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$cargoText = Get-Content -LiteralPath (Join-Path $projectRoot 'native\Cargo.toml') -Raw
$versionMatch = [regex]::Match($cargoText, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
    throw 'Unable to read native package version.'
}
$version = $versionMatch.Groups[1].Value
$arguments = @{
    PackageRoot = Join-Path $projectRoot 'dist-native\CodexPet'
    MsiPath = Join-Path $projectRoot "dist-native\CodexPet-$version-x64.msi"
    AllowUnsignedQa = $AllowUnsignedQa
}
if (-not $AllowUnsignedQa) {
    $arguments.ExpectedCertificateThumbprint = [string]$env:CODEX_PET_SIGNING_THUMBPRINT
}
& (Join-Path $PSScriptRoot 'release-check.ps1') @arguments
