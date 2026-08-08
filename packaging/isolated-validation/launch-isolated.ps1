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

$fixture = $FixtureRoot
if ($fixture.IndexOf([char]0) -ge 0 -or
    -not [System.IO.Path]::IsPathRooted($fixture) -or
    $fixture -match '^[A-Za-z]:[^\\/]' -or
    -not [System.IO.Directory]::Exists($fixture)) {
    throw 'the fixture root must be an existing absolute directory'
}

$artifactDirectory = $PSScriptRoot
$manifestPath = Join-Path $artifactDirectory 'isolated-validation-manifest.json'
$executable = Join-Path $artifactDirectory 'Agents Commander Isolated Gates.exe'
$profile = Join-Path $artifactDirectory 'resources/package-profile.toml'
$launcherPath = Join-Path $artifactDirectory 'launch-isolated.ps1'
$nativeProcessModulePath = Join-Path $artifactDirectory 'native-process.psm1'
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "handoff manifest is missing: $manifestPath"
}

$actualManifestHash = Get-Sha256 -LiteralPath $manifestPath
if ($actualManifestHash -cne $ExpectedManifestSha256.ToLowerInvariant()) {
    throw 'the handoff manifest hash does not match the trusted expected hash'
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($manifest.schema -cne 'isolated-validation-handoff-v1' -or
    $manifest.artifactKind -cne 'portable-layout' -or
    $manifest.mode -cne 'isolated-validation-package' -or
    $manifest.productLabel -cne 'Agents Commander Isolated Gates' -or
    $manifest.bundleIdentifier -cne 'dev.agentscommander.isolatedgates' -or
    $manifest.headerIdentity -cne 'WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated') {
    throw 'handoff manifest does not describe the required portable artifact layout'
}

$requiredPayloads = @(
    [pscustomobject]@{ Name = 'executable'; RelativePath = 'Agents Commander Isolated Gates.exe'; LiteralPath = $executable },
    [pscustomobject]@{ Name = 'profile'; RelativePath = 'resources/package-profile.toml'; LiteralPath = $profile },
    [pscustomobject]@{ Name = 'launcher'; RelativePath = 'launch-isolated.ps1'; LiteralPath = $launcherPath },
    [pscustomobject]@{ Name = 'nativeProcessModule'; RelativePath = 'native-process.psm1'; LiteralPath = $nativeProcessModulePath }
)
foreach ($requiredPayload in $requiredPayloads) {
    $payload = $manifest.payloads.($requiredPayload.Name)
    if ($null -eq $payload -or
        $payload.relativePath -cne $requiredPayload.RelativePath -or
        -not (Test-Path -LiteralPath $requiredPayload.LiteralPath -PathType Leaf) -or
        (Get-Sha256 -LiteralPath $requiredPayload.LiteralPath) -cne $payload.sha256.ToLowerInvariant()) {
        throw 'handoff payload hash verification failed'
    }
}
if ($manifest.compiledProfileSha256 -cne $manifest.payloads.profile.sha256) {
    throw 'handoff manifest reports mismatched compiled and installed profile hashes'
}

Import-Module -Name $nativeProcessModulePath -Force -ErrorAction Stop

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
if ([string]::IsNullOrWhiteSpace([string]$statusJson.effectiveRoot)) {
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
