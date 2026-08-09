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
    $parsedTimestamp = [DateTime]::MinValue
    if ($null -eq $timestamp -or
        $timestamp.Value -isnot [string] -or
        -not [DateTime]::TryParseExact(
            [string]$timestamp.Value,
            'o',
            [System.Globalization.CultureInfo]::InvariantCulture,
            [System.Globalization.DateTimeStyles]::RoundtripKind,
            [ref]$parsedTimestamp
        ) -or
        $parsedTimestamp.Kind -ne [DateTimeKind]::Utc) {
        Write-Verbose '[isolated-validation] existing receipt timestamp failed validation'
        throw 'the existing launch receipt does not match the trusted handoff'
    }
}

function Get-IsolatedReceiptDynamicFields {
    param(
        [Parameter(Mandatory)][string]$PackageId,
        [Parameter(Mandatory)][string]$RootPath
    )

    $effectiveRoot = $RootPath
    if ($null -eq ('IsolatedValidationFileIdentity' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;
using System.Text;

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationFileTime {
    public UInt32 Low;
    public UInt32 High;
}

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationByHandleFileInformation {
    public UInt32 FileAttributes;
    public IsolatedValidationFileTime CreationTime;
    public IsolatedValidationFileTime LastAccessTime;
    public IsolatedValidationFileTime LastWriteTime;
    public UInt32 VolumeSerialNumber;
    public UInt32 FileSizeHigh;
    public UInt32 FileSizeLow;
    public UInt32 NumberOfLinks;
    public UInt32 FileIndexHigh;
    public UInt32 FileIndexLow;
}

public static class IsolatedValidationFileIdentity {
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern SafeFileHandle CreateFile(
        string fileName,
        UInt32 desiredAccess,
        UInt32 shareMode,
        IntPtr securityAttributes,
        UInt32 creationDisposition,
        UInt32 flagsAndAttributes,
        IntPtr templateFile);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetFileInformationByHandle(
        SafeFileHandle file,
        out IsolatedValidationByHandleFileInformation information);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern UInt32 GetFinalPathNameByHandle(
        SafeFileHandle file,
        StringBuilder path,
        UInt32 pathLength,
        UInt32 flags);
}
'@ -ErrorAction Stop
    }

    $handle = [IsolatedValidationFileIdentity]::CreateFile(
        $effectiveRoot,
        [uint32]0,
        [uint32]7,
        [IntPtr]::Zero,
        [uint32]3,
        [uint32]0x02200000,
        [IntPtr]::Zero
    )
    if ($handle.IsInvalid) {
        throw 'could not read the isolated root identity'
    }

    try {
        $information = New-Object IsolatedValidationByHandleFileInformation
        if (-not [IsolatedValidationFileIdentity]::GetFileInformationByHandle($handle, [ref]$information)) {
            throw 'could not read the isolated root identity'
        }
        if (($information.FileAttributes -band [uint32]0x00000400) -ne 0) {
            throw 'the isolated root must not be a reparse point'
        }

        $pathCapacity = 1024
        while ($true) {
            $canonicalPath = [System.Text.StringBuilder]::new($pathCapacity)
            $pathLength = [IsolatedValidationFileIdentity]::GetFinalPathNameByHandle(
                $handle,
                $canonicalPath,
                [uint32]$pathCapacity,
                [uint32]0
            )
            if ($pathLength -eq 0) {
                throw 'could not read the isolated root path'
            }
            if ($pathLength -lt $pathCapacity) {
                $effectiveRoot = $canonicalPath.ToString()
                break
            }
            if ($pathLength -ge 32768) {
                throw 'could not read the isolated root path'
            }
            $pathCapacity = [int]$pathLength + 1
        }
    }
    finally {
        $handle.Dispose()
    }

    $packageBytes = [System.Text.Encoding]::UTF8.GetBytes($PackageId)
    $payload = New-Object byte[] ($packageBytes.Length + 16)
    $volume = [BitConverter]::GetBytes([uint64]$information.VolumeSerialNumber)
    $file = [BitConverter]::GetBytes(
        ([uint64]$information.FileIndexHigh * [uint64]4294967296) + [uint64]$information.FileIndexLow
    )
    [Buffer]::BlockCopy($packageBytes, 0, $payload, 0, $packageBytes.Length)
    [Buffer]::BlockCopy($volume, 0, $payload, $packageBytes.Length, $volume.Length)
    [Buffer]::BlockCopy($file, 0, $payload, $packageBytes.Length + $volume.Length, $file.Length)
    $hasher = [System.Security.Cryptography.SHA256]::Create()
    try {
        $mutexHash = ([BitConverter]::ToString($hasher.ComputeHash($payload))).Replace('-', '').ToLowerInvariant()
    }
    finally {
        $hasher.Dispose()
    }

    return [ordered]@{
        effectiveRoot = $effectiveRoot
        mutexHash = $mutexHash
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
    $expectedDynamicFields = Get-IsolatedReceiptDynamicFields `
        -PackageId $trustedReceiptFields.packageId `
        -RootPath $isolatedStateRoot
    $trustedReceiptFields.effectiveRoot = $expectedDynamicFields.effectiveRoot
    $trustedReceiptFields.mutexHash = $expectedDynamicFields.mutexHash
    Assert-ReceiptFields -Receipt $existingReceipt -ExpectedFields $trustedReceiptFields
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
} elseif ($statusJson.effectiveRoot -cne $trustedReceiptFields.effectiveRoot -or
    $statusJson.mutexHash -cne $trustedReceiptFields.mutexHash) {
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
