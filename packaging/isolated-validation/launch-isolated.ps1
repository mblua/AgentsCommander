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

    if ($CaptureOutput.IsPresent) {
        return Start-IsolatedValidationNativeProcess `
            -Mode CaptureAndWait `
            -FilePath $Executable `
            -WorkingDirectory $PSScriptRoot `
            -Arguments $Arguments `
            -StandardOutputLimitBytes 64KB `
            -StandardErrorLimitBytes 64KB `
            -RemoveAgentsCommanderEnvironment
    }

    $lease = Start-IsolatedValidationNativeProcess `
        -Mode Start `
        -FilePath $Executable `
        -WorkingDirectory $PSScriptRoot `
        -Arguments $Arguments `
        -RemoveAgentsCommanderEnvironment
    return $lease.Process
}

function Normalize-ComparablePath {
    param([Parameter(Mandatory)][string]$Path)

    if ($Path.StartsWith('\\?\', [System.StringComparison]::Ordinal)) {
        return $Path.Substring(4)
    }
    return $Path
}

function Publish-ReceiptAtomically {
    param(
        [Parameter(Mandatory)][string]$TemporaryPath,
        [Parameter(Mandatory)][string]$DestinationPath
    )

    try {
        if (Test-Path -LiteralPath $DestinationPath -PathType Leaf) {
            [System.IO.File]::Move($TemporaryPath, $DestinationPath, $true)
        } else {
            [System.IO.File]::Move($TemporaryPath, $DestinationPath)
        }
    } finally {
        if (Test-Path -LiteralPath $TemporaryPath -PathType Leaf) {
            Remove-Item -LiteralPath $TemporaryPath -Force -ErrorAction SilentlyContinue
        }
    }
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
$expectedExecutableFileName = 'agentscommander.exe'
$expectedProfileResourceRelativePath = 'resources/isolated-validation/package-profile.toml'
if ($manifest.artifactKind -cne 'portable-layout' -or
    $manifest.executableFileName -cne $expectedExecutableFileName -or
    $manifest.profileResourceRelativePath -cne $expectedProfileResourceRelativePath) {
    throw 'handoff manifest does not describe the required portable artifact layout'
}

$executable = Join-Path $artifactDirectory $expectedExecutableFileName
$profile = Join-Path $artifactDirectory ($expectedProfileResourceRelativePath -replace '/', '\')
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
if ($manifest.bundledProfileSha256 -cne $manifest.installedProfileSha256) {
    throw 'handoff manifest reports mismatched bundled and portable profile hashes'
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
try {
    [System.IO.File]::WriteAllText(
        $receiptTemporaryPath,
        ($receipt | ConvertTo-Json -Depth 8) + [Environment]::NewLine,
        [System.Text.UTF8Encoding]::new($false)
    )
    Publish-ReceiptAtomically -TemporaryPath $receiptTemporaryPath -DestinationPath $receiptPath
} finally {
    if (Test-Path -LiteralPath $receiptTemporaryPath -PathType Leaf) {
        Remove-Item -LiteralPath $receiptTemporaryPath -Force -ErrorAction SilentlyContinue
    }
}

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
