[CmdletBinding()]
param(
    [switch]$AllowUnsignedQa,
    [ValidatePattern('^$|^[0-9A-Fa-f]{40}$')]
    [string]$CertificateThumbprint = ''
)

$ErrorActionPreference = 'Stop'
if ($AllowUnsignedQa -and -not [string]::IsNullOrWhiteSpace($CertificateThumbprint)) {
    throw 'Choose either -AllowUnsignedQa or -CertificateThumbprint; the release tiers are mutually exclusive.'
}
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$cargoPath = Join-Path $projectRoot 'native\Cargo.toml'
$cargoText = Get-Content -LiteralPath $cargoPath -Raw
$versionMatch = [regex]::Match($cargoText, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
    throw 'Unable to read native package version.'
}
$version = $versionMatch.Groups[1].Value
$productCodeSeed = [Text.Encoding]::UTF8.GetBytes("codex-pet-msi-product:$version")
$sha256 = [Security.Cryptography.SHA256]::Create()
try {
    $productCodeBytes = $sha256.ComputeHash($productCodeSeed)[0..15]
}
finally {
    $sha256.Dispose()
}
$productCodeBytes[6] = ($productCodeBytes[6] -band 0x0f) -bor 0x50
$productCodeBytes[8] = ($productCodeBytes[8] -band 0x3f) -bor 0x80
$productCode = ([Guid]::new([byte[]]$productCodeBytes)).ToString().ToUpperInvariant()
$releaseExe = Join-Path $projectRoot 'native\target\release\codex-pet-native.exe'
$packageRoot = Join-Path $projectRoot 'dist-native\CodexPet'
$msiPath = Join-Path $projectRoot "dist-native\CodexPet-$version-x64.msi"

$previousBuildJobs = $env:CARGO_BUILD_JOBS
$env:CARGO_BUILD_JOBS = '1'
try {
    & cargo build --manifest-path $cargoPath --release
    if ($LASTEXITCODE -ne 0) {
        throw "Native release build failed with exit code $LASTEXITCODE."
    }
}
finally {
    if ($null -eq $previousBuildJobs) {
        Remove-Item Env:CARGO_BUILD_JOBS -ErrorAction SilentlyContinue
    }
    else {
        $env:CARGO_BUILD_JOBS = $previousBuildJobs
    }
}

if ([string]::IsNullOrWhiteSpace($CertificateThumbprint)) {
    if (-not $AllowUnsignedQa) {
        throw 'A code-signing certificate thumbprint is required for a release build. Use -AllowUnsignedQa only for internal QA.'
    }
}
else {
    & (Join-Path $PSScriptRoot 'sign-release.ps1') -Path $releaseExe -CertificateThumbprint $CertificateThumbprint
}

& (Join-Path $PSScriptRoot 'package-native.ps1') -SkipBuild

$manifest = Get-Content -LiteralPath (Join-Path $packageRoot 'manifest.json') -Raw | ConvertFrom-Json
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

& dotnet build (Join-Path $projectRoot 'installer\CodexPet.wixproj') -c Release `
    -p:NativePackageRoot="$packageRoot" -p:ProductVersion="$version" -p:ProductCode="$productCode"
if ($LASTEXITCODE -ne 0) {
    throw "WiX MSI build failed with exit code $LASTEXITCODE."
}
if (-not (Test-Path -LiteralPath $msiPath -PathType Leaf)) {
    throw "Expected MSI was not produced: $msiPath"
}
$wixPdb = [IO.Path]::ChangeExtension($msiPath, '.wixpdb')
if (Test-Path -LiteralPath $wixPdb -PathType Leaf) {
    $privateArtifactRoot = Join-Path $projectRoot "artifacts\release-$version"
    New-Item -ItemType Directory -Path $privateArtifactRoot -Force | Out-Null
    Move-Item -LiteralPath $wixPdb -Destination (Join-Path $privateArtifactRoot ([IO.Path]::GetFileName($wixPdb))) -Force
}

if (-not [string]::IsNullOrWhiteSpace($CertificateThumbprint)) {
    & (Join-Path $PSScriptRoot 'sign-release.ps1') -Path $msiPath -CertificateThumbprint $CertificateThumbprint
}

$verification = & (Join-Path $PSScriptRoot 'release-check.ps1') `
    -PackageRoot $packageRoot -MsiPath $msiPath -AllowUnsignedQa:$AllowUnsignedQa `
    -ExpectedCertificateThumbprint $CertificateThumbprint

[pscustomobject]@{
    Version = $version
    ProductCode = $productCode
    PackageRoot = $packageRoot
    Msi = $msiPath
    Signed = -not [string]::IsNullOrWhiteSpace($CertificateThumbprint)
    Verification = $verification
}
