[CmdletBinding()]
param(
    [string]$ExecutablePath,
    [int]$HookCount = 8
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if ([string]::IsNullOrWhiteSpace($ExecutablePath)) {
    $ExecutablePath = Join-Path $env:LOCALAPPDATA 'Programs\CodexPet\CodexPet.exe'
}
$ExecutablePath = (Resolve-Path -LiteralPath $ExecutablePath).Path
$artifactRoot = Join-Path $projectRoot 'artifacts\release-0.3.0-final'
New-Item -ItemType Directory -Path $artifactRoot -Force | Out-Null
$testRoot = Join-Path $artifactRoot ("lazy-profile-{0}-{1}" -f [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(), $PID)
$localAppData = Join-Path $testRoot 'LocalAppData'
$userProfile = Join-Path $testRoot 'UserProfile'
New-Item -ItemType Directory -Path $localAppData, $userProfile -Force | Out-Null

$raw = @{
    hook_event_name = 'UserPromptSubmit'
    session_id = '019fcc02-569f-7790-92ff-209aaca4275c'
    turn_id = 'turn-lazy'
    prompt = 'must-not-persist'
    cwd = 'must-not-persist'
} | ConvertTo-Json -Compress

$hooks = @()
1..$HookCount | ForEach-Object {
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $ExecutablePath
    $start.Arguments = '--hook --owner codex-pet-overlay'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardInput = $true
    $start.EnvironmentVariables['LOCALAPPDATA'] = $localAppData
    $start.EnvironmentVariables['USERPROFILE'] = $userProfile
    $process = [Diagnostics.Process]::Start($start)
    $process.StandardInput.Write($raw)
    $process.StandardInput.Close()
    $hooks += $process
}
foreach ($process in $hooks) {
    if (-not $process.WaitForExit(5000)) {
        throw 'Hook process exceeded the bounded exit time.'
    }
    if ($process.ExitCode -ne 0) {
        throw "Hook process failed with exit code $($process.ExitCode)."
    }
}

$state = $null
for ($attempt = 0; $attempt -lt 120; $attempt++) {
    try {
        $state = Invoke-RestMethod 'http://127.0.0.1:48193/v1/state' -TimeoutSec 1
        break
    }
    catch {
        Start-Sleep -Milliseconds 100
    }
}
if ($null -eq $state) {
    throw 'Lazy-start health check timed out.'
}

$events = Join-Path $localAppData 'com.local.codex-pet-overlay\session-data\events'
$pending = -1
for ($attempt = 0; $attempt -lt 100; $attempt++) {
    $pending = @(Get-ChildItem -LiteralPath $events -Filter '*.json' -File -ErrorAction SilentlyContinue).Count
    if ($pending -eq 0) {
        break
    }
    Start-Sleep -Milliseconds 100
}
$petProcesses = @(Get-CimInstance Win32_Process -Filter "Name='CodexPet.exe'" | Where-Object ExecutablePath -eq $ExecutablePath)
$sessionSnapshot = Join-Path $localAppData 'com.local.codex-pet-overlay\session-data\session-state.json'
$snapshotText = if (Test-Path -LiteralPath $sessionSnapshot) {
    Get-Content -LiteralPath $sessionSnapshot -Raw
}
else {
    ''
}
$observed = [int]$state.hookEventCount + [int]$state.recoveredEventCount
$result = [pscustomobject]@{
    TestRoot = $testRoot
    HookProcesses = $HookCount
    PetProcessCount = $petProcesses.Count
    HttpOk = [bool]$state.ok
    HookEventCount = $state.hookEventCount
    RecoveredEventCount = $state.recoveredEventCount
    ObservedTotal = $observed
    PendingJson = $pending
    SnapshotExists = Test-Path -LiteralPath $sessionSnapshot
    SensitivePromptPersisted = $snapshotText.Contains('must-not-persist')
}
$accepted = $petProcesses.Count -eq 1 -and $state.ok -and $observed -ge $HookCount -and $pending -eq 0 -and $result.SnapshotExists -and -not $result.SensitivePromptPersisted
$shutdown = Start-Process -FilePath $ExecutablePath -ArgumentList '--shutdown' -WindowStyle Hidden -Wait -PassThru
$result | Add-Member -NotePropertyName ShutdownExitCode -NotePropertyValue $shutdown.ExitCode
$result | Add-Member -NotePropertyName Accepted -NotePropertyValue ($accepted -and $shutdown.ExitCode -eq 0)
$evidencePath = Join-Path $testRoot 'result.json'
$json = $result | ConvertTo-Json -Compress
[IO.File]::WriteAllText($evidencePath, $json, [Text.UTF8Encoding]::new($false))
Write-Output $json
if (-not $accepted -or $shutdown.ExitCode -ne 0) {
    throw 'Lazy-start acceptance failed.'
}
