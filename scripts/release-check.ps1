[CmdletBinding()]
param(
    [string]$PackageRoot,
    [string]$MsiPath,
    [ValidatePattern('^$|^[0-9A-Fa-f]{40}$')]
    [string]$ExpectedCertificateThumbprint = '',
    [switch]$AllowUnsignedQa
)

$ErrorActionPreference = 'Stop'
$previousModulePath = $env:PSModulePath
try {
    $env:PSModulePath = Join-Path $PSHOME 'Modules'
    Import-Module (Join-Path $PSHOME 'Modules\Microsoft.PowerShell.Security\Microsoft.PowerShell.Security.psd1') -Force -ErrorAction Stop
}
finally {
    $env:PSModulePath = $previousModulePath
}

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
if ([string]::IsNullOrWhiteSpace($PackageRoot)) {
    $PackageRoot = Join-Path $projectRoot 'dist-native\CodexPet'
}
$PackageRoot = (Resolve-Path -LiteralPath $PackageRoot).Path
$manifestPath = Join-Path $PackageRoot 'manifest.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json

$cargoText = Get-Content -LiteralPath (Join-Path $projectRoot 'native\Cargo.toml') -Raw
$versionMatch = [regex]::Match($cargoText, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success -or $manifest.version -ne $versionMatch.Groups[1].Value) {
    throw 'Package version does not match native/Cargo.toml.'
}
$packageJson = Get-Content -LiteralPath (Join-Path $projectRoot 'package.json') -Raw | ConvertFrom-Json
if ($packageJson.version -ne $manifest.version) {
    throw 'Package version does not match package.json.'
}

$manifestFiles = @{}
foreach ($entry in @($manifest.files)) {
    if ($manifestFiles.ContainsKey($entry.path)) {
        throw "Duplicate manifest path: $($entry.path)"
    }
    $manifestFiles[$entry.path] = $entry
    $path = Join-Path $PackageRoot ($entry.path.Replace('/', '\'))
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Manifest file is missing: $($entry.path)"
    }
    $file = Get-Item -LiteralPath $path
    if ($file.Length -ne [int64]$entry.bytes) {
        throw "Manifest byte count mismatch: $($entry.path)"
    }
    $hash = Get-Sha256Hex $path
    if ($hash -ne $entry.sha256) {
        throw "Manifest SHA-256 mismatch: $($entry.path)"
    }
}
$actual = @(Get-ChildItem -LiteralPath $PackageRoot -Recurse -File | ForEach-Object {
    $_.FullName.Substring($PackageRoot.Length + 1).Replace('\', '/')
})
$unexpected = @($actual | Where-Object { $_ -ne 'manifest.json' -and -not $manifestFiles.ContainsKey($_) })
if ($unexpected.Count -gt 0) {
    throw "Unlisted package files: $($unexpected -join ', ')"
}
if ($actual.Count -ne $manifestFiles.Count + 1) {
    throw 'Package file count does not equal manifest entries plus manifest.json.'
}

foreach ($required in @(
    'CodexPet.exe',
    'libmpv-2.dll',
    'README.md',
    'INSTALLATION.md',
    'THIRD_PARTY_NOTICES.md',
    'LICENSE.GPL-2.0.txt',
    'VideoSource/V1/video-config.json',
    'VideoSource/V1/ui-palette.json'
)) {
    if (-not (Test-Path -LiteralPath (Join-Path $PackageRoot $required) -PathType Leaf)) {
        throw "Required release file is missing: $required"
    }
}

[xml]$wix = Get-Content -LiteralPath (Join-Path $projectRoot 'installer\CodexPet.wxs') -Raw -Encoding UTF8
$namespace = New-Object System.Xml.XmlNamespaceManager($wix.NameTable)
$namespace.AddNamespace('w', 'http://wixtoolset.org/schemas/v4/wxs')
$wixPayload = @($wix.SelectNodes('//w:File', $namespace) | ForEach-Object {
    $source = [string]$_.Source
    $prefix = '$(var.NativePackageRoot)\'
    if (-not $source.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "WiX payload source is outside NativePackageRoot: $source"
    }
    $source.Substring($prefix.Length).Replace('\', '/')
}) | Sort-Object -Unique
$packagePayload = @(@($manifest.files).path + 'manifest.json') | Sort-Object -Unique
$missingFromMsi = @($packagePayload | Where-Object { $_ -notin $wixPayload })
$extraInMsi = @($wixPayload | Where-Object { $_ -notin $packagePayload })
if ($missingFromMsi.Count -gt 0 -or $extraInMsi.Count -gt 0) {
    throw "MSI payload declaration differs from the staged package. Missing=[$($missingFromMsi -join ', ')] Extra=[$($extraInMsi -join ', ')]"
}
$rootReadmeHash = Get-Sha256Hex (Join-Path $projectRoot 'README.md')
$packageReadmeHash = Get-Sha256Hex (Join-Path $PackageRoot 'README.md')
if ($rootReadmeHash -ne $packageReadmeHash) {
    throw 'The packaged README is stale.'
}

$hookSelfTest = Start-Process -FilePath (Join-Path $PackageRoot 'CodexPet.exe') -ArgumentList '--hook-self-test' -WindowStyle Hidden -Wait -PassThru
if ($hookSelfTest.ExitCode -ne 0) {
    throw "Packaged Hook self-test failed with exit code $($hookSelfTest.ExitCode)."
}

$signatureTargets = @((Join-Path $PackageRoot 'CodexPet.exe'))
if (-not [string]::IsNullOrWhiteSpace($MsiPath)) {
    $signatureTargets += (Resolve-Path -LiteralPath $MsiPath).Path
}
elseif (-not $AllowUnsignedQa) {
    throw 'A signed release check requires -MsiPath.'
}
if (-not $AllowUnsignedQa -and [string]::IsNullOrWhiteSpace($ExpectedCertificateThumbprint)) {
    throw 'A signed release check requires -ExpectedCertificateThumbprint.'
}
$expectedThumbprint = $ExpectedCertificateThumbprint.Replace(' ', '').ToUpperInvariant()
$signatureResults = foreach ($target in $signatureTargets) {
    $signature = Get-AuthenticodeSignature -LiteralPath $target
    if ($signature.Status -ne 'Valid' -and -not $AllowUnsignedQa) {
        throw "Release signature is not valid for ${target}: $($signature.Status)"
    }
    if ($signature.Status -eq 'Valid' -and -not [string]::IsNullOrWhiteSpace($expectedThumbprint)) {
        $actualThumbprint = [string]$signature.SignerCertificate.Thumbprint
        if ($actualThumbprint.Replace(' ', '').ToUpperInvariant() -ne $expectedThumbprint) {
            throw "Release signer thumbprint does not match the expected publisher for ${target}."
        }
        if ($null -eq $signature.TimeStamperCertificate) {
            throw "Release signature is missing an RFC 3161 timestamp for ${target}."
        }
    }
    [pscustomobject]@{
        Path = $target
        Status = [string]$signature.Status
        Signer = if ($null -eq $signature.SignerCertificate) { $null } else { $signature.SignerCertificate.Subject }
        SignerThumbprint = if ($null -eq $signature.SignerCertificate) { $null } else { $signature.SignerCertificate.Thumbprint }
        TimeStamper = if ($null -eq $signature.TimeStamperCertificate) { $null } else { $signature.TimeStamperCertificate.Subject }
    }
}

[pscustomobject]@{
    PackageRoot = $PackageRoot
    Version = $manifest.version
    ManifestEntries = $manifestFiles.Count
    ActualFiles = $actual.Count
    HookSelfTest = 'passed'
    ReleaseTier = if (@($signatureResults | Where-Object Status -ne 'Valid').Count -eq 0) { 'signed-release' } else { 'unsigned-qa' }
    Signatures = @($signatureResults)
}
