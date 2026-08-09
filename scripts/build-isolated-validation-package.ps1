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
    }
}

function Open-IsolatedValidationExtractionDirectoryLease {
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

    return $quarantinePath
}

function Clear-IsolatedValidationQuarantinedDirectoryContents {
    param([Parameter(Mandatory)][string]$Path)

    foreach ($child in @(Get-ChildItem -LiteralPath $Path -Force -ErrorAction Stop)) {
        if (($child.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
            Remove-Item -LiteralPath $child.FullName -Force -ErrorAction Stop
        }
        elseif ($child.PSIsContainer) {
            Clear-IsolatedValidationQuarantinedDirectoryContents -Path $child.FullName
            Remove-Item -LiteralPath $child.FullName -Force -ErrorAction Stop
        }
        else {
            Remove-Item -LiteralPath $child.FullName -Force -ErrorAction Stop
        }
    }
}

function Remove-IsolatedValidationDirectoryByHandle {
    param([Parameter(Mandatory)]$DirectoryLease)

    $dispositionInfo = New-Object IsolatedValidationExtractionDispositionInfo
    $dispositionInfo.DeleteFile = [byte]1
    $dispositionInfoSize = [Runtime.InteropServices.Marshal]::SizeOf($dispositionInfo)
    $dispositionBuffer = [Runtime.InteropServices.Marshal]::AllocHGlobal($dispositionInfoSize)
    try {
        [Runtime.InteropServices.Marshal]::StructureToPtr($dispositionInfo, $dispositionBuffer, $false)
        if (-not [IsolatedValidationExtractionDirectoryIdentity]::SetFileInformationByHandle(
                $DirectoryLease.handle,
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
$installedRoot = Join-Path $releaseDirectory ('isolated-validation-installed-' + [Guid]::NewGuid().ToString('N'))
$installedPackageDirectory = Join-Path $installedRoot 'PFiles\Agents Commander Isolated Gates'
$msiExecutable = Join-Path $env:SystemRoot 'System32/msiexec.exe'
if (-not (Test-Path -LiteralPath $msiExecutable -PathType Leaf)) {
    throw "missing Windows Installer executable: $msiExecutable"
}
$extractionFailure = $null
$installedRootIdentity = $null
$installedRootLease = $null
$releaseDirectoryLease = $null
$quarantinePath = $null
try {
New-Item -ItemType Directory -Path $installedRoot -ErrorAction Stop | Out-Null
$releaseDirectoryLease = Open-IsolatedValidationExtractionDirectoryLease -Path $releaseDirectory -DesiredAccess ([uint32]0x000000A0)
$installedRootLease = Open-IsolatedValidationExtractionDirectoryLease -Path $installedRoot -DesiredAccess ([uint32]0x00010080)
$installedRootIdentity = $installedRootLease.identity
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
        if ($null -ne $installedRootLease -and $null -ne $installedRootIdentity -and $null -ne $releaseDirectoryLease) {
            $currentIdentity = Get-IsolatedValidationDirectoryIdentityFromHandle -Handle $installedRootLease.handle -Path $installedRoot
            if (-not (Test-IsolatedValidationDirectoryIdentity -Expected $installedRootIdentity -Actual $currentIdentity)) {
                throw "MSI administrative extraction identity changed before cleanup: $installedRoot"
            }

            $quarantineLeaf = '.isolated-validation-cleanup-' + [Guid]::NewGuid().ToString('N')
            $quarantinePath = Move-IsolatedValidationExtractionToQuarantine `
                -DirectoryLease $installedRootLease `
                -ParentDirectoryLease $releaseDirectoryLease `
                -QuarantineLeaf $quarantineLeaf

            Clear-IsolatedValidationQuarantinedDirectoryContents -Path $quarantinePath
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
        if ($null -ne $releaseDirectoryLease) {
            $releaseDirectoryLease.handle.Dispose()
            $releaseDirectoryLease = $null
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
