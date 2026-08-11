[CmdletBinding()]
param(
    [string]$ExecutablePath
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if ([string]::IsNullOrWhiteSpace($ExecutablePath)) {
    $ExecutablePath = Join-Path $projectRoot 'native\target\release\codex-pet-native.exe'
}
if (-not (Test-Path -LiteralPath $ExecutablePath -PathType Leaf)) {
    throw "Codex Pet executable is missing: $ExecutablePath"
}
$process = Start-Process -FilePath $ExecutablePath -ArgumentList '--remove-hooks' -WindowStyle Hidden -Wait -PassThru
if ($process.ExitCode -ne 0) {
    throw "Codex Pet integration removal failed with exit code $($process.ExitCode)."
}
[pscustomobject]@{
    removed = $true
    owned_hooks_only = $true
    startup_removed = $true
    user_data_preserved = $true
} | ConvertTo-Json -Compress
