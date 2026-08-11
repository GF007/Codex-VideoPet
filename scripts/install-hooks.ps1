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
    $previousBuildJobs = $env:CARGO_BUILD_JOBS
    $env:CARGO_BUILD_JOBS = '1'
    try {
        & cargo build --manifest-path (Join-Path $projectRoot 'native\Cargo.toml') --release
        if ($LASTEXITCODE -ne 0) {
            throw "Native Hook installer build failed with exit code $LASTEXITCODE."
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

$process = Start-Process -FilePath $ExecutablePath -ArgumentList '--install-hooks' -WindowStyle Hidden -Wait -PassThru
if ($process.ExitCode -ne 0) {
    throw "Codex Pet Hook installation failed with exit code $($process.ExitCode)."
}

[pscustomobject]@{
    installed = $true
    executable = (Resolve-Path -LiteralPath $ExecutablePath).Path
    event_count = 11
    trust_required = $true
    restart_or_new_session_required = $true
} | ConvertTo-Json -Compress
