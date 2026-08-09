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

$fixture = $FixtureRoot

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

$nativeProcessModule = Import-Module -Name $nativeProcessModulePath -Force -PassThru -ErrorAction Stop

$fixtureIsFullyQualified = & $nativeProcessModule {
    param($Path)
    Test-IsolatedValidationFullyQualifiedPath -Path $Path
} $fixture
if (-not $fixtureIsFullyQualified -or
    -not [System.IO.Directory]::Exists($fixture)) {
    throw 'the fixture root must be an existing absolute directory'
}

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

    $timestamp = $Receipt.PSObject.Properties['utcTimestamp']
    $timestampText = $null
    if ($null -ne $timestamp) {
        if ($timestamp.Value -is [string]) {
            $timestampText = [string]$timestamp.Value
        }
        elseif ($timestamp.Value -is [DateTime]) {
            # ConvertFrom-Json in PowerShell Core materializes ISO-8601 JSON strings as DateTime.
            $timestampText = $timestamp.Value.ToString(
                'o',
                [System.Globalization.CultureInfo]::InvariantCulture
            )
        }
    }

    $parsedTimestamp = [DateTime]::MinValue
    if ([string]::IsNullOrWhiteSpace($timestampText) -or
        -not [DateTime]::TryParseExact(
            $timestampText,
            'o',
            [System.Globalization.CultureInfo]::InvariantCulture,
            [System.Globalization.DateTimeStyles]::RoundtripKind,
            [ref]$parsedTimestamp
        ) -or
        $parsedTimestamp.Kind -ne [DateTimeKind]::Utc) {
        Write-Verbose '[isolated-validation] existing receipt timestamp failed validation'
        throw 'the existing launch receipt does not match the trusted handoff'
    }

    $effectiveRoot = $Receipt.PSObject.Properties['effectiveRoot']
    $mutexHash = $Receipt.PSObject.Properties['mutexHash']
    if ($null -eq $effectiveRoot -or
        $effectiveRoot.Value -isnot [string] -or
        [string]::IsNullOrWhiteSpace([string]$effectiveRoot.Value) -or
        $null -eq $mutexHash -or
        $mutexHash.Value -isnot [string] -or
        [string]$mutexHash.Value -cnotmatch '^[0-9a-f]{64}$') {
        Write-Verbose '[isolated-validation] existing receipt dynamic root identity failed validation'
        throw 'the existing launch receipt does not match the trusted handoff'
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

function Initialize-IsolatedValidationReceiptNative {
    if ($null -eq ('IsolatedValidationReceiptNative' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

public static class IsolatedValidationReceiptNative
{
    [StructLayout(LayoutKind.Sequential)]
    public struct ByHandleFileInformation
    {
        public uint FileAttributes;
        public System.Runtime.InteropServices.ComTypes.FILETIME CreationTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastAccessTime;
        public System.Runtime.InteropServices.ComTypes.FILETIME LastWriteTime;
        public uint VolumeSerialNumber;
        public uint FileSizeHigh;
        public uint FileSizeLow;
        public uint NumberOfLinks;
        public uint FileIndexHigh;
        public uint FileIndexLow;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern SafeFileHandle CreateFile(
        string fileName,
        uint desiredAccess,
        uint shareMode,
        IntPtr securityAttributes,
        uint creationDisposition,
        uint flagsAndAttributes,
        IntPtr templateFile
    );

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetFileInformationByHandle(
        SafeFileHandle file,
        out ByHandleFileInformation information
    );
}
'@
    }
}

function Get-IsolatedReceiptDirectoryIdentity {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][string]$Description
    )

    Initialize-IsolatedValidationReceiptNative

    $directoryHandle = [IsolatedValidationReceiptNative]::CreateFile(
        $Path,
        [uint32]0x00000080,
        [uint32]0x00000007,
        [IntPtr]::Zero,
        [uint32]3,
        [uint32]0x02200000,
        [IntPtr]::Zero
    )
    if ($directoryHandle.IsInvalid) {
        $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
        $directoryHandle.Dispose()
        throw "could not inspect the $Description directory identity: $errorCode"
    }

    try {
        $information = New-Object IsolatedValidationReceiptNative+ByHandleFileInformation
        if (-not [IsolatedValidationReceiptNative]::GetFileInformationByHandle($directoryHandle, [ref]$information)) {
            $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
            throw "could not read the $Description directory identity: $errorCode"
        }
        if (([uint32]$information.FileAttributes -band [uint32]0x00000400) -ne 0) {
            throw "the $Description directory identity is a reparse point"
        }

        return [pscustomobject]@{
            volumeSerialNumber = [uint32]$information.VolumeSerialNumber
            fileIndex = ([uint64]$information.FileIndexHigh * [uint64]4294967296) + [uint64]$information.FileIndexLow
        }
    }
    finally {
        $directoryHandle.Dispose()
    }
}

function Get-IsolatedReceiptFinalRootMutexHash {
    param(
        [Parameter(Mandatory)][string]$PackageId,
        [Parameter(Mandatory)]$RootIdentity
    )

    $hashInput = New-Object 'System.Collections.Generic.List[byte]'
    $hashInput.AddRange([Text.Encoding]::UTF8.GetBytes($PackageId))
    $hashInput.AddRange([BitConverter]::GetBytes([uint64]$RootIdentity.volumeSerialNumber))
    $hashInput.AddRange([BitConverter]::GetBytes([uint64]$RootIdentity.fileIndex))

    $sha256 = [Security.Cryptography.SHA256]::Create()
    try {
        return ([BitConverter]::ToString($sha256.ComputeHash($hashInput.ToArray())) -replace '-', '').ToLowerInvariant()
    }
    finally {
        $sha256.Dispose()
    }
}

function Assert-IsolatedReceiptFinalRootBinding {
    param(
        [Parameter(Mandatory)]$Receipt,
        [Parameter(Mandatory)][string]$RawStateRoot,
        [Parameter(Mandatory)][string]$PackageId
    )

    # Status owns the bootstrap parent-plus-leaf lock. Persisted receipts instead bind
    # the canonical final root and its final-root mutex. The raw state-root argument is
    # inspected only by handle and is never resolved or normalized here.
    $rawIdentity = Get-IsolatedReceiptDirectoryIdentity `
        -Path $RawStateRoot `
        -Description 'raw isolated app-state'
    $receiptIdentity = Get-IsolatedReceiptDirectoryIdentity `
        -Path ([string]$Receipt.effectiveRoot) `
        -Description 'receipt effective-root'
    if ($rawIdentity.volumeSerialNumber -ne $receiptIdentity.volumeSerialNumber -or
        $rawIdentity.fileIndex -ne $receiptIdentity.fileIndex) {
        throw 'the existing launch receipt does not match the trusted handoff'
    }

    $expectedMutexHash = Get-IsolatedReceiptFinalRootMutexHash `
        -PackageId $PackageId `
        -RootIdentity $rawIdentity
    if ([string]$Receipt.mutexHash -cne $expectedMutexHash) {
        throw 'the existing launch receipt does not match the trusted handoff'
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
    Assert-IsolatedReceiptFinalRootBinding `
        -Receipt $existingReceipt `
        -RawStateRoot $isolatedStateRoot `
        -PackageId $trustedReceiptFields.packageId
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

if ($null -eq $existingReceipt) {
    $trustedReceiptFields.effectiveRoot = $statusJson.effectiveRoot
    $trustedReceiptFields.mutexHash = $statusJson.mutexHash
} elseif ($statusJson.effectiveRoot -cne $existingReceipt.effectiveRoot -or
    $statusJson.mutexHash -cne $existingReceipt.mutexHash) {
    throw 'E_ISOLATION_STATUS'
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

    $launchResult = [pscustomobject]@{
        processId = $guiProcess.Id
        receipt = $receiptPath
        stateRoot = $statusJson.effectiveRoot
    }
    $launchResult | ConvertTo-Json -Depth 4
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
