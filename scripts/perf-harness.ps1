[CmdletBinding()]
param(
    [int]$Iterations = 1000,
    [int]$SameStateEvents = 200,
    [int]$SwitchDelayMs = 8,
    [double]$LegacyPrivateMiB = 0,
    [string]$BaselinePath = '',
    [string]$ExecutablePath = '',
    [string]$OutputDirectory = '',
    [switch]$KeepRunning
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if ([string]::IsNullOrWhiteSpace($ExecutablePath)) {
    $ExecutablePath = Join-Path $projectRoot 'native\target\release\codex-pet-native.exe'
}
$exePath = [IO.Path]::GetFullPath($ExecutablePath)
$baseline = $null
if ($LegacyPrivateMiB -le 0) {
    if ([string]::IsNullOrWhiteSpace($BaselinePath)) {
        $BaselinePath = Join-Path $projectRoot 'artifacts\baseline-webview2-20260805.json'
    }
    if (-not (Test-Path -LiteralPath $BaselinePath -PathType Leaf)) {
        throw "Legacy process-tree baseline is missing: $BaselinePath"
    }
    $baseline = Get-Content -LiteralPath $BaselinePath -Raw | ConvertFrom-Json
    $LegacyPrivateMiB = [double]$baseline.process_tree_private_mib
}
$apiRoot = 'http://127.0.0.1:48193'
$releaseAfterMs = 1000

if ($Iterations -lt 20) { throw 'Iterations must be at least 20.' }
if ($SameStateEvents -lt 1) { throw 'SameStateEvents must be positive.' }
if (-not (Test-Path -LiteralPath $exePath -PathType Leaf)) {
    throw "Native release executable is missing: $exePath"
}
$targetProcessName = [IO.Path]::GetFileNameWithoutExtension($exePath)
if (Get-Process -Name $targetProcessName -ErrorAction SilentlyContinue) {
    throw "A $targetProcessName process is already running. Exit it before the isolated performance run."
}
if (Get-NetTCPConnection -LocalPort 48193 -State Listen -ErrorAction SilentlyContinue) {
    throw 'TCP port 48193 is already in use. Stop the existing CodexPet/API owner before the isolated performance run.'
}

if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $projectRoot ('artifacts\perf-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
}
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null

function Get-State {
    Invoke-RestMethod -Uri "$apiRoot/v1/state" -TimeoutSec 5
}

function Set-State([string]$State, [string]$Source) {
    $body = @{ state = $State; source = $Source } | ConvertTo-Json -Compress
    Invoke-RestMethod -Method Post -Uri "$apiRoot/v1/state" -ContentType 'application/json' -Body $body -TimeoutSec 5 | Out-Null
}

function Set-Visibility([bool]$Visible) {
    $body = @{ visible = $Visible } | ConvertTo-Json -Compress
    Invoke-RestMethod -Method Post -Uri "$apiRoot/v1/window" -ContentType 'application/json' -Body $body -TimeoutSec 5 | Out-Null
}

function Get-ProcessTree([int]$RootProcessId) {
    $rows = @(Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name)
    $ids = [Collections.Generic.HashSet[int]]::new()
    $null = $ids.Add($RootProcessId)
    do {
        $added = 0
        foreach ($row in $rows) {
            if ($ids.Contains([int]$row.ParentProcessId) -and $ids.Add([int]$row.ProcessId)) {
                $added++
            }
        }
    } while ($added -gt 0)
    $processes = foreach ($processIdValue in $ids) {
        Get-Process -Id $processIdValue -ErrorAction SilentlyContinue
    }
    return @($processes)
}

function Get-Metrics([int]$ProcessId, [string]$Phase, [int]$Sequence) {
    $process = Get-Process -Id $ProcessId -ErrorAction Stop
    $process.Refresh()
    $tree = @(Get-ProcessTree $ProcessId)
    $state = Get-State
    [pscustomobject][ordered]@{
        timestamp_utc = [DateTimeOffset]::UtcNow.ToString('o')
        phase = $Phase
        sequence = $Sequence
        pid = $ProcessId
        working_set_bytes = [int64]$process.WorkingSet64
        private_bytes = [int64]$process.PrivateMemorySize64
        process_tree_count = $tree.Count
        process_tree_working_set_bytes = [int64](($tree | Measure-Object WorkingSet64 -Sum).Sum)
        process_tree_private_bytes = [int64](($tree | Measure-Object PrivateMemorySize64 -Sum).Sum)
        process_tree_names = @($tree.ProcessName | Sort-Object -Unique)
        handles = [int]$process.HandleCount
        threads = [int]$process.Threads.Count
        cpu_seconds = [double]$process.CPU
        state = $state.state.state
        video = $state.videoPath
        ready = [bool]$state.playback.ready
        playing = [bool]$state.playback.playing
        source_switch_count = [int]$state.playback.source_switch_count
        decoder_start_count = [int]$state.playback.decoder_start_count
        crossfade_count = [int]$state.playback.crossfade_count
        crossfade_active = [bool]$state.playback.crossfade_active
        active_player_instances = [int]$state.playback.active_player_instances
        player_release_count = [int]$state.playback.player_release_count
        hwdec_current = $state.playback.hwdec_current
        ui_error = $state.uiError
        playback_error = $state.playback.error
    }
}

$previousRelease = $env:CODEX_PET_RELEASE_AFTER_MS
$env:CODEX_PET_RELEASE_AFTER_MS = $releaseAfterMs.ToString()
$process = $null
$samples = [Collections.Generic.List[object]]::new()
$result = $null

try {
    $process = Start-Process -FilePath $exePath -WorkingDirectory (Split-Path $exePath) -WindowStyle Hidden -PassThru
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 200
        if ($process.HasExited) { throw "CodexPet exited during startup with code $($process.ExitCode)." }
        try { $state = Get-State } catch { $state = $null }
    } until ($state -and $state.http.bound -or [DateTime]::UtcNow -ge $deadline)
    if (-not $state -or -not $state.http.bound) { throw 'CodexPet API did not become ready within 30 seconds.' }

    $states = @('idle', 'running', 'waiting', 'review', 'jumping', 'running-left', 'running-right', 'waving', 'failed')
    for ($index = 0; $index -lt 25; $index++) {
        Set-State $states[$index % $states.Count] 'perf-warmup'
        Start-Sleep -Milliseconds $SwitchDelayMs
    }
    Start-Sleep -Seconds 2
    $before = Get-Metrics $process.Id 'before-switches' 0
    $samples.Add($before)

    for ($index = 0; $index -lt $Iterations; $index++) {
        Set-State $states[$index % $states.Count] 'perf-switch'
        if ($SwitchDelayMs -gt 0) { Start-Sleep -Milliseconds $SwitchDelayMs }
        if (($index + 1) % 50 -eq 0) {
            $samples.Add((Get-Metrics $process.Id 'switches' ($index + 1)))
        }
    }
    Start-Sleep -Seconds 3
    $afterSwitches = Get-Metrics $process.Id 'after-switches' $Iterations
    $samples.Add($afterSwitches)

    Set-State 'running' 'same-state-anchor'
    Start-Sleep -Milliseconds 500
    $sameStart = Get-State
    for ($index = 0; $index -lt $SameStateEvents; $index++) {
        Set-State 'running' 'same-state-flood'
    }
    Start-Sleep -Milliseconds 500
    $sameEnd = Get-State

    Set-Visibility $false
    Start-Sleep -Milliseconds 250
    $hiddenPaused = Get-Metrics $process.Id 'hidden-paused' 0
    $samples.Add($hiddenPaused)
    Start-Sleep -Milliseconds ($releaseAfterMs + 750)
    $hiddenReleased = Get-Metrics $process.Id 'hidden-released' 0
    $samples.Add($hiddenReleased)
    Set-Visibility $true

    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    do {
        Start-Sleep -Milliseconds 200
        $shown = Get-Metrics $process.Id 'shown-recreated' 0
    } until (($shown.ready -and $shown.active_player_instances -eq 1) -or [DateTime]::UtcNow -ge $deadline)
    $samples.Add($shown)

    $privateGrowth = [int64]$afterSwitches.process_tree_private_bytes - [int64]$before.process_tree_private_bytes
    $growthLimit = [math]::Max(64MB, [int64]([double]$before.process_tree_private_bytes * 0.25))
    $legacyBytes = [int64]($LegacyPrivateMiB * 1MB)
    $reductionPercent = (1.0 - ([double]$shown.process_tree_private_bytes / $legacyBytes)) * 100.0
    $switchSamples = @($samples | Where-Object { $_.phase -eq 'switches' })
    $sampleCount = $switchSamples.Count
    $sumX = [double](($switchSamples | Measure-Object sequence -Sum).Sum)
    $sumY = [double](($switchSamples | Measure-Object process_tree_private_bytes -Sum).Sum)
    $sumXY = 0.0
    $sumXX = 0.0
    foreach ($sample in $switchSamples) {
        $sumXY += [double]$sample.sequence * [double]$sample.process_tree_private_bytes
        $sumXX += [double]$sample.sequence * [double]$sample.sequence
    }
    $denominator = ($sampleCount * $sumXX) - ($sumX * $sumX)
    $slopeBytesPerSwitch = if ($denominator -eq 0) { 0.0 } else { (($sampleCount * $sumXY) - ($sumX * $sumY)) / $denominator }
    $peakTreePrivate = [int64](($samples | Measure-Object process_tree_private_bytes -Maximum).Maximum)
    $gates = [ordered]@{
        memory_reduction_15_percent = $reductionPercent -ge 15.0
        bounded_switch_growth = $privateGrowth -le $growthLimit
        bounded_switch_slope = $slopeBytesPerSwitch -le (8MB / 100.0)
        bounded_resource_growth = (($afterSwitches.handles - $before.handles) -le 16 -and ($afterSwitches.threads - $before.threads) -le 4)
        same_state_did_not_reload = [int]$sameEnd.playback.source_switch_count -eq [int]$sameStart.playback.source_switch_count
        hardware_decode_active = ($afterSwitches.ready -and $afterSwitches.hwdec_current -match 'd3d11va|dxva2')
        bounded_dual_player_instances = (@($samples | Where-Object { $_.active_player_instances -gt 2 }).Count -eq 0)
        crossfade_completed = ([int]$afterSwitches.crossfade_count -gt [int]$before.crossfade_count -and -not [bool]$afterSwitches.crossfade_active)
        hidden_pauses = -not $hiddenPaused.playing
        long_hidden_releases = ($hiddenReleased.active_player_instances -eq 0 -and $hiddenReleased.player_release_count -ge 1)
        hidden_release_recovers_memory = [int64]$hiddenReleased.process_tree_private_bytes -le [int64]$before.process_tree_private_bytes
        show_recreates_player = ($shown.active_player_instances -eq 1 -and $shown.decoder_start_count -ge 2 -and $shown.ready)
        renderer_has_no_error = (@($samples | Where-Object { $_.ui_error -or $_.playback_error }).Count -eq 0)
    }
    $passed = @($gates.Values | Where-Object { -not $_ }).Count -eq 0
    $result = [ordered]@{
        passed = $passed
        generated_at_utc = [DateTimeOffset]::UtcNow.ToString('o')
        executable = $exePath
        baseline_source = if ($baseline) { [IO.Path]::GetFullPath($BaselinePath) } else { 'LegacyPrivateMiB parameter' }
        baseline_method = if ($baseline) { $baseline.method } else { 'Caller-supplied process-tree private-memory baseline.' }
        iterations = $Iterations
        same_state_events = $SameStateEvents
        legacy_private_mib = $LegacyPrivateMiB
        native_private_mib = [math]::Round(([double]$shown.process_tree_private_bytes / 1MB), 2)
        reduction_percent = [math]::Round($reductionPercent, 2)
        private_growth_mib = [math]::Round(([double]$privateGrowth / 1MB), 2)
        growth_limit_mib = [math]::Round(([double]$growthLimit / 1MB), 2)
        peak_private_mib = [math]::Round(([double]$peakTreePrivate / 1MB), 2)
        slope_mib_per_100_switches = [math]::Round(($slopeBytesPerSwitch * 100.0 / 1MB), 3)
        gates = $gates
        checkpoints = [ordered]@{
            before = $before
            after_switches = $afterSwitches
            same_state_start = [ordered]@{
                state = $sameStart.state.state
                source_switch_count = [int]$sameStart.playback.source_switch_count
            }
            same_state_end = [ordered]@{
                state = $sameEnd.state.state
                source_switch_count = [int]$sameEnd.playback.source_switch_count
            }
            hidden_paused = $hiddenPaused
            hidden_released = $hiddenReleased
            shown_recreated = $shown
        }
        verification_boundary = 'Automated switch/lifecycle run only; an 8-hour soak remains a separate long-duration acceptance gate.'
    }
    $result | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'result.json') -Encoding utf8
    $samples | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $OutputDirectory 'samples.json') -Encoding utf8
    [pscustomobject]$result
    if (-not $passed) { exit 1 }
}
finally {
    if (-not $KeepRunning -and $process -and -not $process.HasExited) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
    }
    if ($null -eq $previousRelease) {
        Remove-Item Env:CODEX_PET_RELEASE_AFTER_MS -ErrorAction SilentlyContinue
    } else {
        $env:CODEX_PET_RELEASE_AFTER_MS = $previousRelease
    }
}
