[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$FixtureRoot,

    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-fA-F]{64}$')]
    [string]$ExpectedManifestSha256
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-Sha256 {
    param([Parameter(Mandatory)][string]$LiteralPath)

    return (Get-FileHash -LiteralPath $LiteralPath -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Start-IsolatedChild {
    param(
        [Parameter(Mandatory)][string]$Executable,
        [Parameter(Mandatory)][string[]]$Arguments,
        [switch]$CaptureOutput
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $Executable
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $CaptureOutput.IsPresent
    $startInfo.RedirectStandardError = $CaptureOutput.IsPresent

    foreach ($key in @($startInfo.Environment.Keys)) {
        if ($key.StartsWith('AGENTSCOMMANDER_', [System.StringComparison]::OrdinalIgnoreCase)) {
            [void]$startInfo.Environment.Remove($key)
        }
    }
    foreach ($argument in $Arguments) {
        [void]$startInfo.ArgumentList.Add($argument)
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw 'failed to start isolated package process'
    }

    if (-not $CaptureOutput.IsPresent) {
        return $process
    }

    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    return [pscustomobject]@{
        ExitCode = $process.ExitCode
        StandardOutput = $stdout
        StandardError = $stderr
    }
}

function Normalize-ComparablePath {
    param([Parameter(Mandatory)][string]$Path)

    if ($Path.StartsWith('\\?\', [System.StringComparison]::Ordinal)) {
        return $Path.Substring(4)
    }
    return $Path
}

if (-not [System.IO.Path]::IsPathRooted($FixtureRoot)) {
    throw '-FixtureRoot must be an existing absolute directory'
}
if (-not (Test-Path -LiteralPath $FixtureRoot -PathType Container)) {
    throw '-FixtureRoot must name an existing directory'
}

$fixture = (Resolve-Path -LiteralPath $FixtureRoot).Path
$artifactDirectory = $PSScriptRoot
$manifestPath = Join-Path $artifactDirectory 'isolated-validation-handoff.json'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "handoff manifest is missing: $manifestPath"
}

$actualManifestHash = Get-Sha256 -LiteralPath $manifestPath
if ($actualManifestHash -cne $ExpectedManifestSha256.ToLowerInvariant()) {
    throw 'the handoff manifest hash does not match the trusted expected hash'
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$executable = Join-Path $artifactDirectory $manifest.executableFileName
$profile = Join-Path $artifactDirectory ($manifest.profileResourceRelativePath -replace '/', '\')
if (-not (Test-Path -LiteralPath $executable -PathType Leaf)) {
    throw "handoff executable is missing: $executable"
}
if (-not (Test-Path -LiteralPath $profile -PathType Leaf)) {
    throw "handoff package profile is missing: $profile"
}
if ((Get-Sha256 -LiteralPath $executable) -cne $manifest.executableSha256) {
    throw 'handoff executable hash verification failed'
}
if ((Get-Sha256 -LiteralPath $profile) -cne $manifest.installedProfileSha256) {
    throw 'handoff profile hash verification failed'
}
if ($manifest.compiledProfileSha256 -cne $manifest.installedProfileSha256) {
    throw 'handoff manifest reports mismatched compiled and installed profile hashes'
}

$isolatedStateRoot = Join-Path $fixture 'app-state'
$status = Start-IsolatedChild -Executable $executable -Arguments @(
    '--isolated-state-root',
    $isolatedStateRoot,
    '--isolation-status'
) -CaptureOutput
if ($status.ExitCode -ne 0) {
    throw "isolation status failed with exit code $($status.ExitCode): $($status.StandardError.Trim())"
}

$statusJson = $status.StandardOutput.Trim() | ConvertFrom-Json
$canonicalRoot = (Resolve-Path -LiteralPath $isolatedStateRoot).Path
if ((Normalize-ComparablePath $statusJson.effectiveRoot) -cne (Normalize-ComparablePath $canonicalRoot)) {
    throw 'isolation status reported an unexpected effective root'
}
if ($statusJson.packageId -cne 'agentscommander-1271-isolated-gates' -or
    $statusJson.bundleIdentifier -cne 'dev.agentscommander.isolatedgates' -or
    $statusJson.headerIdentity -cne 'WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated') {
    throw 'isolation status reported an unexpected fixed package identity'
}
if ($statusJson.profileSha256 -cne $manifest.compiledProfileSha256) {
    throw 'isolation status profile hash does not match the trusted handoff manifest'
}

$receiptPath = Join-Path $fixture 'launch-receipt.json'
$receiptTemporaryPath = Join-Path $fixture ('.launch-receipt-' + [Guid]::NewGuid().ToString('N') + '.tmp')
$receipt = [ordered]@{
    schemaVersion = 1
    expectedManifestSha256 = $ExpectedManifestSha256.ToLowerInvariant()
    manifestSha256 = $actualManifestHash
    executableSha256 = $manifest.executableSha256
    profileSha256 = $manifest.compiledProfileSha256
    isolationStatus = $statusJson
    utcTimestamp = [DateTime]::UtcNow.ToString('o')
}
[System.IO.File]::WriteAllText(
    $receiptTemporaryPath,
    ($receipt | ConvertTo-Json -Depth 8) + [Environment]::NewLine,
    [System.Text.UTF8Encoding]::new($false)
)
[System.IO.File]::Move($receiptTemporaryPath, $receiptPath)

$guiProcess = Start-IsolatedChild -Executable $executable -Arguments @(
    '--app',
    '--isolated-state-root',
    $isolatedStateRoot
)

[pscustomobject]@{
    processId = $guiProcess.Id
    receipt = $receiptPath
    stateRoot = $canonicalRoot
} | ConvertTo-Json -Depth 4
