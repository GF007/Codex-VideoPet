[CmdletBinding()]
param(
    [switch]$SkipBuild,
    [switch]$WithoutVideos,
    [switch]$NoArchive
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$manifestPath = Join-Path $projectRoot 'native\Cargo.toml'
$releaseRoot = Join-Path $projectRoot 'native\target\release'
$exePath = Join-Path $releaseRoot 'codex-pet-native.exe'
$mpvPath = Join-Path $projectRoot 'native\third_party\mpv\libmpv-2.dll'
$noticePath = Join-Path $projectRoot 'native\third_party\mpv\THIRD_PARTY_NOTICES.md'
$licensePath = Join-Path $projectRoot 'native\third_party\mpv\LICENSE.GPL-2.0.txt'
$distRoot = Join-Path $projectRoot 'dist-native'
$packageRoot = Join-Path $distRoot 'CodexPet'
$paletteGenerator = Join-Path $projectRoot 'scripts\generate-ui-palette.mjs'
$palettePath = Join-Path $projectRoot 'VideoSource\V1\ui-palette.json'
$installationGuidePath = Join-Path $projectRoot 'docs\windows-installation.md'
$cargoText = Get-Content -LiteralPath $manifestPath -Raw
$versionMatch = [regex]::Match($cargoText, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
    throw "Unable to read the native package version from $manifestPath"
}
$packageVersion = $versionMatch.Groups[1].Value
$archivePath = Join-Path $distRoot "CodexPet-$packageVersion-win64.zip"

function Get-Sha256([string]$Path) {
    $stream = [IO.File]::OpenRead($Path)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha.ComputeHash($stream))).Replace('-', '')
    }
    finally {
        $sha.Dispose()
        $stream.Dispose()
    }
}

& node $paletteGenerator
if ($LASTEXITCODE -ne 0) {
    throw "UI palette generation failed with exit code $LASTEXITCODE."
}

if (-not $SkipBuild) {
    $previousBuildJobs = $env:CARGO_BUILD_JOBS
    $env:CARGO_BUILD_JOBS = '1'
    try {
        & cargo build --manifest-path $manifestPath --release
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
}

foreach ($required in @($exePath, $mpvPath, $noticePath, $licensePath, $installationGuidePath)) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Required package input is missing: $required"
    }
}

$resolvedProject = [IO.Path]::GetFullPath($projectRoot).TrimEnd('\')
$resolvedPackage = [IO.Path]::GetFullPath($packageRoot)
if (-not $resolvedPackage.StartsWith($resolvedProject + '\', [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to replace a package directory outside the project root: $resolvedPackage"
}

if (Test-Path -LiteralPath $packageRoot) {
    Remove-Item -LiteralPath $packageRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $packageRoot -Force | Out-Null

Copy-Item -LiteralPath $exePath -Destination (Join-Path $packageRoot 'CodexPet.exe')
Copy-Item -LiteralPath $mpvPath -Destination (Join-Path $packageRoot 'libmpv-2.dll')
Copy-Item -LiteralPath $noticePath -Destination (Join-Path $packageRoot 'THIRD_PARTY_NOTICES.md')
Copy-Item -LiteralPath $licensePath -Destination (Join-Path $packageRoot 'LICENSE.GPL-2.0.txt')
Copy-Item -LiteralPath (Join-Path $projectRoot 'README.md') -Destination (Join-Path $packageRoot 'README.md')
Copy-Item -LiteralPath $installationGuidePath -Destination (Join-Path $packageRoot 'INSTALLATION.md')

$hookSelfTest = Start-Process -FilePath $exePath -ArgumentList '--hook-self-test' -WindowStyle Hidden -Wait -PassThru
if ($hookSelfTest.ExitCode -ne 0) {
    throw "Native Hook self-test failed with exit code $($hookSelfTest.ExitCode)."
}

if (-not $WithoutVideos) {
    $videoSource = Join-Path $projectRoot 'VideoSource\V1'
    if (-not (Test-Path -LiteralPath $videoSource -PathType Container)) {
        throw "Video catalog is missing: $videoSource"
    }
    $videoDestination = Join-Path $packageRoot 'VideoSource\V1'
    New-Item -ItemType Directory -Path $videoDestination -Force | Out-Null
    $videoConfigPath = Join-Path $videoSource 'video-config.json'
    if (Test-Path -LiteralPath $videoConfigPath -PathType Leaf) {
        $previousVideoRoot = $env:CODEX_PET_VIDEO_DIR
        $env:CODEX_PET_VIDEO_DIR = $videoSource
        try {
            $validation = Start-Process -FilePath $exePath -ArgumentList '--validate-video-config' -WindowStyle Hidden -Wait -PassThru
            if ($validation.ExitCode -ne 0) {
                throw "Native video config validation failed with exit code $($validation.ExitCode): $videoConfigPath"
            }
        }
        finally {
            if ($null -eq $previousVideoRoot) {
                Remove-Item Env:CODEX_PET_VIDEO_DIR -ErrorAction SilentlyContinue
            }
            else {
                $env:CODEX_PET_VIDEO_DIR = $previousVideoRoot
            }
        }
        $videoConfig = Get-Content -LiteralPath $videoConfigPath -Raw | ConvertFrom-Json
        if ($videoConfig.schema_version -ne 1 -or $null -eq $videoConfig.states) {
            throw "Video config schema is invalid: $videoConfigPath"
        }
        Copy-Item -LiteralPath $videoConfigPath -Destination (Join-Path $videoDestination 'video-config.json')
        if (-not (Test-Path -LiteralPath $palettePath -PathType Leaf)) {
            throw "UI palette is missing: $palettePath"
        }
        Copy-Item -LiteralPath $palettePath -Destination (Join-Path $videoDestination 'ui-palette.json')
        $resolvedVideoSource = [IO.Path]::GetFullPath($videoSource).TrimEnd('\')
        $referenced = @(
            foreach ($state in $videoConfig.states.PSObject.Properties) {
                foreach ($clip in @($state.Value.clips)) {
                    if ($null -eq $clip -or [string]::IsNullOrWhiteSpace([string]$clip.file)) {
                        throw "Video config contains an empty file reference in state $($state.Name)."
                    }
                    $relative = ([string]$clip.file).Replace('/', '\')
                    if ([IO.Path]::IsPathRooted($relative)) {
                        throw "Video config contains an absolute file reference: $relative"
                    }
                    $sourcePath = [IO.Path]::GetFullPath((Join-Path $videoSource $relative))
                    if (-not $sourcePath.StartsWith($resolvedVideoSource + '\', [StringComparison]::OrdinalIgnoreCase)) {
                        throw "Video config file reference escapes VideoSource\V1: $relative"
                    }
                    if (-not (Test-Path -LiteralPath $sourcePath -PathType Leaf)) {
                        throw "Configured video is missing: $sourcePath"
                    }
                    [pscustomobject]@{ Relative = $relative; Source = $sourcePath }
                }
            }
        ) | Sort-Object Relative -Unique
        foreach ($video in $referenced) {
            $destinationPath = Join-Path $videoDestination $video.Relative
            $destinationDirectory = Split-Path -Parent $destinationPath
            New-Item -ItemType Directory -Path $destinationDirectory -Force | Out-Null
            Copy-Item -LiteralPath $video.Source -Destination $destinationPath
        }
    }
    else {
        throw "Packaging requires validated VideoSource\V1\video-config.json; legacy directory scans remain runtime-only."
    }
}

$files = Get-ChildItem -LiteralPath $packageRoot -Recurse -File | Sort-Object FullName | ForEach-Object {
    [ordered]@{
        path = $_.FullName.Substring($packageRoot.Length + 1).Replace('\', '/')
        bytes = $_.Length
        sha256 = Get-Sha256 $_.FullName
    }
}
$manifest = [ordered]@{
    package = 'CodexPet-native-win64'
    version = $packageVersion
    release_kind = 'portable-and-msi-payload'
    built_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
    files = @($files)
}
$manifestJson = $manifest | ConvertTo-Json -Depth 6
[IO.File]::WriteAllText(
    (Join-Path $packageRoot 'manifest.json'),
    $manifestJson,
    [Text.UTF8Encoding]::new($false)
)

if (-not $NoArchive) {
    $legacyArchive = Join-Path $distRoot 'CodexPet-native-win64.zip'
    if (Test-Path -LiteralPath $legacyArchive -PathType Leaf) {
        Remove-Item -LiteralPath $legacyArchive -Force
    }
    if (Test-Path -LiteralPath $archivePath) {
        Remove-Item -LiteralPath $archivePath -Force
    }
    Compress-Archive -LiteralPath $packageRoot -DestinationPath $archivePath -CompressionLevel Optimal
}

[pscustomobject]@{
    PackageRoot = $packageRoot
    Archive = if ($NoArchive) { $null } else { $archivePath }
    FileCount = (Get-ChildItem -LiteralPath $packageRoot -Recurse -File).Count
    IncludesVideos = -not $WithoutVideos
    Version = $packageVersion
}
