[CmdletBinding()]
param(
    [string]$ExecutablePath = ''
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if ([string]::IsNullOrWhiteSpace($ExecutablePath)) {
    $ExecutablePath = Join-Path $projectRoot 'native\target\release\codex-pet-native.exe'
}
$ExecutablePath = (Resolve-Path -LiteralPath $ExecutablePath).Path
$videoRoot = (Resolve-Path -LiteralPath (Join-Path $projectRoot 'VideoSource\V1')).Path
$testRoot = Join-Path $projectRoot (
    'artifacts\size-preset-{0}-{1}' -f [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(), $PID
)
$localAppData = Join-Path $testRoot 'LocalAppData'
New-Item -ItemType Directory -Path $localAppData -Force | Out-Null

if (-not ('CodexPetSizeProbe' -as [type])) {
    Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class CodexPetSizeProbe
{
    [StructLayout(LayoutKind.Sequential)]
    public struct Rect
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindow(string className, string windowName);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool GetWindowRect(IntPtr window, out Rect rect);

    [DllImport("user32.dll")]
    public static extern uint GetDpiForWindow(IntPtr window);

    [DllImport("user32.dll")]
    public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);

    [DllImport("user32.dll", SetLastError = true)]
    public static extern bool PostMessage(IntPtr window, uint message, UIntPtr wParam, IntPtr lParam);
}
'@
}
[CodexPetSizeProbe]::SetThreadDpiAwarenessContext([IntPtr]::new(-4)) | Out-Null

function Wait-ForHealth {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $deadline) {
        try {
            $health = Invoke-RestMethod 'http://127.0.0.1:48193/health' -TimeoutSec 1
            if ($health.ok) { return }
        }
        catch {}
        Start-Sleep -Milliseconds 200
    }
    throw 'Codex Pet health endpoint did not become ready.'
}

function Wait-ForPlayer {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $deadline) {
        $state = Invoke-RestMethod 'http://127.0.0.1:48193/v1/state' -TimeoutSec 2
        if ($state.playback.ready -and $state.playback.active_player_instances -ge 1) {
            return $state
        }
        Start-Sleep -Milliseconds 200
    }
    throw 'Codex Pet player did not become ready.'
}

function Read-VideoRect([IntPtr]$Window) {
    $rect = [CodexPetSizeProbe+Rect]::new()
    if (-not [CodexPetSizeProbe]::GetWindowRect($Window, [ref]$rect)) {
        throw 'Unable to read the Codex Pet video window rectangle.'
    }
    $dpi = [CodexPetSizeProbe]::GetDpiForWindow($Window)
    if ($dpi -eq 0) { $dpi = 96 }
    [pscustomobject]@{
        Width = $rect.Right - $rect.Left
        Height = $rect.Bottom - $rect.Top
        Dpi = $dpi
    }
}

function Wait-ForVideoWindow {
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        $window = [CodexPetSizeProbe]::FindWindow('CodexPetNativeVideo', 'Codex Pet Overlay')
        if ($window -ne [IntPtr]::Zero) { return $window }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)
    throw 'Codex Pet video window was not found.'
}

function Set-And-VerifyPreset(
    [IntPtr]$Window,
    [int]$Command,
    [string]$Name,
    [int]$Width,
    [int]$Height
) {
    if (-not [CodexPetSizeProbe]::PostMessage($Window, 0x0111, [UIntPtr]::new($Command), [IntPtr]::Zero)) {
        throw "Unable to dispatch the $Name size command."
    }
    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        Start-Sleep -Milliseconds 100
        $rect = Read-VideoRect $Window
        if ($rect.Width -eq $Width -and $rect.Height -eq $Height) { break }
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($rect.Width -ne $Width -or $rect.Height -ne $Height) {
        throw "$Name size readback was $($rect.Width)x$($rect.Height), expected ${Width}x${Height}."
    }
    $state = Wait-ForPlayer
    if ($null -ne $state.playback.error -or $null -ne $state.uiError) {
        throw "$Name size reported a player or UI error."
    }
    $settingsPath = Join-Path $localAppData 'com.local.codex-pet-overlay\settings.json'
    $settings = Get-Content -LiteralPath $settingsPath -Raw | ConvertFrom-Json
    if ($settings.window_size_preset -ne $Name) {
        throw "$Name size was not persisted."
    }
    [pscustomobject]@{
        Preset = $Name
        Width = $rect.Width
        Height = $rect.Height
        Ready = [bool]$state.playback.ready
        ActivePlayers = [int]$state.playback.active_player_instances
    }
}

$process = $null
try {
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $ExecutablePath
    $start.Arguments = '--login-start'
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.EnvironmentVariables['LOCALAPPDATA'] = $localAppData
    $start.EnvironmentVariables['CODEX_PET_VIDEO_DIR'] = $videoRoot
    $process = [Diagnostics.Process]::Start($start)
    Wait-ForHealth
    Invoke-RestMethod 'http://127.0.0.1:48193/v1/window' -Method Post -ContentType 'application/json' -Body '{"visible":true}' -TimeoutSec 2 | Out-Null
    $initialState = Wait-ForPlayer
    $window = Wait-ForVideoWindow
    $initial = Read-VideoRect $window
    if ($initial.Width -ne 360 -or $initial.Height -ne 640) {
        throw "Default size readback was $($initial.Width)x$($initial.Height), expected 360x640."
    }
    $small = Set-And-VerifyPreset $window 1005 'small' 288 512
    $medium = Set-And-VerifyPreset $window 1006 'medium' 360 640
    $large = Set-And-VerifyPreset $window 1007 'large' 450 800
    $samples = @($small, $medium, $large)

    $shutdown = Start-Process -FilePath $ExecutablePath -ArgumentList '--shutdown' -WindowStyle Hidden -Wait -PassThru
    if ($shutdown.ExitCode -ne 0 -or -not $process.WaitForExit(10000)) {
        throw 'Codex Pet did not shut down cleanly before the persistence readback.'
    }
    $process = [Diagnostics.Process]::Start($start)
    Wait-ForHealth
    Invoke-RestMethod 'http://127.0.0.1:48193/v1/window' -Method Post -ContentType 'application/json' -Body '{"visible":true}' -TimeoutSec 2 | Out-Null
    $restartState = Wait-ForPlayer
    $restartWindow = Wait-ForVideoWindow
    $restartRect = Read-VideoRect $restartWindow
    if ($restartRect.Width -ne 450 -or $restartRect.Height -ne 800) {
        throw "Restart size readback was $($restartRect.Width)x$($restartRect.Height), expected 450x800."
    }
    if ($null -ne $restartState.playback.error -or $null -ne $restartState.uiError) {
        throw 'Restarted large size reported a player or UI error.'
    }
    $cargoText = Get-Content -LiteralPath (Join-Path $projectRoot 'native\Cargo.toml') -Raw
    $version = [regex]::Match($cargoText, '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"').Groups[1].Value
    $result = [pscustomobject]@{
        Accepted = $true
        Version = $version
        InitialWidth = $initial.Width
        InitialHeight = $initial.Height
        Samples = $samples
        RestartPreset = 'large'
        RestartWidth = $restartRect.Width
        RestartHeight = $restartRect.Height
    }
}
finally {
    if ($null -ne $process -and -not $process.HasExited) {
        $shutdown = Start-Process -FilePath $ExecutablePath -ArgumentList '--shutdown' -WindowStyle Hidden -Wait -PassThru
        if ($shutdown.ExitCode -ne 0 -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force
        }
    }
}

$evidencePath = Join-Path $testRoot 'result.json'
$json = $result | ConvertTo-Json -Depth 5
[IO.File]::WriteAllText($evidencePath, $json, [Text.UTF8Encoding]::new($false))
$result | ConvertTo-Json -Depth 5
