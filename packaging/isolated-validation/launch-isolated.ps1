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

trap {
    $failure = $_.Exception.Message
    if ($failure -notmatch '^E_ISOLATION_[A-Z_]+$') {
        $failure = 'E_ISOLATION_LAUNCHER'
    }
    [Console]::Error.WriteLine($failure)
    exit 2
}

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

function Test-FullyQualifiedWindowsPath {
    param([Parameter(Mandatory)][string]$Path)

    if ($Path.IndexOf([char]0) -ge 0) {
        return $false
    }

    $isPathFullyQualified = [System.IO.Path].GetMethod(
        'IsPathFullyQualified',
        [System.Type[]]@([string])
    )
    if ($null -ne $isPathFullyQualified) {
        return [System.IO.Path]::IsPathFullyQualified($Path)
    }

    return $Path -match '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+(?:[\\/]|$)|\\\\\?\\(?:[A-Za-z]:[\\/]|UNC\\[^\\/]+[\\/][^\\/]+(?:[\\/]|$)))'
}

$fixture = $FixtureRoot
if (-not (Test-FullyQualifiedWindowsPath -Path $fixture) -or
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

function Assert-ReceiptFields {
    param(
        [Parameter(Mandatory)]$Receipt,
        [Parameter(Mandatory)][System.Collections.IDictionary]$ExpectedFields
    )

    foreach ($entry in $ExpectedFields.GetEnumerator()) {
        $property = $Receipt.PSObject.Properties[$entry.Key]
        if ($null -eq $property -or
            $property.Value -isnot [string] -or
            $property.Value -cne [string]$entry.Value) {
            Write-Verbose "[isolated-validation] existing receipt trusted field failed: $($entry.Key)"
            throw 'the existing launch receipt does not match the trusted handoff'
        }
    }
}

function Assert-ReceiptDynamicFields {
    param([Parameter(Mandatory)]$Receipt)

    foreach ($name in @('effectiveRoot', 'mutexHash')) {
        $property = $Receipt.PSObject.Properties[$name]
        if ($null -eq $property -or
            $property.Value -isnot [string] -or
            [string]::IsNullOrWhiteSpace($property.Value)) {
            Write-Verbose "[isolated-validation] existing receipt dynamic field failed: $name"
            throw 'the existing launch receipt has an invalid dynamic field'
        }
    }

    $timestampProperty = $Receipt.PSObject.Properties['utcTimestamp']
    if ($null -eq $timestampProperty -or $null -eq $timestampProperty.Value) {
        Write-Verbose '[isolated-validation] existing receipt timestamp is missing'
        throw 'the existing launch receipt has an invalid timestamp'
    }
    if ($timestampProperty.Value -is [DateTime]) {
        return
    }
    if ($timestampProperty.Value -isnot [string] -or
        [string]::IsNullOrWhiteSpace($timestampProperty.Value)) {
        Write-Verbose '[isolated-validation] existing receipt timestamp has an invalid shape'
        throw 'the existing launch receipt has an invalid timestamp'
    }

    $timestamp = [DateTime]::MinValue
    if (-not [DateTime]::TryParse(
        [string]$timestampProperty.Value,
        [System.Globalization.CultureInfo]::InvariantCulture,
        [System.Globalization.DateTimeStyles]::RoundtripKind,
        [ref]$timestamp
    )) {
        Write-Verbose '[isolated-validation] existing receipt timestamp failed validation'
        throw 'the existing launch receipt has an invalid timestamp'
    }
}

function Stop-AndDisposeIsolatedGuiProcess {
    param([System.Diagnostics.Process]$Process)

    if ($null -eq $Process) {
        return
    }

    try {
        if (-not $Process.HasExited) {
            $Process.Kill()
        }
        if (-not $Process.WaitForExit(3000)) {
            throw 'owned GUI child did not exit within the cleanup timeout'
        }
        if (-not $Process.HasExited) {
            throw 'owned GUI child did not exit after termination'
        }
        Write-Verbose '[isolated-validation] owned GUI cleanup completed'
    }
    catch {
        Write-Verbose "[isolated-validation] owned GUI cleanup failed: $($_.Exception.Message)"
        throw 'E_ISOLATION_NATIVE_PROCESS'
    }
    finally {
        $Process.Dispose()
    }
}

$isolatedStateRoot = Join-Path $fixture 'app-state'
$receiptPath = Join-Path $fixture 'launch-receipt.json'
$trustedReceiptFields = [ordered]@{
    schema = 'isolated-validation-launch-receipt-v1'
    expectedManifestSha256 = $ExpectedManifestSha256.ToLowerInvariant()
    manifestSha256 = $actualManifestHash
    fixtureRoot = $fixture
    isolatedStateRoot = $isolatedStateRoot
    packageId = 'agentscommander-1271-isolated-gates'
    profileSha256 = $manifest.compiledProfileSha256
    workspace = 'AgentsCommander_1271_isolated'
    matrix = 'WG-1271-ISOLATED-GATES'
    replicaAgent = 'gate-tester'
    headerIdentity = 'WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated'
    bundleIdentifier = 'dev.agentscommander.isolatedgates'
    executableSha256 = $manifest.payloads.executable.sha256
    profilePayloadSha256 = $manifest.payloads.profile.sha256
    launcherSha256 = $manifest.payloads.launcher.sha256
    nativeProcessModuleSha256 = $manifest.payloads.nativeProcessModule.sha256
}

$existingReceipt = $null
if (Test-Path -LiteralPath $receiptPath) {
    if (-not (Test-Path -LiteralPath $receiptPath -PathType Leaf)) {
        throw 'the existing launch receipt is not a regular file'
    }
    try {
        $existingReceipt = Get-Content -LiteralPath $receiptPath -Raw | ConvertFrom-Json
    }
    catch {
        throw 'the existing launch receipt is malformed'
    }
    Assert-ReceiptFields -Receipt $existingReceipt -ExpectedFields $trustedReceiptFields
    Assert-ReceiptDynamicFields -Receipt $existingReceipt
}

$status = Start-IsolatedChild -Executable $executable -Arguments @(
    '--isolated-state-root',
    $isolatedStateRoot,
    '--isolation-status'
) -CaptureOutput
if ($status.ExitCode -ne 0) {
    throw 'E_ISOLATION_STATUS'
}

try {
    $statusJson = $status.StandardOutput.Trim() | ConvertFrom-Json
}
catch {
    throw 'E_ISOLATION_STATUS'
}
if ([string]::IsNullOrWhiteSpace([string]$statusJson.effectiveRoot) -or
    [string]::IsNullOrWhiteSpace([string]$statusJson.mutexHash) -or
    $statusJson.packageId -cne $trustedReceiptFields.packageId -or
    $statusJson.profileSha256 -cne $trustedReceiptFields.profileSha256 -or
    $statusJson.workspace -cne $trustedReceiptFields.workspace -or
    $statusJson.matrix -cne $trustedReceiptFields.matrix -or
    $statusJson.replicaAgent -cne $trustedReceiptFields.replicaAgent -or
    $statusJson.headerIdentity -cne $trustedReceiptFields.headerIdentity -or
    $statusJson.bundleIdentifier -cne $trustedReceiptFields.bundleIdentifier) {
    throw 'E_ISOLATION_STATUS'
}

$trustedReceiptFields.effectiveRoot = $statusJson.effectiveRoot
$trustedReceiptFields.mutexHash = $statusJson.mutexHash
if ($null -ne $existingReceipt) {
    Assert-ReceiptFields -Receipt $existingReceipt -ExpectedFields $trustedReceiptFields
}

$receiptTemporaryPath = $null
$guiProcess = $null
$publishedReceipt = $false
try {
    if ($null -eq $existingReceipt) {
        $receiptTemporaryPath = Join-Path $fixture ('.launch-receipt-' + [Guid]::NewGuid().ToString('N') + '.tmp')
        $receipt = [ordered]@{}
        foreach ($entry in $trustedReceiptFields.GetEnumerator()) {
            $receipt[$entry.Key] = $entry.Value
        }
        $receipt.utcTimestamp = [DateTime]::UtcNow.ToString('o')
        [System.IO.File]::WriteAllText(
            $receiptTemporaryPath,
            ($receipt | ConvertTo-Json -Depth 8) + [Environment]::NewLine,
            [System.Text.UTF8Encoding]::new($false)
        )
    }

    $guiProcess = Start-IsolatedChild -Executable $executable -Arguments @(
        '--app',
        '--isolated-state-root',
        $isolatedStateRoot
    )

    if ($null -eq $existingReceipt) {
        [System.IO.File]::Move($receiptTemporaryPath, $receiptPath)
        $publishedReceipt = $true
        $receiptTemporaryPath = $null
    }

    [pscustomobject]@{
        processId = $guiProcess.Id
        receipt = $receiptPath
        stateRoot = $statusJson.effectiveRoot
    } | ConvertTo-Json -Depth 4
}
catch {
    if ($null -ne $guiProcess) {
        Stop-AndDisposeIsolatedGuiProcess -Process $guiProcess
        $guiProcess = $null
    }
    throw
}
finally {
    if ($null -ne $receiptTemporaryPath -and (Test-Path -LiteralPath $receiptTemporaryPath -PathType Leaf)) {
        Remove-Item -LiteralPath $receiptTemporaryPath -Force -ErrorAction SilentlyContinue
    }
    if ($null -ne $guiProcess) {
        $guiProcess.Dispose()
    }
}
