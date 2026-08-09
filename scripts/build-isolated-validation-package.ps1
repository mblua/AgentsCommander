[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-fA-F]{40}$')]
    [string]$Frozen1271Commit,

    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-fA-F]{40}$')]
    [string]$IsolatedStateRootCommit,

    [switch]$RevisionPreflightOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ExpectedFrozen1271Commit = 'd68495086e168e5258500832b2ef45b4337ed21a'
$PackageFeature = 'isolated-validation-package'
$ProfileRelativePath = 'packaging/isolated-validation/package-profile.toml'
$OverlayRelativePath = 'src-tauri/tauri.conf.isolated-validation.json'

function Invoke-Git {
    param(
        [Parameter(Mandatory)][string[]]$Arguments,
        [switch]$AllowNonZeroExit
    )

    $result = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $GitExecutable `
        -WorkingDirectory $RepoRoot `
        -Arguments $Arguments `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment
    if ($result.ExitCode -ne 0 -and -not $AllowNonZeroExit) {
        throw "git command failed with exit code $($result.ExitCode)"
    }

    if ($AllowNonZeroExit) {
        return $result
    }

    return $result.StandardOutput
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string]$LiteralPath)

    return (Get-FileHash -LiteralPath $LiteralPath -Algorithm SHA256).Hash.ToLowerInvariant()
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$NativeProcessModulePath = Join-Path $RepoRoot 'packaging/isolated-validation/native-process.psm1'
if (-not [System.IO.File]::Exists($NativeProcessModulePath)) {
    throw 'E_ISOLATION_NATIVE_PROCESS'
}
Import-Module -Name $NativeProcessModulePath -Force -ErrorAction Stop
$GitExecutable = (Get-Command -Name 'git.exe' -CommandType Application -ErrorAction Stop).Path

if ($Frozen1271Commit.ToLowerInvariant() -cne $ExpectedFrozen1271Commit) {
    throw "-Frozen1271Commit must be $ExpectedFrozen1271Commit"
}

$status = Invoke-Git -Arguments @('status', '--porcelain')
if ($status) {
    throw 'a clean worktree is required before building the isolated validation package'
}

$headReference = Invoke-Git -Arguments @('symbolic-ref', '-q', 'HEAD') -AllowNonZeroExit
if ($headReference.ExitCode -eq 0) {
    throw 'a detached worktree is required before building the isolated validation package'
}

$targetCommit = (Invoke-Git -Arguments @('rev-parse', '--verify', "$IsolatedStateRootCommit^{commit}")).Trim()
if ($targetCommit.ToLowerInvariant() -cne $IsolatedStateRootCommit.ToLowerInvariant()) {
    throw '-IsolatedStateRootCommit must resolve to its supplied full 40-hex SHA'
}

$ancestryProbe = Invoke-Git -Arguments @(
    'merge-base',
    '--is-ancestor',
    $ExpectedFrozen1271Commit,
    $targetCommit
) -AllowNonZeroExit
if ($ancestryProbe.ExitCode -ne 0) {
    throw 'the requested combined commit does not descend from the frozen #1271 commit'
}

$normalConfigAtFrozen = Invoke-Git -Arguments @(
    'show',
    ($ExpectedFrozen1271Commit + ':src-tauri/tauri.conf.json')
)
$normalConfigAtTarget = Invoke-Git -Arguments @(
    'show',
    ($targetCommit + ':src-tauri/tauri.conf.json')
)
if (($normalConfigAtFrozen -join "`n") -cne ($normalConfigAtTarget -join "`n")) {
    throw 'the normal Tauri configuration differs from the frozen #1271 baseline'
}

if ($RevisionPreflightOnly) {
    [pscustomobject]@{
        result = 'passed'
        stage = 'revision-preflight'
        targetCommit = $targetCommit
    } | ConvertTo-Json -Depth 3
    return
}

Invoke-Git -Arguments @('checkout', '--detach', $targetCommit) | Out-Null

$profilePath = Join-Path $RepoRoot $ProfileRelativePath
$overlayPath = Join-Path $RepoRoot $OverlayRelativePath
if (-not (Test-Path -LiteralPath $profilePath -PathType Leaf)) {
    throw "missing checked-in package profile: $profilePath"
}
if (-not (Test-Path -LiteralPath $overlayPath -PathType Leaf)) {
    throw "missing isolated validation overlay: $overlayPath"
}

$tauriCli = Join-Path $RepoRoot 'node_modules/.bin/tauri.cmd'
if (-not (Test-Path -LiteralPath $tauriCli -PathType Leaf)) {
    throw "missing Tauri CLI: $tauriCli"
}

$tauriBuild = Start-IsolatedValidationNativeProcess `
    -Mode Wait `
    -FilePath $tauriCli `
    -WorkingDirectory $RepoRoot `
    -Arguments @('build', '--features', $PackageFeature, '--config', $OverlayRelativePath) `
    -RemoveAgentsCommanderEnvironment
if ($tauriBuild.ExitCode -ne 0) {
    throw "isolated validation package build failed with exit code $($tauriBuild.ExitCode)"
}

function Get-IsolatedValidationDirectoryIdentity {
    param([Parameter(Mandatory)][string]$Path)

    if ($null -eq ('IsolatedValidationExtractionDirectoryIdentity' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationExtractionFileTime {
    public UInt32 Low;
    public UInt32 High;
}

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationExtractionByHandleFileInformation {
    public UInt32 FileAttributes;
    public IsolatedValidationExtractionFileTime CreationTime;
    public IsolatedValidationExtractionFileTime LastAccessTime;
    public IsolatedValidationExtractionFileTime LastWriteTime;
    public UInt32 VolumeSerialNumber;
    public UInt32 FileSizeHigh;
    public UInt32 FileSizeLow;
    public UInt32 NumberOfLinks;
    public UInt32 FileIndexHigh;
    public UInt32 FileIndexLow;
}

public static class IsolatedValidationExtractionDirectoryIdentity {
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
        out IsolatedValidationExtractionByHandleFileInformation information);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool SetFileInformationByHandle(
        SafeFileHandle file,
        Int32 informationClass,
        IntPtr information,
        UInt32 informationLength);

}

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationExtractionRenameInfo {
    public Byte ReplaceIfExists;
    public IntPtr RootDirectory;
    public UInt32 FileNameLength;
    public UInt16 FileNameFirstCharacter;
}

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationExtractionDispositionInfo {
    public Byte DeleteFile;
}
'@ -ErrorAction Stop
    }

    $handle = [IsolatedValidationExtractionDirectoryIdentity]::CreateFile(
        $Path,
        [uint32]0,
        [uint32]3,
        [IntPtr]::Zero,
        [uint32]3,
        [uint32]0x02200000,
        [IntPtr]::Zero
    )
    if ($handle.IsInvalid) {
        throw "could not open MSI administrative extraction directory: $Path"
    }

    try {
        $information = New-Object IsolatedValidationExtractionByHandleFileInformation
        if (-not [IsolatedValidationExtractionDirectoryIdentity]::GetFileInformationByHandle($handle, [ref]$information)) {
            throw "could not read MSI administrative extraction directory identity: $Path"
        }
        if (($information.FileAttributes -band [uint32]0x00000400) -ne 0) {
            throw "MSI administrative extraction directory must not be a reparse point: $Path"
        }

        return [pscustomobject]@{
            volumeSerialNumber = [uint32]$information.VolumeSerialNumber
            fileIndex = ([uint64]$information.FileIndexHigh * [uint64]4294967296) + [uint64]$information.FileIndexLow
        }
    }
    finally {
        $handle.Dispose()
    }
}

function Test-IsolatedValidationDirectoryIdentity {
    param(
        [Parameter(Mandatory)]$Expected,
        [Parameter(Mandatory)]$Actual
    )

    return $Expected.volumeSerialNumber -eq $Actual.volumeSerialNumber -and
        $Expected.fileIndex -eq $Actual.fileIndex
}

function Get-IsolatedValidationDirectoryIdentityFromHandle {
    param(
        [Parameter(Mandatory)]$Handle,
        [Parameter(Mandatory)][string]$Path
    )

    $information = New-Object IsolatedValidationExtractionByHandleFileInformation
    if (-not [IsolatedValidationExtractionDirectoryIdentity]::GetFileInformationByHandle($Handle, [ref]$information)) {
        throw "could not read MSI administrative extraction directory identity: $Path"
    }
    if (($information.FileAttributes -band [uint32]0x00000400) -ne 0) {
        throw "MSI administrative extraction directory must not be a reparse point: $Path"
    }

    return [pscustomobject]@{
        volumeSerialNumber = [uint32]$information.VolumeSerialNumber
        fileIndex = ([uint64]$information.FileIndexHigh * [uint64]4294967296) + [uint64]$information.FileIndexLow
        fileAttributes = [uint32]$information.FileAttributes
    }
}

function Initialize-IsolatedValidationExtractionCleanupNative {
    if ($null -ne ('IsolatedValidationExtractionCleanupNative' -as [type])) {
        return
    }

    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using Microsoft.Win32.SafeHandles;

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationExtractionCleanupUnicodeString {
    public UInt16 Length;
    public UInt16 MaximumLength;
    public IntPtr Buffer;
}

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationExtractionCleanupObjectAttributes {
    public UInt32 Length;
    public IntPtr RootDirectory;
    public IntPtr ObjectName;
    public UInt32 Attributes;
    public IntPtr SecurityDescriptor;
    public IntPtr SecurityQualityOfService;
}

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationExtractionCleanupIoStatusBlock {
    public IntPtr Status;
    public IntPtr Information;
}

[StructLayout(LayoutKind.Sequential)]
public struct IsolatedValidationExtractionCleanupFileIdBothDirectoryInformationHeader {
    public UInt32 NextEntryOffset;
    public UInt32 FileIndex;
    public Int64 CreationTime;
    public Int64 LastAccessTime;
    public Int64 LastWriteTime;
    public Int64 ChangeTime;
    public Int64 EndOfFile;
    public Int64 AllocationSize;
    public UInt32 FileAttributes;
    public UInt32 FileNameLength;
    public UInt32 EaSize;
    public Byte ShortNameLength;
    [MarshalAs(UnmanagedType.ByValArray, SizeConst = 12, ArraySubType = UnmanagedType.U2)]
    public UInt16[] ShortName;
    public UInt64 FileId;
    public UInt16 FileNameFirstCharacter;
}

public static class IsolatedValidationExtractionCleanupNative {
    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetFileInformationByHandleEx(
        SafeFileHandle file,
        Int32 informationClass,
        IntPtr information,
        UInt32 informationLength);

    [DllImport("ntdll.dll")]
    public static extern Int32 NtCreateFile(
        out IntPtr fileHandle,
        UInt32 desiredAccess,
        ref IsolatedValidationExtractionCleanupObjectAttributes objectAttributes,
        out IsolatedValidationExtractionCleanupIoStatusBlock ioStatusBlock,
        IntPtr allocationSize,
        UInt32 fileAttributes,
        UInt32 shareAccess,
        UInt32 createDisposition,
        UInt32 createOptions,
        IntPtr eaBuffer,
        UInt32 eaLength);

    [DllImport("ntdll.dll")]
    public static extern Int32 NtClose(IntPtr handle);

    public static SafeFileHandle CreateSafeFileHandle(IntPtr handle) {
        return new SafeFileHandle(handle, true);
    }
}
'@ -ErrorAction Stop
}

function Open-IsolatedValidationExtractionEntryLease {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][uint32]$DesiredAccess
    )

    if ($null -eq ('IsolatedValidationExtractionDirectoryIdentity' -as [type])) {
        Get-IsolatedValidationDirectoryIdentity -Path $Path | Out-Null
    }

    $handle = [IsolatedValidationExtractionDirectoryIdentity]::CreateFile(
        $Path,
        $DesiredAccess,
        [uint32]3,
        [IntPtr]::Zero,
        [uint32]3,
        [uint32]0x02200000,
        [IntPtr]::Zero
    )
    if ($handle.IsInvalid) {
        $handle.Dispose()
        throw "could not retain MSI administrative extraction directory handle: $Path"
    }

    try {
        return [pscustomobject]@{
            handle = $handle
            identity = Get-IsolatedValidationDirectoryIdentityFromHandle -Handle $handle -Path $Path
            path = $Path
        }
    }
    catch {
        $handle.Dispose()
        throw
    }
}

function Open-IsolatedValidationExtractionDirectoryLease {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][uint32]$DesiredAccess
    )

    return Open-IsolatedValidationExtractionEntryLease @PSBoundParameters
}

function Move-IsolatedValidationExtractionToQuarantine {
    param(
        [Parameter(Mandatory)]$DirectoryLease,
        [Parameter(Mandatory)]$ParentDirectoryLease,
        [Parameter(Mandatory)][string]$QuarantineLeaf
    )

    $quarantinePath = Join-Path $ParentDirectoryLease.path $QuarantineLeaf
    $quarantineNameBytes = [Text.Encoding]::Unicode.GetBytes($quarantinePath)
    $renameInfo = New-Object IsolatedValidationExtractionRenameInfo
    $renameInfo.ReplaceIfExists = [byte]0
    $renameInfo.RootDirectory = [IntPtr]::Zero
    $renameInfo.FileNameLength = [uint32]$quarantineNameBytes.Length
    $renameFileNameOffset = [Runtime.InteropServices.Marshal]::OffsetOf(
        [IsolatedValidationExtractionRenameInfo],
        'FileNameFirstCharacter'
    ).ToInt32()
    $renameInfoSize = [Runtime.InteropServices.Marshal]::SizeOf($renameInfo)
    $renameBufferLength = $renameInfoSize + $quarantineNameBytes.Length + 2
    $renameBuffer = [Runtime.InteropServices.Marshal]::AllocHGlobal($renameBufferLength)
    try {
        [Runtime.InteropServices.Marshal]::StructureToPtr($renameInfo, $renameBuffer, $false)
        [Runtime.InteropServices.Marshal]::Copy(
            $quarantineNameBytes,
            0,
            [IntPtr]::Add($renameBuffer, $renameFileNameOffset),
            $quarantineNameBytes.Length
        )
        [Runtime.InteropServices.Marshal]::WriteInt16(
            $renameBuffer,
            $renameFileNameOffset + $quarantineNameBytes.Length,
            [int16]0
        )
        if (-not [IsolatedValidationExtractionDirectoryIdentity]::SetFileInformationByHandle(
                $DirectoryLease.handle,
                3,
                $renameBuffer,
                [uint32]$renameBufferLength
            )) {
            $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
            throw "could not atomically quarantine MSI administrative extraction: $errorCode"
        }
    }
    finally {
        [Runtime.InteropServices.Marshal]::FreeHGlobal($renameBuffer)
    }

    $DirectoryLease.path = $quarantinePath
    return $quarantinePath
}

function Get-IsolatedValidationQuarantinedDirectoryEntries {
    param([Parameter(Mandatory)]$DirectoryLease)

    Initialize-IsolatedValidationExtractionCleanupNative

    # Query the held directory handle directly. This creates a stable entry
    # snapshot that is later bound to the child handle opened relative to this
    # same trusted parent, instead of re-resolving an absolute child path.
    $fileIdBothDirectoryInfo = 10
    $fileIdBothDirectoryRestartInfo = 11
    $errorNoMoreFiles = 18
    $bufferLength = 65536
    $buffer = [Runtime.InteropServices.Marshal]::AllocHGlobal($bufferLength)
    try {
        $fileAttributesOffset = [Runtime.InteropServices.Marshal]::OffsetOf(
            [IsolatedValidationExtractionCleanupFileIdBothDirectoryInformationHeader],
            'FileAttributes'
        ).ToInt32()
        $fileNameLengthOffset = [Runtime.InteropServices.Marshal]::OffsetOf(
            [IsolatedValidationExtractionCleanupFileIdBothDirectoryInformationHeader],
            'FileNameLength'
        ).ToInt32()
        $fileNameOffset = [Runtime.InteropServices.Marshal]::OffsetOf(
            [IsolatedValidationExtractionCleanupFileIdBothDirectoryInformationHeader],
            'FileNameFirstCharacter'
        ).ToInt32()
        $entryHeaderSize = [Runtime.InteropServices.Marshal]::SizeOf(
            (New-Object IsolatedValidationExtractionCleanupFileIdBothDirectoryInformationHeader)
        )
        if ($fileAttributesOffset -lt 0 -or
            $fileNameLengthOffset -lt $fileAttributesOffset -or
            $fileNameOffset -le $fileNameLengthOffset -or
            $entryHeaderSize -lt $fileNameOffset -or
            $entryHeaderSize -gt $bufferLength -or
            $fileNameOffset -ge $bufferLength) {
            throw 'could not determine safe FILE_ID_BOTH_DIR_INFORMATION field offsets'
        }

        $entries = New-Object System.Collections.ArrayList
        $informationClass = $fileIdBothDirectoryRestartInfo
        while ($true) {
            if (-not [IsolatedValidationExtractionCleanupNative]::GetFileInformationByHandleEx(
                    $DirectoryLease.handle,
                    $informationClass,
                    $buffer,
                    [uint32]$bufferLength
                )) {
                $errorCode = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
                if ($errorCode -eq $errorNoMoreFiles) {
                    break
                }
                throw "could not enumerate quarantined MSI administrative extraction by handle: $errorCode"
            }

            $entryOffset = 0
            while ($true) {
                if ($entryOffset -lt 0 -or $entryOffset -gt ($bufferLength - $entryHeaderSize)) {
                    throw 'received malformed FILE_ID_BOTH_DIR_INFORMATION entry offset'
                }

                $entryPointer = [IntPtr]::Add($buffer, $entryOffset)
                $entry = [Runtime.InteropServices.Marshal]::PtrToStructure(
                    $entryPointer,
                    [type][IsolatedValidationExtractionCleanupFileIdBothDirectoryInformationHeader]
                )
                $fileNameByteLength = [uint32]$entry.FileNameLength
                if (($fileNameByteLength -eq 0) -or
                    (($fileNameByteLength % 2) -ne 0) -or
                    $fileNameByteLength -gt ($bufferLength - $entryOffset - $fileNameOffset)) {
                    throw 'received malformed FILE_ID_BOTH_DIR_INFORMATION file name'
                }

                $entryName = [Runtime.InteropServices.Marshal]::PtrToStringUni(
                    [IntPtr]::Add($entryPointer, $fileNameOffset),
                    [int]($fileNameByteLength / 2)
                )
                if ([string]::IsNullOrEmpty($entryName)) {
                    throw 'received an empty FILE_ID_BOTH_DIR_INFORMATION file name'
                }
                if ($entryName -ne '.' -and $entryName -ne '..') {
                    if ($entryName.Contains([string][char]0) -or
                        $entryName.Contains('\\') -or
                        $entryName.Contains('/') -or
                        $entryName.Contains(':')) {
                        throw 'received a non-leaf FILE_ID_BOTH_DIR_INFORMATION file name'
                    }
                    [void]$entries.Add([pscustomobject]@{
                            name = $entryName
                            fileIndex = [uint64]$entry.FileId
                            fileAttributes = [uint32]$entry.FileAttributes
                        })
                }

                $nextEntryOffset = [uint64]$entry.NextEntryOffset
                if ($nextEntryOffset -eq 0) {
                    break
                }
                if (($nextEntryOffset % 8) -ne 0 -or
                    $nextEntryOffset -lt [uint64]$entryHeaderSize -or
                    $nextEntryOffset -gt [uint64]($bufferLength - $entryOffset)) {
                    throw 'received malformed FILE_ID_BOTH_DIR_INFORMATION next-entry offset'
                }
                $entryOffset += [int]$nextEntryOffset
            }

            $informationClass = $fileIdBothDirectoryInfo
        }

        return $entries.ToArray()
    }
    finally {
        [Runtime.InteropServices.Marshal]::FreeHGlobal($buffer)
    }
}

function Open-IsolatedValidationExtractionEntryLeaseRelative {
    param(
        [Parameter(Mandatory)]$ParentDirectoryLease,
        [Parameter(Mandatory)]$Entry
    )

    $entryName = [string]$Entry.name
    Initialize-IsolatedValidationExtractionCleanupNative

    if ([string]::IsNullOrEmpty($entryName) -or
        $entryName -eq '.' -or
        $entryName -eq '..' -or
        $entryName.Contains([string][char]0) -or
        $entryName.Contains('\\') -or
        $entryName.Contains('/') -or
        $entryName.Contains(':')) {
        throw 'refusing to open a non-leaf quarantined MSI administrative extraction entry'
    }
    if (([uint32]$Entry.fileAttributes -band [uint32]0x00000400) -ne 0) {
        throw "refusing to open a reparse-point quarantined MSI administrative extraction entry: $entryName"
    }

    $entryNameBytes = [Text.Encoding]::Unicode.GetBytes($entryName)
    if ($entryNameBytes.Length -gt ([UInt16]::MaxValue - 2)) {
        throw "quarantined MSI administrative extraction entry name is too long: $entryName"
    }

    $unicodeString = New-Object IsolatedValidationExtractionCleanupUnicodeString
    $unicodeString.Length = [uint16]$entryNameBytes.Length
    $unicodeString.MaximumLength = [uint16]($entryNameBytes.Length + 2)
    $unicodeString.Buffer = [Runtime.InteropServices.Marshal]::StringToHGlobalUni($entryName)
    $unicodeStringPointer = [IntPtr]::Zero
    $rawHandle = [IntPtr]::Zero
    $handle = $null
    try {
        $unicodeStringPointer = [Runtime.InteropServices.Marshal]::AllocHGlobal(
            [Runtime.InteropServices.Marshal]::SizeOf($unicodeString)
        )
        [Runtime.InteropServices.Marshal]::StructureToPtr($unicodeString, $unicodeStringPointer, $false)

        $objectAttributes = New-Object IsolatedValidationExtractionCleanupObjectAttributes
        $objectAttributes.Length = [uint32][Runtime.InteropServices.Marshal]::SizeOf($objectAttributes)
        $objectAttributes.RootDirectory = $ParentDirectoryLease.handle.DangerousGetHandle()
        $objectAttributes.ObjectName = $unicodeStringPointer
        $objectAttributes.Attributes = [uint32]0x00000040
        $ioStatusBlock = New-Object IsolatedValidationExtractionCleanupIoStatusBlock
        $desiredAccess = [uint32]0x00110080
        if (([uint32]$Entry.fileAttributes -band [uint32]0x00000010) -ne 0) {
            $desiredAccess = $desiredAccess -bor [uint32]0x00000001
        }

        $ntStatus = [IsolatedValidationExtractionCleanupNative]::NtCreateFile(
            [ref]$rawHandle,
            $desiredAccess,
            [ref]$objectAttributes,
            [ref]$ioStatusBlock,
            [IntPtr]::Zero,
            [uint32]0,
            [uint32]3,
            [uint32]1,
            [uint32]0x00204020,
            [IntPtr]::Zero,
            [uint32]0
        )
        if ($ntStatus -ne 0) {
            $hexStatus = '{0:X8}' -f ([uint32]$ntStatus)
            throw "could not retain quarantined MSI administrative extraction entry relative to its trusted parent: $entryName (NTSTATUS 0x$hexStatus)"
        }

        $handle = [IsolatedValidationExtractionCleanupNative]::CreateSafeFileHandle($rawHandle)
        $rawHandle = [IntPtr]::Zero
        if ($handle.IsInvalid) {
            $handle.Dispose()
            throw "could not retain quarantined MSI administrative extraction entry handle: $entryName"
        }

        $identity = Get-IsolatedValidationDirectoryIdentityFromHandle `
            -Handle $handle `
            -Path "relative entry '$entryName'"
        if ($identity.fileIndex -ne [uint64]$Entry.fileIndex) {
            throw "quarantined MSI administrative extraction entry identity changed after enumeration: $entryName"
        }
        $expectedDirectory = (([uint32]$Entry.fileAttributes -band [uint32]0x00000010) -ne 0)
        $actualDirectory = (($identity.fileAttributes -band [uint32]0x00000010) -ne 0)
        if ($expectedDirectory -ne $actualDirectory) {
            throw "quarantined MSI administrative extraction entry type changed after enumeration: $entryName"
        }

        return [pscustomobject]@{
            handle = $handle
            identity = $identity
            name = $entryName
        }
    }
    catch {
        if ($null -ne $handle) {
            $handle.Dispose()
        }
        throw
    }
    finally {
        if ($rawHandle -ne [IntPtr]::Zero) {
            [void][IsolatedValidationExtractionCleanupNative]::NtClose($rawHandle)
        }
        if ($unicodeStringPointer -ne [IntPtr]::Zero) {
            [Runtime.InteropServices.Marshal]::FreeHGlobal($unicodeStringPointer)
        }
        if ($unicodeString.Buffer -ne [IntPtr]::Zero) {
            [Runtime.InteropServices.Marshal]::FreeHGlobal($unicodeString.Buffer)
        }
    }
}

function Clear-IsolatedValidationQuarantinedDirectoryEntries {
    param(
        [Parameter(Mandatory)]$DirectoryLease,
        [Parameter(Mandatory)][AllowEmptyCollection()][object[]]$Entries
    )

    foreach ($entry in $Entries) {
        $childLease = $null
        try {
            $childLease = Open-IsolatedValidationExtractionEntryLeaseRelative `
                -ParentDirectoryLease $DirectoryLease `
                -Entry $entry
            if (($childLease.identity.fileAttributes -band [uint32]0x00000010) -ne 0) {
                Clear-IsolatedValidationQuarantinedDirectoryContents -DirectoryLease $childLease
            }
            Remove-IsolatedValidationExtractionEntryByHandle -EntryLease $childLease
        }
        finally {
            if ($null -ne $childLease) {
                $childLease.handle.Dispose()
            }
        }
    }
}

function Clear-IsolatedValidationQuarantinedDirectoryContents {
    param([Parameter(Mandatory)]$DirectoryLease)

    $entries = @(Get-IsolatedValidationQuarantinedDirectoryEntries -DirectoryLease $DirectoryLease)
    Clear-IsolatedValidationQuarantinedDirectoryEntries `
        -DirectoryLease $DirectoryLease `
        -Entries $entries
}

function Remove-IsolatedValidationExtractionEntryByHandle {
    param([Parameter(Mandatory)]$EntryLease)

    $dispositionInfo = New-Object IsolatedValidationExtractionDispositionInfo
    $dispositionInfo.DeleteFile = [byte]1
    $dispositionInfoSize = [Runtime.InteropServices.Marshal]::SizeOf($dispositionInfo)
    $dispositionBuffer = [Runtime.InteropServices.Marshal]::AllocHGlobal($dispositionInfoSize)
    try {
        [Runtime.InteropServices.Marshal]::StructureToPtr($dispositionInfo, $dispositionBuffer, $false)
        if (-not [IsolatedValidationExtractionDirectoryIdentity]::SetFileInformationByHandle(
                $EntryLease.handle,
                4,
                $dispositionBuffer,
                [uint32]$dispositionInfoSize
            )) {
            throw 'could not mark quarantined MSI administrative extraction for handle-bound deletion'
        }
    }
    finally {
        [Runtime.InteropServices.Marshal]::FreeHGlobal($dispositionBuffer)
    }
}

function Remove-IsolatedValidationDirectoryByHandle {
    param([Parameter(Mandatory)]$DirectoryLease)

    Remove-IsolatedValidationExtractionEntryByHandle -EntryLease $DirectoryLease
}

$releaseDirectory = Join-Path $RepoRoot 'target/release'
$executable = Get-ChildItem -LiteralPath $releaseDirectory -Filter '*.exe' -File |
    Where-Object { $_.BaseName -eq 'agentscommander' } |
    Select-Object -First 1
if ($null -eq $executable) {
    throw "could not locate the packaged executable below $releaseDirectory"
}

$msiDirectory = Join-Path $releaseDirectory 'bundle/msi'
$msiArtifacts = @(Get-ChildItem -LiteralPath $msiDirectory -Filter '*.msi' -File)
if ($msiArtifacts.Count -ne 1) {
    throw "expected exactly one isolated validation MSI below $msiDirectory; found $($msiArtifacts.Count)"
}
$extractionParentDirectory = $env:TEMP
if ([string]::IsNullOrWhiteSpace($extractionParentDirectory)) {
    $extractionParentDirectory = [System.IO.Path]::GetTempPath()
}
$installedRoot = Join-Path $extractionParentDirectory ('agentscommander-isolated-validation-installed-' + [Guid]::NewGuid().ToString('N'))
$installedPackageDirectory = Join-Path $installedRoot 'PFiles\Agents Commander Isolated Gates'
$msiExecutable = Join-Path $env:SystemRoot 'System32/msiexec.exe'
if (-not (Test-Path -LiteralPath $msiExecutable -PathType Leaf)) {
    throw "missing Windows Installer executable: $msiExecutable"
}
$extractionFailure = $null
$extractionParentDirectoryIdentity = Get-IsolatedValidationDirectoryIdentity -Path $extractionParentDirectory
$installedRootIdentity = $null
$installedRootLease = $null
$extractionParentDirectoryLease = $null
$quarantinePath = $null
try {
New-Item -ItemType Directory -Path $installedRoot -ErrorAction Stop | Out-Null
$installedRootIdentity = Get-IsolatedValidationDirectoryIdentity -Path $installedRoot
$administrativeInstall = Start-IsolatedValidationNativeProcess `
    -Mode Wait `
    -FilePath $msiExecutable `
    -WorkingDirectory $releaseDirectory `
    -Arguments @('/a', $msiArtifacts[0].FullName, '/qn', "TARGETDIR=$installedRoot") `
    -RemoveAgentsCommanderEnvironment
if ($administrativeInstall.ExitCode -ne 0) {
    throw "could not materialize the isolated validation MSI with exit code $($administrativeInstall.ExitCode)"
}
$installedExecutable = Join-Path $installedPackageDirectory 'agentscommander.exe'
$installedProfile = Join-Path $installedPackageDirectory 'package-profile.toml'
if (-not (Test-Path -LiteralPath $installedExecutable -PathType Leaf) -or
    -not (Test-Path -LiteralPath $installedProfile -PathType Leaf)) {
    throw 'the isolated validation MSI did not materialize its deterministic executable and profile resource layout'
}

# Stage the actual executable and exact resource bytes materialized by the MSI.
$artifactDirectory = Join-Path $releaseDirectory ('isolated-validation-portable-' + [Guid]::NewGuid().ToString('N'))
$artifactResources = Join-Path $artifactDirectory 'resources'
New-Item -ItemType Directory -Path $artifactResources -ErrorAction Stop | Out-Null
$artifactExecutable = Join-Path $artifactDirectory 'Agents Commander Isolated Gates.exe'
$artifactProfile = Join-Path $artifactResources 'package-profile.toml'
Copy-Item -LiteralPath $installedExecutable -Destination $artifactExecutable -ErrorAction Stop
Copy-Item -LiteralPath $installedProfile -Destination $artifactProfile -ErrorAction Stop

$launcherSource = Join-Path $RepoRoot 'packaging/isolated-validation/launch-isolated.ps1'
$launcherDestination = Join-Path $artifactDirectory 'launch-isolated.ps1'
$nativeProcessModuleSource = Join-Path $RepoRoot 'packaging/isolated-validation/native-process.psm1'
$nativeProcessModuleDestination = Join-Path $artifactDirectory 'native-process.psm1'
if (-not (Test-Path -LiteralPath $launcherSource -PathType Leaf) -or
    -not (Test-Path -LiteralPath $nativeProcessModuleSource -PathType Leaf)) {
    throw 'E_ISOLATION_NATIVE_PROCESS'
}
Copy-Item -LiteralPath $launcherSource -Destination $launcherDestination -ErrorAction Stop
Copy-Item -LiteralPath $nativeProcessModuleSource -Destination $nativeProcessModuleDestination -ErrorAction Stop

$manifestPath = Join-Path $artifactDirectory 'isolated-validation-manifest.json'
$profileHash = Get-Sha256 -LiteralPath $profilePath
$materializedProfileHash = Get-Sha256 -LiteralPath $installedProfile
$installedProfileHash = Get-Sha256 -LiteralPath $artifactProfile
if ($profileHash -cne $materializedProfileHash -or $materializedProfileHash -cne $installedProfileHash) {
    throw 'compiled and portable artifact profile bytes must be identical'
}
if ((Get-Sha256 -LiteralPath $launcherSource) -cne (Get-Sha256 -LiteralPath $launcherDestination) -or
    (Get-Sha256 -LiteralPath $nativeProcessModuleSource) -cne (Get-Sha256 -LiteralPath $nativeProcessModuleDestination)) {
    throw 'the staged launcher and native process module must be byte-identical copies'
}
}
catch {
    $extractionFailure = $_
    throw
}
finally {
    $cleanupFailure = $null
    try {
        if ($null -ne $installedRootIdentity -and (Test-Path -LiteralPath $installedRoot)) {
            $extractionParentDirectoryLease = Open-IsolatedValidationExtractionDirectoryLease `
                -Path $extractionParentDirectory `
                -DesiredAccess ([uint32]0x000000A0)
            if (-not (Test-IsolatedValidationDirectoryIdentity `
                    -Expected $extractionParentDirectoryIdentity `
                    -Actual $extractionParentDirectoryLease.identity)) {
                throw "MSI administrative extraction parent identity changed before cleanup: $extractionParentDirectory"
            }

            $installedRootLease = Open-IsolatedValidationExtractionDirectoryLease `
                -Path $installedRoot `
                -DesiredAccess ([uint32]0x00010081)
            $currentIdentity = $installedRootLease.identity
            if (-not (Test-IsolatedValidationDirectoryIdentity -Expected $installedRootIdentity -Actual $currentIdentity)) {
                throw "MSI administrative extraction identity changed before cleanup: $installedRoot"
            }

            $quarantineLeaf = '.isolated-validation-cleanup-' + [Guid]::NewGuid().ToString('N')
            $quarantinePath = Move-IsolatedValidationExtractionToQuarantine `
                -DirectoryLease $installedRootLease `
                -ParentDirectoryLease $extractionParentDirectoryLease `
                -QuarantineLeaf $quarantineLeaf

            Clear-IsolatedValidationQuarantinedDirectoryContents -DirectoryLease $installedRootLease
            Remove-IsolatedValidationDirectoryByHandle -DirectoryLease $installedRootLease
        }
        elseif (Test-Path -LiteralPath $installedRoot) {
            throw "refusing to remove MSI administrative extraction without its invocation-owned directory handle: $installedRoot"
        }
    }
    catch {
        $cleanupFailure = "could not remove MSI administrative extraction '$installedRoot': $($_.Exception.Message)"
    }
    finally {
        if ($null -ne $installedRootLease) {
            $installedRootLease.handle.Dispose()
            $installedRootLease = $null
        }
        if ($null -ne $extractionParentDirectoryLease) {
            $extractionParentDirectoryLease.handle.Dispose()
            $extractionParentDirectoryLease = $null
        }
    }

    if ($null -eq $cleanupFailure -and $null -ne $quarantinePath -and (Test-Path -LiteralPath $quarantinePath)) {
        $cleanupFailure = "MSI administrative extraction remains after handle-bound cleanup: $quarantinePath"
    }
    if ($null -ne $cleanupFailure) {
        if ($null -ne $extractionFailure) {
            Write-Warning $cleanupFailure
        }
        else {
            throw $cleanupFailure
        }
    }
}

$manifest = [ordered]@{
    schema = 'isolated-validation-handoff-v1'
    baseSha = (Invoke-Git -Arguments @('merge-base', $ExpectedFrozen1271Commit, $targetCommit)).Trim()
    frozen1271Commit = $ExpectedFrozen1271Commit
    isolatedStateRootCommit = $targetCommit
    combinedSourceSha = (Invoke-Git -Arguments @('rev-parse', 'HEAD')).Trim()
    combinedTreeSha = (Invoke-Git -Arguments @('rev-parse', 'HEAD^{tree}')).Trim()
    cleanWorktree = $true
    artifactKind = 'portable-layout'
    compiledProfileSha256 = $profileHash
    utcTimestamp = [DateTime]::UtcNow.ToString('o')
    mode = 'isolated-validation-package'
    target = $env:TARGET
    productLabel = 'Agents Commander Isolated Gates'
    bundleIdentifier = 'dev.agentscommander.isolatedgates'
    headerIdentity = 'WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated'
    launcherCommand = '.\launch-isolated.ps1 -FixtureRoot <absolute-fixture-root> -ExpectedManifestSha256 <trusted-hash>'
    payloads = [ordered]@{
        executable = [ordered]@{
            relativePath = 'Agents Commander Isolated Gates.exe'
            sha256 = Get-Sha256 -LiteralPath $artifactExecutable
        }
        profile = [ordered]@{
            relativePath = 'resources/package-profile.toml'
            sha256 = $installedProfileHash
        }
        launcher = [ordered]@{
            relativePath = 'launch-isolated.ps1'
            sha256 = Get-Sha256 -LiteralPath $launcherDestination
        }
        nativeProcessModule = [ordered]@{
            relativePath = 'native-process.psm1'
            sha256 = Get-Sha256 -LiteralPath $nativeProcessModuleDestination
        }
    }
}

$manifestJson = $manifest | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText($manifestPath, $manifestJson + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
$manifestHash = Get-Sha256 -LiteralPath $manifestPath

[pscustomobject]@{
    artifactDirectory = $artifactDirectory
    executable = $artifactExecutable
    manifest = $manifestPath
    manifestSha256 = $manifestHash
    profile = $artifactProfile
} | ConvertTo-Json -Depth 4
