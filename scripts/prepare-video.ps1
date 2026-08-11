[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$InputVideo,
    [string]$OutputVideo,
    [string]$FfmpegPath
)

$projectRoot = Split-Path -Parent $PSScriptRoot
$resolvedInput = (Resolve-Path -LiteralPath $InputVideo).Path
if ([string]::IsNullOrWhiteSpace($OutputVideo)) {
    $OutputVideo = Join-Path $projectRoot 'pet-placeholder-optimized.mp4'
}
$resolvedOutput = [IO.Path]::GetFullPath($OutputVideo)

if ([string]::IsNullOrWhiteSpace($FfmpegPath)) {
    $ffmpegCommand = Get-Command ffmpeg -ErrorAction SilentlyContinue
    $FfmpegPath = if ($ffmpegCommand) { $ffmpegCommand.Source } else { $null }
}

if ([string]::IsNullOrWhiteSpace($FfmpegPath) -or -not (Test-Path -LiteralPath $FfmpegPath -PathType Leaf)) {
    throw 'ffmpeg was not found. Install it on PATH or pass -FfmpegPath.'
}

& $FfmpegPath -hide_banner -loglevel warning -y -i $resolvedInput `
    -map '0:v:0' -an -vf 'scale=360:640:flags=lanczos,fps=24' `
    -c:v libx264 -preset slow -crf 23 -pix_fmt yuv420p -profile:v high `
    -movflags '+faststart' $resolvedOutput

if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $resolvedOutput)) {
    throw "Video optimization failed with exit code $LASTEXITCODE"
}

Get-Item -LiteralPath $resolvedOutput | Select-Object FullName, Length, LastWriteTime
