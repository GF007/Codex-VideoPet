[CmdletBinding()]
param(
    [string]$ExecutablePath
)

$ErrorActionPreference = 'Stop'
$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if ([string]::IsNullOrWhiteSpace($ExecutablePath)) {
    $ExecutablePath = Join-Path $env:LOCALAPPDATA 'Programs\CodexPet\CodexPet.exe'
}
$ExecutablePath = (Resolve-Path -LiteralPath $ExecutablePath).Path
$artifactRoot = Join-Path $projectRoot 'artifacts\release-0.3.0-final'
New-Item -ItemType Directory -Path $artifactRoot -Force | Out-Null
$testRoot = Join-Path $artifactRoot ("hooks-profile-{0}-{1}" -f [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds(), $PID)
$profile = Join-Path $testRoot 'UserProfile'
$localAppData = Join-Path $testRoot 'LocalAppData'
$codexRoot = Join-Path $profile '.codex'
New-Item -ItemType Directory -Path $codexRoot, $localAppData -Force | Out-Null
$hooksPath = Join-Path $codexRoot 'hooks.json'
$fixture = @{
    customRoot = 'preserve'
    hooks = @{
        Stop = @(@{
            matcher = 'user-rule'
            customGroup = $true
            hooks = @(@{
                type = 'command'
                command = 'third-party-command'
                customNested = $true
            })
        })
        CustomEvent = @(@{ hooks = @(@{ type = 'command'; command = 'custom' }) })
    }
} | ConvertTo-Json -Depth 12
[IO.File]::WriteAllText($hooksPath, $fixture, [Text.UTF8Encoding]::new($false))

function Invoke-IntegrationCommand([string]$Argument) {
    $start = New-Object Diagnostics.ProcessStartInfo
    $start.FileName = $ExecutablePath
    $start.Arguments = $Argument
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.EnvironmentVariables['USERPROFILE'] = $profile
    $start.EnvironmentVariables['LOCALAPPDATA'] = $localAppData
    $process = [Diagnostics.Process]::Start($start)
    if (-not $process.WaitForExit(10000)) {
        throw "$Argument timed out."
    }
    if ($process.ExitCode -ne 0) {
        throw "$Argument failed with exit code $($process.ExitCode)."
    }
}

Invoke-IntegrationCommand '--install-hooks'
$firstHash = (Get-FileHash -LiteralPath $hooksPath -Algorithm SHA256).Hash
$firstBackups = @(Get-ChildItem -LiteralPath $codexRoot -Filter 'hooks.json.bak.*' -File).Count
Invoke-IntegrationCommand '--install-hooks'
$secondHash = (Get-FileHash -LiteralPath $hooksPath -Algorithm SHA256).Hash
$secondBackups = @(Get-ChildItem -LiteralPath $codexRoot -Filter 'hooks.json.bak.*' -File).Count
$installed = Get-Content -LiteralPath $hooksPath -Raw | ConvertFrom-Json
$allHooks = @($installed.hooks.psobject.Properties.Value | ForEach-Object { @($_) } | ForEach-Object { @($_.hooks) })
$ownedCount = @($allHooks | Where-Object {
    [string]$_.command -like '* --hook --owner codex-pet-overlay' -and
    [string]$_.commandWindows -like '* --hook --owner codex-pet-overlay'
}).Count
$userPreservedAfterInstall = @($installed.hooks.Stop[0].hooks | Where-Object command -eq 'third-party-command').Count -eq 1

Invoke-IntegrationCommand '--remove-hooks'
$removed = Get-Content -LiteralPath $hooksPath -Raw | ConvertFrom-Json
$remainingHooks = @($removed.hooks.psobject.Properties.Value | ForEach-Object { @($_) } | ForEach-Object { @($_.hooks) })
$remainingOwned = @($remainingHooks | Where-Object { [string]$_.command -like '* --hook --owner codex-pet-overlay' }).Count
$userPreservedAfterRemove = @($removed.hooks.Stop[0].hooks | Where-Object command -eq 'third-party-command').Count -eq 1
$result = [pscustomobject]@{
    TestRoot = $testRoot
    OwnedAfterInstall = $ownedCount
    FirstAndSecondInstallIdentical = $firstHash -eq $secondHash
    BackupCountStableOnNoop = $firstBackups -eq 1 -and $secondBackups -eq 1
    UserHookPreservedAfterInstall = $userPreservedAfterInstall
    OwnedAfterRemove = $remainingOwned
    UserHookPreservedAfterRemove = $userPreservedAfterRemove
    UnknownRootPreserved = $removed.customRoot -eq 'preserve'
}
$accepted = $ownedCount -eq 11 -and $result.FirstAndSecondInstallIdentical -and $result.BackupCountStableOnNoop -and $userPreservedAfterInstall -and $remainingOwned -eq 0 -and $userPreservedAfterRemove -and $result.UnknownRootPreserved
$result | Add-Member -NotePropertyName Accepted -NotePropertyValue $accepted
$json = $result | ConvertTo-Json -Compress
[IO.File]::WriteAllText((Join-Path $testRoot 'result.json'), $json, [Text.UTF8Encoding]::new($false))
Write-Output $json
if (-not $accepted) {
    throw 'Hook lifecycle acceptance failed.'
}
