[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path,
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^[0-9A-Fa-f]{40}$')]
    [string]$CertificateThumbprint,
    [string]$TimestampUrl = 'http://timestamp.digicert.com'
)

$ErrorActionPreference = 'Stop'

function Resolve-SignTool {
    $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($null -ne $command) {
        return $command.Source
    }
    $kitsRoot = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
    $candidate = Get-ChildItem -LiteralPath $kitsRoot -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending |
        ForEach-Object { Join-Path $_.FullName 'x64\signtool.exe' } |
        Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
        Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($candidate)) {
        throw 'signtool.exe was not found. Install the Windows SDK signing tools.'
    }
    return $candidate
}

$signTool = Resolve-SignTool
$thumbprint = $CertificateThumbprint.ToUpperInvariant()
foreach ($item in $Path) {
    $resolved = (Resolve-Path -LiteralPath $item).Path
    & $signTool sign /sha1 $thumbprint /fd SHA256 /tr $TimestampUrl /td SHA256 $resolved
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode signing failed for $resolved"
    }
    & $signTool verify /pa /all $resolved
    if ($LASTEXITCODE -ne 0) {
        throw "Authenticode verification failed for $resolved"
    }
}

[pscustomobject]@{
    Signed = @($Path).Count
    CertificateThumbprint = $thumbprint
    TimestampUrl = $TimestampUrl
}
