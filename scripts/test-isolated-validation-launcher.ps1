[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidateSet('Desktop', 'Core')]
    [string]$ExpectedPSEdition,

    [int]$ExpectedMajorVersion,

    [int]$MinimumMajorVersion
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($PSVersionTable.PSEdition -cne $ExpectedPSEdition) {
    throw "expected PowerShell edition $ExpectedPSEdition, received $($PSVersionTable.PSEdition)"
}
if ($ExpectedMajorVersion -gt 0 -and $PSVersionTable.PSVersion.Major -ne $ExpectedMajorVersion) {
    throw "expected PowerShell major version $ExpectedMajorVersion, received $($PSVersionTable.PSVersion.Major)"
}
if ($MinimumMajorVersion -gt 0 -and $PSVersionTable.PSVersion.Major -lt $MinimumMajorVersion) {
    throw "expected PowerShell major version at least $MinimumMajorVersion, received $($PSVersionTable.PSVersion.Major)"
}
if (($ExpectedMajorVersion -gt 0) -eq ($MinimumMajorVersion -gt 0)) {
    throw 'supply exactly one of -ExpectedMajorVersion or -MinimumMajorVersion'
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string]$LiteralPath)

    return (Get-FileHash -LiteralPath $LiteralPath -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Write-JsonFile {
    param(
        [Parameter(Mandatory)][string]$LiteralPath,
        [Parameter(Mandatory)]$Value
    )

    [System.IO.File]::WriteAllText(
        $LiteralPath,
        ($Value | ConvertTo-Json -Depth 10) + [Environment]::NewLine,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Get-NativeExecutable {
    param([Parameter(Mandatory)][string]$Name)

    $command = Get-Command -Name $Name -CommandType Application -ErrorAction Stop
    $path = [string]$command.Source
    if ([string]::IsNullOrWhiteSpace($path)) {
        $path = [string]$command.Path
    }
    if ([string]::IsNullOrWhiteSpace($path) -or -not [System.IO.File]::Exists($path)) {
        throw "required native executable is unavailable: $Name"
    }
    return $path
}

function ConvertFrom-NativeJsonArray {
    param([Parameter(Mandatory)][string]$Json)

    $decoded = $Json | ConvertFrom-Json
    if ($decoded -is [System.Array]) {
        foreach ($item in $decoded) {
            Write-Output -NoEnumerate $item
        }
        return
    }

    Write-Output -NoEnumerate $decoded
}

function Assert-NativeProcessFailure {
    param(
        [Parameter(Mandatory)][scriptblock]$Action,
        [Parameter(Mandatory)][string]$CaseName
    )

    $failed = $false
    try {
        & $Action
    }
    catch {
        if ($_.Exception.Message -ne 'E_ISOLATION_NATIVE_PROCESS') {
            throw
        }
        $failed = $true
    }
    if (-not $failed) {
        throw "$CaseName unexpectedly succeeded"
    }
}

function Invoke-Launcher {
    param(
        [Parameter(Mandatory)][string]$Launcher,
        [Parameter(Mandatory)][string]$FixtureRoot,
        [Parameter(Mandatory)][string]$ExpectedManifestSha256,
        [switch]$IncludeVerboseOutput
    )

    $output = @()
    $scriptSucceeded = $false
    try {
        if ($IncludeVerboseOutput.IsPresent) {
            & $Launcher -FixtureRoot $FixtureRoot -ExpectedManifestSha256 $ExpectedManifestSha256 -Verbose 4>&1 2>&1 |
                ForEach-Object { $output += $_ }
        }
        else {
            $output = & $Launcher -FixtureRoot $FixtureRoot -ExpectedManifestSha256 $ExpectedManifestSha256 2>&1
        }
        $scriptSucceeded = $?
        return [pscustomobject]@{
            Succeeded = $scriptSucceeded
            Output = ($output -join [Environment]::NewLine)
        }
    }
    catch {
        if ($IncludeVerboseOutput.IsPresent) {
            $output += $_
        }
        return [pscustomobject]@{
            Succeeded = $false
            Output = if ($IncludeVerboseOutput.IsPresent) { $output -join [Environment]::NewLine } else { $_.Exception.Message }
        }
    }
}

function New-IsolatedValidationKillOnCloseJob {
    if ($null -eq ('IsolatedValidationKillOnCloseJob' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;

public sealed class IsolatedValidationKillOnCloseJob : IDisposable {
    [StructLayout(LayoutKind.Sequential)]
    private struct IoCounters {
        public UInt64 ReadOperationCount;
        public UInt64 WriteOperationCount;
        public UInt64 OtherOperationCount;
        public UInt64 ReadTransferCount;
        public UInt64 WriteTransferCount;
        public UInt64 OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct BasicLimitInformation {
        public Int64 PerProcessUserTimeLimit;
        public Int64 PerJobUserTimeLimit;
        public UInt32 LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public UInt32 ActiveProcessLimit;
        public UIntPtr Affinity;
        public UInt32 PriorityClass;
        public UInt32 SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct ExtendedLimitInformation {
        public BasicLimitInformation BasicLimitInformation;
        public IoCounters IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateJobObject(IntPtr jobAttributes, string name);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetInformationJobObject(
        IntPtr job,
        Int32 informationClass,
        IntPtr information,
        UInt32 informationLength);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool CloseHandle(IntPtr handle);

    private const UInt32 JobObjectLimitKillOnJobClose = 0x00002000;
    private const Int32 JobObjectExtendedLimitInformation = 9;
    private IntPtr handle;

    private IsolatedValidationKillOnCloseJob(IntPtr handle) {
        this.handle = handle;
    }

    public static IsolatedValidationKillOnCloseJob Create() {
        IntPtr handle = CreateJobObject(IntPtr.Zero, null);
        if (handle == IntPtr.Zero || handle == new IntPtr(-1)) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "could not create test job");
        }

        var job = new IsolatedValidationKillOnCloseJob(handle);
        try {
            var information = new ExtendedLimitInformation();
            information.BasicLimitInformation.LimitFlags = JobObjectLimitKillOnJobClose;
            Int32 length = Marshal.SizeOf(typeof(ExtendedLimitInformation));
            IntPtr memory = Marshal.AllocHGlobal(length);
            try {
                Marshal.StructureToPtr(information, memory, false);
                if (!SetInformationJobObject(
                    job.handle,
                    JobObjectExtendedLimitInformation,
                    memory,
                    (UInt32)length)) {
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "could not configure test job");
                }
            }
            finally {
                Marshal.FreeHGlobal(memory);
            }
            return job;
        }
        catch {
            job.Dispose();
            throw;
        }
    }

    public void Assign(IntPtr processHandle) {
        if (this.handle == IntPtr.Zero) {
            throw new ObjectDisposedException("IsolatedValidationKillOnCloseJob");
        }
        if (!AssignProcessToJobObject(this.handle, processHandle)) {
            throw new Win32Exception(Marshal.GetLastWin32Error(), "could not assign launcher wrapper to test job");
        }
    }

    public void Dispose() {
        if (this.handle != IntPtr.Zero) {
            CloseHandle(this.handle);
            this.handle = IntPtr.Zero;
        }
        GC.SuppressFinalize(this);
    }
}
'@ -ErrorAction Stop
    }

    return [IsolatedValidationKillOnCloseJob]::Create()
}

function Start-LauncherUnderTestJob {
    param(
        [Parameter(Mandatory)][string]$Launcher,
        [Parameter(Mandatory)][string]$FixtureRoot,
        [Parameter(Mandatory)][string]$ExpectedManifestSha256,
        [Parameter(Mandatory)][string]$TestRoot
    )

    $hostExecutable = if ($PSVersionTable.PSEdition -eq 'Core') {
        Join-Path $PSHOME 'pwsh.exe'
    }
    else {
        Join-Path $PSHOME 'powershell.exe'
    }
    if (-not (Test-Path -LiteralPath $hostExecutable -PathType Leaf)) {
        throw "missing current PowerShell host executable: $hostExecutable"
    }

    $wrapper = Join-Path $TestRoot 'job-owned-launcher-wrapper.ps1'
    $startGate = Join-Path $TestRoot 'job-owned-launcher-start-gate'
    $resultPath = Join-Path $TestRoot 'job-owned-launcher-result.json'
    $failurePath = Join-Path $TestRoot 'job-owned-launcher-failure.txt'
    [System.IO.File]::WriteAllText($wrapper, @'
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$StartGate,
    [Parameter(Mandatory)][string]$Launcher,
    [Parameter(Mandatory)][string]$FixtureRoot,
    [Parameter(Mandatory)][string]$ExpectedManifestSha256,
    [Parameter(Mandatory)][string]$ResultPath,
    [Parameter(Mandatory)][string]$FailurePath
)

$ErrorActionPreference = 'Stop'
$deadline = [DateTime]::UtcNow.AddSeconds(15)
while (-not (Test-Path -LiteralPath $StartGate -PathType Leaf)) {
    if ([DateTime]::UtcNow -ge $deadline) {
        throw 'test job start gate was not released'
    }
    Start-Sleep -Milliseconds 10
}

try {
    $launchOutput = @(& $Launcher -FixtureRoot $FixtureRoot -ExpectedManifestSha256 $ExpectedManifestSha256)
    [System.IO.File]::WriteAllText(
        $ResultPath,
        ($launchOutput -join [Environment]::NewLine),
        [System.Text.UTF8Encoding]::new($false)
    )
}
catch {
    [System.IO.File]::WriteAllText(
        $FailurePath,
        $_.Exception.Message,
        [System.Text.UTF8Encoding]::new($false)
    )
    exit 1
}
'@, [System.Text.UTF8Encoding]::new($false))

    $job = New-IsolatedValidationKillOnCloseJob
    $wrapperProcess = $null
    try {
        $wrapperLease = Start-IsolatedValidationNativeProcess `
            -Mode Start `
            -FilePath $hostExecutable `
            -WorkingDirectory $TestRoot `
            -Arguments @(
                '-NoProfile',
                '-ExecutionPolicy',
                'Bypass',
                '-File',
                $wrapper,
                '-StartGate',
                $startGate,
                '-Launcher',
                $Launcher,
                '-FixtureRoot',
                $FixtureRoot,
                '-ExpectedManifestSha256',
                $ExpectedManifestSha256,
                '-ResultPath',
                $resultPath,
                '-FailurePath',
                $failurePath
            ) `
            -RemoveAgentsCommanderEnvironment
        $wrapperProcess = $wrapperLease.Process
        if ($null -eq $wrapperProcess) {
            throw 'job-owned launcher wrapper did not return an original process lease'
        }
        $job.Assign($wrapperProcess.Handle)
        [System.IO.File]::WriteAllText($startGate, 'start', [System.Text.UTF8Encoding]::new($false))
        if (-not $wrapperProcess.WaitForExit(30000)) {
            throw 'job-owned launcher wrapper did not exit'
        }
        if ($wrapperProcess.ExitCode -ne 0) {
            $failure = if (Test-Path -LiteralPath $failurePath -PathType Leaf) {
                [System.IO.File]::ReadAllText($failurePath, [System.Text.UTF8Encoding]::new($false))
            }
            else {
                'no wrapper failure text was written'
            }
            throw "job-owned launcher wrapper failed: $failure"
        }
        if (-not (Test-Path -LiteralPath $resultPath -PathType Leaf)) {
            throw 'job-owned launcher wrapper did not write a JSON launch result'
        }

        return [pscustomobject]@{
            Launch = [System.IO.File]::ReadAllText($resultPath, [System.Text.UTF8Encoding]::new($false)) |
                ConvertFrom-Json -ErrorAction Stop
            WrapperProcess = $wrapperProcess
            Job = $job
        }
    }
    catch {
        if ($null -ne $wrapperProcess) {
            try {
                if (-not $wrapperProcess.HasExited) {
                    $wrapperProcess.Kill()
                    $null = $wrapperProcess.WaitForExit(3000)
                }
            }
            finally {
                $wrapperProcess.Dispose()
            }
        }
        $job.Dispose()
        throw
    }
}

function Invoke-IsolatedValidationExtractionCleanupRegression {
    param(
        [Parameter(Mandatory)][string]$Builder,
        [Parameter(Mandatory)][string]$TestRoot
    )

    $builderSource = [System.IO.File]::ReadAllText($Builder)
    $functionStart = $builderSource.IndexOf(
        'function Get-IsolatedValidationDirectoryIdentity {',
        [System.StringComparison]::Ordinal
    )
    $mainStart = $builderSource.IndexOf(
        '$releaseDirectory = Join-Path $RepoRoot',
        [System.StringComparison]::Ordinal
    )
    if ($functionStart -lt 0 -or $mainStart -le $functionStart) {
        throw 'could not load the production extraction cleanup primitives for regression coverage'
    }
    if ($builderSource -match 'Remove-Item -LiteralPath \$installedRoot -Recurse') {
        throw 'production MSI cleanup must not recursively delete the public administrative extraction path'
    }

    . ([scriptblock]::Create($builderSource.Substring($functionStart, $mainStart - $functionStart)))

    $regressionRoot = Join-Path $TestRoot ('extraction-cleanup-regression-' + [Guid]::NewGuid().ToString('N'))
    $releaseDirectory = Join-Path $regressionRoot 'release'
    $installedRoot = Join-Path $releaseDirectory 'invocation-owned'
    $foreignTarget = Join-Path $regressionRoot 'foreign-target'
    $quarantinePath = $null
    $installedRootLease = $null
    $releaseDirectoryLease = $null
    try {
        New-Item -ItemType Directory -Path $installedRoot -Force -ErrorAction Stop | Out-Null
        New-Item -ItemType Directory -Path (Join-Path $installedRoot 'payload') -Force -ErrorAction Stop | Out-Null
        [System.IO.File]::WriteAllText((Join-Path $installedRoot 'payload\owned.txt'), 'owned')

        $releaseDirectoryLease = Open-IsolatedValidationExtractionDirectoryLease `
            -Path $releaseDirectory `
            -DesiredAccess ([uint32]0x000000A0)
        $installedRootLease = Open-IsolatedValidationExtractionDirectoryLease `
            -Path $installedRoot `
            -DesiredAccess ([uint32]0x00010080)
        $expectedIdentity = $installedRootLease.identity
        $actualIdentity = Get-IsolatedValidationDirectoryIdentityFromHandle `
            -Handle $installedRootLease.handle `
            -Path $installedRoot
        if (-not (Test-IsolatedValidationDirectoryIdentity -Expected $expectedIdentity -Actual $actualIdentity)) {
            throw 'the retained administrative extraction handle did not retain its invocation-owned identity'
        }

        $quarantineLeaf = '.isolated-validation-cleanup-regression-' + [Guid]::NewGuid().ToString('N')
        $quarantinePath = Move-IsolatedValidationExtractionToQuarantine `
            -DirectoryLease $installedRootLease `
            -ParentDirectoryLease $releaseDirectoryLease `
            -QuarantineLeaf $quarantineLeaf

        New-Item -ItemType Directory -Path $foreignTarget -Force -ErrorAction Stop | Out-Null
        $foreignSentinel = Join-Path $foreignTarget 'must-survive.txt'
        [System.IO.File]::WriteAllText($foreignSentinel, 'foreign')
        New-Item -ItemType Junction -Path $installedRoot -Target $foreignTarget -ErrorAction Stop | Out-Null

        Clear-IsolatedValidationQuarantinedDirectoryContents -Path $quarantinePath
        Remove-IsolatedValidationDirectoryByHandle -DirectoryLease $installedRootLease
    }
    finally {
        if ($null -ne $installedRootLease) {
            $installedRootLease.handle.Dispose()
        }
        if ($null -ne $releaseDirectoryLease) {
            $releaseDirectoryLease.handle.Dispose()
        }
    }

    if ($null -eq $quarantinePath -or (Test-Path -LiteralPath $quarantinePath)) {
        throw 'handle-bound MSI cleanup did not remove the quarantined invocation-owned directory'
    }
    if (-not (Test-Path -LiteralPath (Join-Path $installedRoot 'must-survive.txt'))) {
        throw 'handle-bound MSI cleanup removed a replacement or reparse substitution at the public extraction path'
    }
}

function Invoke-IsolatedValidationTestRootCleanup {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][int]$RetryLimit,
        $PrimaryFailure,
        [Parameter(Mandatory)][scriptblock]$RemoveAction
    )

    $succeeded = $false
    $failureMessage = $null
    $attempts = 0
    for ($attempt = 1; $attempt -le $RetryLimit; $attempt++) {
        $attempts = $attempt
        try {
            & $RemoveAction $Path
            if (-not (Test-Path -LiteralPath $Path)) {
                $succeeded = $true
                break
            }
            $failureMessage = 'the test root remained after a successful removal call'
        }
        catch {
            $failureMessage = $_.Exception.Message
        }
        if ($attempt -lt $RetryLimit) {
            Start-Sleep -Milliseconds 100
        }
    }

    if ($succeeded) {
        return [pscustomobject]@{
            Succeeded = $true
            Attempts = $attempts
            CleanupMessage = $null
        }
    }

    $cleanupMessage = "failed to remove isolated launcher test root after $RetryLimit attempts: $failureMessage"
    if ($null -ne $PrimaryFailure) {
        Write-Warning "$cleanupMessage; preserving the primary test failure"
        return [pscustomobject]@{
            Succeeded = $false
            Attempts = $attempts
            CleanupMessage = $cleanupMessage
        }
    }

    throw $cleanupMessage
}

function Wait-ForProcessExit {
    param(
        [Parameter(Mandatory)][int]$ProcessId,
        [Parameter(Mandatory)][string]$CaseName
    )

    for ($attempt = 1; $attempt -le 160; $attempt++) {
        if ($null -eq (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
            return
        }
        Start-Sleep -Milliseconds 25
    }
    throw "$CaseName process $ProcessId did not exit"
}

function Assert-LauncherFailsBeforeChild {
    param(
        [Parameter(Mandatory)][string]$Launcher,
        [Parameter(Mandatory)][string]$FixtureRoot,
        [Parameter(Mandatory)][string]$ExpectedManifestSha256,
        [Parameter(Mandatory)][string]$ChildSentinel,
        [Parameter(Mandatory)][string]$CaseName,
        [string]$StatusChildSentinel,
        [switch]$ExpectStatusChild
    )

    if (Test-Path -LiteralPath $ChildSentinel) {
        Remove-Item -LiteralPath $ChildSentinel -Force
    }
    if (-not [string]::IsNullOrWhiteSpace($StatusChildSentinel) -and
        (Test-Path -LiteralPath $StatusChildSentinel)) {
        Remove-Item -LiteralPath $StatusChildSentinel -Force
    }
    $result = Invoke-Launcher -Launcher $Launcher -FixtureRoot $FixtureRoot -ExpectedManifestSha256 $ExpectedManifestSha256
    if ($result.Succeeded) {
        throw "$CaseName unexpectedly launched: $($result.Output)"
    }
    if (Test-Path -LiteralPath $ChildSentinel) {
        throw "$CaseName started a child before rejecting the input"
    }
    if ($ExpectStatusChild.IsPresent) {
        if ([string]::IsNullOrWhiteSpace($StatusChildSentinel) -or
            -not (Test-Path -LiteralPath $StatusChildSentinel)) {
            throw "$CaseName did not run status before rejecting its dynamic root identity"
        }
    }
    elseif (-not [string]::IsNullOrWhiteSpace($StatusChildSentinel) -and
        (Test-Path -LiteralPath $StatusChildSentinel)) {
        throw "$CaseName started its status child before rejecting the input"
    }
}

function Assert-PowerShellAstContract {
    param([Parameter(Mandatory)][string[]]$ProductionPaths)

    foreach ($path in $ProductionPaths) {
        $tokens = $null
        $parseErrors = $null
        $ast = [System.Management.Automation.Language.Parser]::ParseFile(
            $path,
            [ref]$tokens,
            [ref]$parseErrors
        )
        if ($parseErrors.Count -gt 0) {
            throw "production PowerShell AST has parse errors: $path"
        }

        $members = @($ast.FindAll({
                    param($node)
                    $node -is [System.Management.Automation.Language.MemberExpressionAst]
                }, $true))
        foreach ($member in $members) {
            $name = $member.Member.Extent.Text
            if ($name -ceq 'ArgumentList' -or $name -ceq 'Environment') {
                throw "forbidden native-process member $name in $path"
            }
        }

        $commands = @($ast.FindAll({
                    param($node)
                    $node -is [System.Management.Automation.Language.CommandAst]
                }, $true))
        foreach ($command in $commands) {
            $name = $command.GetCommandName()
            if ($name -in @('Start-Process', 'Stop-Process', 'Invoke-Expression', 'cmd.exe')) {
                throw "forbidden production command $name in $path"
            }
        }
    }
}

function Invoke-DetachedPackageBuildHandoff {
    param(
        [Parameter(Mandatory)][string]$RepositoryRoot,
        [Parameter(Mandatory)][string]$TestRoot,
        [Parameter(Mandatory)][string]$GitExecutable,
        [Parameter(Mandatory)][string]$NpmExecutable,
        [Parameter(Mandatory)][string]$Frozen1271Commit,
        [Parameter(Mandatory)][string]$IsolatedStateRootCommit
    )

    $head = (Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $GitExecutable `
        -WorkingDirectory $RepositoryRoot `
        -Arguments @('rev-parse', 'HEAD^{commit}') `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment).StandardOutput.Trim()
    if ($head -notmatch '^[0-9a-f]{40}$') {
        throw 'real Git did not return a full detached-checkout revision'
    }

    $checkout = Join-Path $TestRoot 'detached-package-build-checkout'
    $created = $false
    $handoff = $null
    try {
        $add = Start-IsolatedValidationNativeProcess `
            -Mode Wait `
            -FilePath $GitExecutable `
            -WorkingDirectory $RepositoryRoot `
            -Arguments @('worktree', 'add', '--detach', $checkout, $head) `
            -RemoveAgentsCommanderEnvironment
        if ($add.ExitCode -ne 0) {
            throw 'could not create detached preflight checkout'
        }
        $created = $true

        $tree = (Start-IsolatedValidationNativeProcess `
            -Mode CaptureAndWait `
            -FilePath $GitExecutable `
            -WorkingDirectory $checkout `
            -Arguments @('rev-parse', 'HEAD^{tree}') `
            -StandardOutputLimitBytes 1MB `
            -StandardErrorLimitBytes 1MB `
            -RemoveAgentsCommanderEnvironment).StandardOutput.Trim()
        $fixtureCommit = (Start-IsolatedValidationNativeProcess `
            -Mode CaptureAndWait `
            -FilePath $GitExecutable `
            -WorkingDirectory $checkout `
            -Arguments @(
                '-c', 'user.name=isolated-validation-test',
                '-c', 'user.email=isolated-validation-test@example.invalid',
                'commit-tree', $tree,
                '-p', $head,
                '-p', $Frozen1271Commit,
                '-m', 'isolated validation detached preflight fixture'
            ) `
            -StandardOutputLimitBytes 1MB `
            -StandardErrorLimitBytes 1MB `
            -RemoveAgentsCommanderEnvironment).StandardOutput.Trim()
        if ($fixtureCommit -notmatch '^[0-9a-f]{40}$') {
            throw 'could not create detached preflight Git fixture revision'
        }
        $reset = Start-IsolatedValidationNativeProcess `
            -Mode Wait `
            -FilePath $GitExecutable `
            -WorkingDirectory $checkout `
            -Arguments @('reset', '--hard', $fixtureCommit) `
            -RemoveAgentsCommanderEnvironment
        if ($reset.ExitCode -ne 0) {
            throw 'could not switch detached preflight checkout to its fixture revision'
        }

        $installDependencies = Start-IsolatedValidationNativeProcess `
            -Mode Wait `
            -FilePath $NpmExecutable `
            -WorkingDirectory $checkout `
            -Arguments @('ci', '--no-audit', '--no-fund') `
            -RemoveAgentsCommanderEnvironment
        if ($installDependencies.ExitCode -ne 0) {
            throw 'could not install detached package build dependencies'
        }

        $build = Join-Path $checkout 'scripts/build-isolated-validation-package.ps1'
        $preflightOutput = @(& $build `
            -Frozen1271Commit $Frozen1271Commit `
            -IsolatedStateRootCommit $fixtureCommit `
            -RevisionPreflightOnly 2>&1)
        if (-not $?) {
            throw 'detached build revision preflight failed'
        }

        $buildOutput = @(& $build `
            -Frozen1271Commit $Frozen1271Commit `
            -IsolatedStateRootCommit $fixtureCommit 2>&1)
        if (-not $?) {
            throw 'real detached package build failed'
        }
        try {
            $buildHandoff = ($buildOutput -join [Environment]::NewLine) | ConvertFrom-Json
        }
        catch {
            throw 'real detached package build did not return a handoff JSON object'
        }

        $artifactSource = [System.IO.Path]::GetFullPath([string]$buildHandoff.artifactDirectory)
        $checkoutPrefix = [System.IO.Path]::GetFullPath($checkout).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
        if (-not $artifactSource.StartsWith($checkoutPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw 'real detached package build returned an artifact outside its detached checkout'
        }
        $handoffDirectory = Join-Path $TestRoot 'actual-detached-package-handoff'
        if (Test-Path -LiteralPath $handoffDirectory) {
            throw 'real detached package handoff destination already exists'
        }
        [System.IO.Directory]::Move($artifactSource, $handoffDirectory)
        $handoff = [pscustomobject]@{
            artifactDirectory = $handoffDirectory
            executable = Join-Path $handoffDirectory 'Agents Commander Isolated Gates.exe'
            manifest = Join-Path $handoffDirectory 'isolated-validation-manifest.json'
            manifestSha256 = [string]$buildHandoff.manifestSha256
            profile = Join-Path $handoffDirectory 'resources/package-profile.toml'
        }
    }
    finally {
        if ($created) {
            $remove = Start-IsolatedValidationNativeProcess `
                -Mode Wait `
                -FilePath $GitExecutable `
                -WorkingDirectory $RepositoryRoot `
                -Arguments @('worktree', 'remove', '--force', $checkout) `
                -RemoveAgentsCommanderEnvironment
            if ($remove.ExitCode -ne 0) {
                throw 'could not remove detached preflight checkout'
            }
        }
    }

    return $handoff
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$moduleSource = Join-Path $repoRoot 'packaging/isolated-validation/native-process.psm1'
$launcherSource = Join-Path $repoRoot 'packaging/isolated-validation/launch-isolated.ps1'
$profileSource = Join-Path $repoRoot 'packaging/isolated-validation/package-profile.toml'
$fixtureSource = Join-Path $repoRoot 'packaging/isolated-validation/test-native-fixture.rs'
$buildScript = Join-Path $repoRoot 'scripts/build-isolated-validation-package.ps1'
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('iv-' + [Guid]::NewGuid().ToString('N').Substring(0, 8))
$frozen1271Commit = 'd68495086e168e5258500832b2ef45b4337ed21a'
$stage = 'initialization'
$testRootCleanupRetryLimit = 60
$primaryFailure = $null
$savedParentEnvironment = @{}
foreach ($name in @(
        'AGENTSCOMMANDER_TEST_NATIVE_PARENT',
        'AGENTSCOMMANDER_TEST_LAUNCHER_INHERITED',
        'ISOLATED_VALIDATION_TEST_PROFILE_HASH',
        'ISOLATED_VALIDATION_TEST_RECEIPT_COLLISION',
        'ISOLATED_VALIDATION_TEST_GUI_PID_PATH',
        'GIT_DIR',
        'GIT_WORK_TREE',
        'GIT_INDEX_FILE',
        'GIT_COMMON_DIR'
    )) {
    $savedParentEnvironment[$name] = [Environment]::GetEnvironmentVariable($name)
}

foreach ($name in @('GIT_DIR', 'GIT_WORK_TREE', 'GIT_INDEX_FILE', 'GIT_COMMON_DIR')) {
    Remove-Item -LiteralPath ("Env:$name") -ErrorAction SilentlyContinue
}

try {
    New-Item -ItemType Directory -Path $testRoot -Force | Out-Null
    Assert-PowerShellAstContract -ProductionPaths @($moduleSource, $buildScript, $launcherSource)

    $nativeProcessModule = Import-Module -Name $moduleSource -Force -PassThru -ErrorAction Stop
    $exported = @(Get-Command -Module native-process | Select-Object -ExpandProperty Name)
    if ($exported.Count -ne 1 -or $exported[0] -cne 'Start-IsolatedValidationNativeProcess') {
        throw "native process module exported unexpected commands: $($exported -join ', ')"
    }
    foreach ($pathVector in @(
        [pscustomobject]@{ Path = $repoRoot; Expected = $true },
        [pscustomobject]@{ Path = 'relative-path'; Expected = $false },
        [pscustomobject]@{ Path = 'C:drive-relative'; Expected = $false },
        [pscustomobject]@{ Path = '\\root-relative'; Expected = $false }
    )) {
        $pathVectorResult = & $nativeProcessModule {
            param($Path)
            Test-IsolatedValidationFullyQualifiedPath -Path $Path
        } $pathVector.Path
        if ($pathVectorResult -ne $pathVector.Expected) {
            throw "shared fully-qualified path policy rejected its conformance vector: $($pathVector.Path)"
        }
    }
    $launcherAst = [System.Management.Automation.Language.Parser]::ParseFile(
        $launcherSource,
        [ref]$null,
        [ref]$null
    )
    $launcherLocalPathValidator = $launcherAst.FindAll({
            param($ast)
            $ast -is [System.Management.Automation.Language.FunctionDefinitionAst] -and
                $ast.Name -eq 'Test-FullyQualifiedWindowsPath'
        }, $true)
    if ($launcherLocalPathValidator.Count -ne 0) {
        throw 'launcher must use the manifest-verified module path validator'
    }

    $node = Get-NativeExecutable -Name 'node.exe'
    $git = Get-NativeExecutable -Name 'git.exe'
    $rustc = Get-NativeExecutable -Name 'rustc.exe'
    $npm = Get-NativeExecutable -Name 'npm.cmd'
    $tauri = Join-Path $repoRoot 'node_modules/.bin/tauri.cmd'
    if (-not [System.IO.File]::Exists($tauri)) {
        throw 'required project node_modules/.bin/tauri.cmd is unavailable'
    }

    $argvValues = @(
        '',
        'contains whitespace',
        'embedded"quote',
        'trailing\\',
        'Unicode-✓',
        '^{commit}',
        '^{tree}',
        '^',
        '&',
        '|',
        '<',
        '>',
        '$literal',
        '(parentheses)'
    )
    $nodeArgvScript = 'process.stdout.write(JSON.stringify(process.argv.slice(1)))'
    $argvProbe = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $node `
        -WorkingDirectory $repoRoot `
        -Arguments (@('-e', $nodeArgvScript, '--') + $argvValues) `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment
    if ($argvProbe.ExitCode -ne 0) {
        throw 'native node.exe argv probe failed'
    }
    $receivedArgv = @(ConvertFrom-NativeJsonArray -Json $argvProbe.StandardOutput)
    if ($receivedArgv.Count -ne $argvValues.Count) {
        throw 'native node.exe argv probe returned an unexpected argument count'
    }
    for ($index = 0; $index -lt $argvValues.Count; $index++) {
        if ($receivedArgv[$index] -cne $argvValues[$index]) {
            throw "native node.exe argv probe changed argument index $index"
        }
    }

    $nulArgument = "before$([char]0)after"
    Assert-NativeProcessFailure -CaseName 'NUL native argument' -Action {
        Start-IsolatedValidationNativeProcess `
            -Mode CaptureAndWait `
            -FilePath $node `
            -WorkingDirectory $repoRoot `
            -Arguments @('-e', $nodeArgvScript, $nulArgument) `
            -StandardOutputLimitBytes 1MB `
            -StandardErrorLimitBytes 1MB `
            -RemoveAgentsCommanderEnvironment
    }

    $zeroArgv = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $node `
        -WorkingDirectory $repoRoot `
        -Arguments @('-e', $nodeArgvScript, '--') `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment
    $oneEmptyArgv = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $node `
        -WorkingDirectory $repoRoot `
        -Arguments @('-e', $nodeArgvScript, '--', '') `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment
    if ($zeroArgv.StandardOutput -cne '[]' -or $oneEmptyArgv.StandardOutput -cne '[""]') {
        throw 'native node.exe argv probe did not distinguish zero values from one empty value'
    }

    $dualStreamScript = "process.stdout.write('o'.repeat(131072)); process.stderr.write('e'.repeat(131072))"
    $dualStream = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $node `
        -WorkingDirectory $repoRoot `
        -Arguments @('-e', $dualStreamScript) `
        -StandardOutputLimitBytes 256KB `
        -StandardErrorLimitBytes 256KB `
        -RemoveAgentsCommanderEnvironment
    if ($dualStream.StandardOutput.Length -ne 131072 -or $dualStream.StandardError.Length -ne 131072) {
        throw 'native module did not concurrently drain both node.exe streams'
    }

    $capPidPath = Join-Path $testRoot 'cap-overflow-child.pid'
    $capScript = "require('fs').writeFileSync(process.argv[1], String(process.pid)); process.stdout.write('o'.repeat(131072)); setInterval(() => {}, 1000)"
    Assert-NativeProcessFailure -CaseName 'bounded capture cap breach' -Action {
        Start-IsolatedValidationNativeProcess `
            -Mode CaptureAndWait `
            -FilePath $node `
            -WorkingDirectory $repoRoot `
            -Arguments @('-e', $capScript, '--', $capPidPath) `
            -StandardOutputLimitBytes 64KB `
            -StandardErrorLimitBytes 64KB `
            -RemoveAgentsCommanderEnvironment | Out-Null
    }
    for ($attempt = 1; $attempt -le 40 -and -not (Test-Path -LiteralPath $capPidPath -PathType Leaf); $attempt++) {
        Start-Sleep -Milliseconds 25
    }
    if (-not (Test-Path -LiteralPath $capPidPath -PathType Leaf)) {
        throw 'cap-overflow child did not publish its PID before termination'
    }
    Wait-ForProcessExit -ProcessId ([int](Get-Content -LiteralPath $capPidPath -Raw)) -CaseName 'cap-overflow original lease'

    $missingFile = Join-Path $testRoot 'missing.exe'
    $missingDirectory = Join-Path $testRoot 'missing-directory'
    foreach ($invalid in @(
            [pscustomobject]@{ FilePath = 'C:relative.exe'; WorkingDirectory = $repoRoot; Name = 'drive-relative executable' },
            [pscustomobject]@{ FilePath = '\\foo'; WorkingDirectory = $repoRoot; Name = 'UNC-root-relative executable' },
            [pscustomobject]@{ FilePath = '/foo'; WorkingDirectory = $repoRoot; Name = 'slash-root-relative executable' },
            [pscustomobject]@{ FilePath = $node; WorkingDirectory = 'C:relative'; Name = 'drive-relative working directory' },
            [pscustomobject]@{ FilePath = $node; WorkingDirectory = '\\foo'; Name = 'UNC-root-relative working directory' },
            [pscustomobject]@{ FilePath = $node; WorkingDirectory = '/foo'; Name = 'slash-root-relative working directory' },
            [pscustomobject]@{ FilePath = $missingFile; WorkingDirectory = $repoRoot; Name = 'missing executable' },
            [pscustomobject]@{ FilePath = $node; WorkingDirectory = $missingDirectory; Name = 'missing working directory' }
        )) {
        Assert-NativeProcessFailure -CaseName $invalid.Name -Action {
            Start-IsolatedValidationNativeProcess `
                -Mode Wait `
                -FilePath $invalid.FilePath `
                -WorkingDirectory $invalid.WorkingDirectory `
                -Arguments @() `
                -RemoveAgentsCommanderEnvironment | Out-Null
        }
    }

    [Environment]::SetEnvironmentVariable('AGENTSCOMMANDER_TEST_NATIVE_PARENT', 'must-remain-parent-only')
    $environmentScript = "process.stdout.write(JSON.stringify(Object.keys(process.env).filter((key) => key.toUpperCase().startsWith('AGENTSCOMMANDER_'))))"
    $environmentProbe = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $node `
        -WorkingDirectory $repoRoot `
        -Arguments @('-e', $environmentScript) `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment
    if ($environmentProbe.StandardOutput -cne '[]' -or
        [Environment]::GetEnvironmentVariable('AGENTSCOMMANDER_TEST_NATIVE_PARENT') -cne 'must-remain-parent-only') {
        throw 'native child environment cleanup leaked or changed AGENTSCOMMANDER_* state'
    }

    foreach ($suffix in @('HEAD^{commit}', 'HEAD^{tree}')) {
        $gitProbe = Start-IsolatedValidationNativeProcess `
            -Mode CaptureAndWait `
            -FilePath $git `
            -WorkingDirectory $repoRoot `
            -Arguments @('rev-parse', $suffix) `
            -StandardOutputLimitBytes 1MB `
            -StandardErrorLimitBytes 1MB `
            -RemoveAgentsCommanderEnvironment
        $gitOutput = ([string]$gitProbe.StandardOutput).Trim()
        if ($gitProbe.ExitCode -ne 0 -or $gitOutput -notmatch '^[0-9a-f]{40}$') {
            throw "real Git suffix probe failed for ${suffix}: exit=$($gitProbe.ExitCode), stdout=[$gitOutput], stderr=[$($gitProbe.StandardError)]"
        }
    }

    $tauriProbe = Start-IsolatedValidationNativeProcess `
        -Mode Wait `
        -FilePath $tauri `
        -WorkingDirectory $repoRoot `
        -Arguments @('--version') `
        -RemoveAgentsCommanderEnvironment
    if ($tauriProbe.ExitCode -ne 0) {
        throw 'real project tauri.cmd invocation failed'
    }

    $currentCommit = (Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $git `
        -WorkingDirectory $repoRoot `
        -Arguments @('rev-parse', 'HEAD^{commit}') `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment).StandardOutput.Trim()
    $stage = 'real detached package build handoff'
    $actualHandoff = Invoke-DetachedPackageBuildHandoff `
        -RepositoryRoot $repoRoot `
        -TestRoot $testRoot `
        -GitExecutable $git `
        -NpmExecutable $npm `
        -Frozen1271Commit $frozen1271Commit `
        -IsolatedStateRootCommit $currentCommit

    $actualArtifact = [string]$actualHandoff.artifactDirectory
    $actualLauncher = Join-Path $actualArtifact 'launch-isolated.ps1'
    $actualModule = Join-Path $actualArtifact 'native-process.psm1'
    $actualProfile = Join-Path $actualArtifact 'resources/package-profile.toml'
    $actualExecutable = Join-Path $actualArtifact 'Agents Commander Isolated Gates.exe'
    $actualManifest = Join-Path $actualArtifact 'isolated-validation-manifest.json'
    foreach ($actualPayload in @($actualLauncher, $actualModule, $actualProfile, $actualExecutable, $actualManifest)) {
        if (-not (Test-Path -LiteralPath $actualPayload -PathType Leaf)) {
            throw "real detached package handoff is missing $([System.IO.Path]::GetFileName($actualPayload))"
        }
    }
    if ((Get-Sha256 -LiteralPath $actualManifest) -cne $actualHandoff.manifestSha256) {
        throw 'real detached package handoff manifest hash changed after detached source cleanup'
    }
    if (Test-Path -LiteralPath (Join-Path $testRoot 'detached-package-build-checkout')) {
        throw 'detached source and Cargo-output checkout remained available to the real package handoff test'
    }

    $actualInvalidReceiptFixture = Join-Path $testRoot 'actual-package-invalid-receipt'
    New-Item -ItemType Directory -Path $actualInvalidReceiptFixture -Force | Out-Null
    [System.IO.File]::WriteAllText(
        (Join-Path $actualInvalidReceiptFixture 'launch-receipt.json'),
        "{}$([Environment]::NewLine)",
        [System.Text.UTF8Encoding]::new($false)
    )
    $actualInvalidReceipt = Invoke-Launcher `
        -Launcher $actualLauncher `
        -FixtureRoot $actualInvalidReceiptFixture `
        -ExpectedManifestSha256 $actualHandoff.manifestSha256
    if ($actualInvalidReceipt.Succeeded -or
        (Test-Path -LiteralPath (Join-Path $actualInvalidReceiptFixture 'app-state'))) {
        throw 'real detached package handoff accepted an invalid receipt or launched a child'
    }

    foreach ($actualPayload in @($actualLauncher, $actualModule, $actualProfile, $actualExecutable)) {
        $stage = "real detached package payload $([System.IO.Path]::GetFileName($actualPayload))"
        $originalPayloadBytes = [System.IO.File]::ReadAllBytes($actualPayload)
        $tamperFixture = Join-Path $testRoot ('actual-package-tamper-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $tamperFixture | Out-Null
        try {
            [System.IO.File]::AppendAllText($actualPayload, 'tampered', [System.Text.UTF8Encoding]::new($false))
            $tamperResult = Invoke-Launcher `
                -Launcher $actualLauncher `
                -FixtureRoot $tamperFixture `
                -ExpectedManifestSha256 $actualHandoff.manifestSha256
            if ($tamperResult.Succeeded -or (Test-Path -LiteralPath (Join-Path $tamperFixture 'app-state'))) {
                throw "real detached package payload tamper was not rejected before child launch: $([System.IO.Path]::GetFileName($actualPayload))"
            }
        }
        finally {
            [System.IO.File]::WriteAllBytes($actualPayload, $originalPayloadBytes)
        }
    }

    $actualValidFixture = Join-Path $testRoot 'actual-copied-handoff-valid-fixture'
    New-Item -ItemType Directory -Path $actualValidFixture | Out-Null
    $actualValidResult = Start-LauncherUnderTestJob `
        -Launcher $actualLauncher `
        -FixtureRoot $actualValidFixture `
        -ExpectedManifestSha256 $actualHandoff.manifestSha256 `
        -TestRoot $testRoot

    try {
        $actualValidLaunch = $actualValidResult.Launch
        if ($null -eq $actualValidLaunch -or [int]$actualValidLaunch.processId -le 0) {
            throw 'real copied handoff launcher did not return its JSON launch result'
        }

        $actualValidReceiptPath = Join-Path $actualValidFixture 'launch-receipt.json'
        if (-not (Test-Path -LiteralPath $actualValidReceiptPath -PathType Leaf)) {
            throw 'real copied handoff launcher did not materialize its status receipt'
        }
        $actualValidReceipt = [System.IO.File]::ReadAllText($actualValidReceiptPath, [System.Text.UTF8Encoding]::new($false)) |
            ConvertFrom-Json -ErrorAction Stop
        if ([string]::IsNullOrWhiteSpace([string]$actualValidReceipt.effectiveRoot) -or
            [string]::IsNullOrWhiteSpace([string]$actualValidReceipt.mutexHash)) {
            throw 'real copied handoff status receipt omitted dynamic root identity'
        }
    }
    finally {
        if ($null -ne $actualValidResult.WrapperProcess) {
            $actualValidResult.WrapperProcess.Dispose()
        }
        if ($null -ne $actualValidResult.Job) {
            $actualValidResult.Job.Dispose()
        }
    }

    $artifact = Join-Path $testRoot 'staged-artifact'
    $resources = Join-Path $artifact 'resources'
    $fixture = Join-Path $testRoot 'fixture root; & metacharacters'
    New-Item -ItemType Directory -Path $resources -Force | Out-Null
    New-Item -ItemType Directory -Path $fixture -Force | Out-Null
    $launcher = Join-Path $artifact 'launch-isolated.ps1'
    $module = Join-Path $artifact 'native-process.psm1'
    $profile = Join-Path $resources 'package-profile.toml'
    $executable = Join-Path $artifact 'Agents Commander Isolated Gates.exe'
    Copy-Item -LiteralPath $launcherSource -Destination $launcher
    Copy-Item -LiteralPath $moduleSource -Destination $module
    Copy-Item -LiteralPath $profileSource -Destination $profile
    $fixtureBuild = Start-IsolatedValidationNativeProcess `
        -Mode Wait `
        -FilePath $rustc `
        -WorkingDirectory $repoRoot `
        -Arguments @('--edition', '2021', $fixtureSource, '-o', $executable) `
        -RemoveAgentsCommanderEnvironment
    if ($fixtureBuild.ExitCode -ne 0) {
        throw 'could not build the native staged-artifact fixture executable'
    }

    $pipeLeakReadyPath = Join-Path $artifact 'pipe-leak-ready.txt'
    $pipeLeakExitPath = Join-Path $artifact 'pipe-leak-exit.txt'
    [Environment]::SetEnvironmentVariable('ISOLATED_VALIDATION_TEST_PIPE_LEAK_READY_PATH', $pipeLeakReadyPath)
    [Environment]::SetEnvironmentVariable('ISOLATED_VALIDATION_TEST_PIPE_LEAK_EXIT_PATH', $pipeLeakExitPath)
    try {
        $pipeLeakStopwatch = [System.Diagnostics.Stopwatch]::StartNew()
        Assert-NativeProcessFailure -CaseName 'descendant-held redirected pipe capture' -Action {
            Start-IsolatedValidationNativeProcess `
                -Mode CaptureAndWait `
                -FilePath $executable `
                -WorkingDirectory $artifact `
                -Arguments @('--pipe-leak') `
                -StandardOutputLimitBytes 32KB `
                -StandardErrorLimitBytes 32KB `
                -RemoveAgentsCommanderEnvironment
        }
        $pipeLeakStopwatch.Stop()
        if ($pipeLeakStopwatch.ElapsedMilliseconds -gt 5000) {
            throw 'descendant-held redirected pipe capture cleanup exceeded its bounded deadline'
        }
        for ($attempt = 1; $attempt -le 80 -and -not (Test-Path -LiteralPath $pipeLeakReadyPath -PathType Leaf); $attempt++) {
            Start-Sleep -Milliseconds 25
        }
        if (-not (Test-Path -LiteralPath $pipeLeakReadyPath -PathType Leaf)) {
            throw 'descendant-held redirected pipe fixture did not inherit the redirected handles'
        }
        for ($attempt = 1; $attempt -le 240 -and -not (Test-Path -LiteralPath $pipeLeakExitPath -PathType Leaf); $attempt++) {
            Start-Sleep -Milliseconds 25
        }
        if (-not (Test-Path -LiteralPath $pipeLeakExitPath -PathType Leaf)) {
            throw 'descendant-held redirected pipe fixture did not release its inherited handles'
        }
    }
    finally {
        Remove-Item Env:ISOLATED_VALIDATION_TEST_PIPE_LEAK_READY_PATH -ErrorAction SilentlyContinue
        Remove-Item Env:ISOLATED_VALIDATION_TEST_PIPE_LEAK_EXIT_PATH -ErrorAction SilentlyContinue
    }

    $profileHash = Get-Sha256 -LiteralPath $profile
    $manifestPath = Join-Path $artifact 'isolated-validation-manifest.json'
    $manifest = [ordered]@{
        schema = 'isolated-validation-handoff-v1'
        baseSha = ('0' * 40)
        frozen1271Commit = $frozen1271Commit
        isolatedStateRootCommit = $currentCommit
        combinedSourceSha = $currentCommit
        combinedTreeSha = $currentCommit
        cleanWorktree = $true
        artifactKind = 'portable-layout'
        compiledProfileSha256 = $profileHash
        utcTimestamp = [DateTime]::UtcNow.ToString('o')
        mode = 'isolated-validation-package'
        target = 'native-test-fixture'
        productLabel = 'Agents Commander Isolated Gates'
        bundleIdentifier = 'dev.agentscommander.isolatedgates'
        headerIdentity = 'WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated'
        launcherCommand = '.\launch-isolated.ps1 -FixtureRoot <absolute-fixture-root> -ExpectedManifestSha256 <trusted-hash>'
        payloads = [ordered]@{
            executable = [ordered]@{ relativePath = 'Agents Commander Isolated Gates.exe'; sha256 = Get-Sha256 -LiteralPath $executable }
            profile = [ordered]@{ relativePath = 'resources/package-profile.toml'; sha256 = $profileHash }
            launcher = [ordered]@{ relativePath = 'launch-isolated.ps1'; sha256 = Get-Sha256 -LiteralPath $launcher }
            nativeProcessModule = [ordered]@{ relativePath = 'native-process.psm1'; sha256 = Get-Sha256 -LiteralPath $module }
        }
    }
    Write-JsonFile -LiteralPath $manifestPath -Value $manifest
    $expectedManifestHash = Get-Sha256 -LiteralPath $manifestPath
    $payloadBytes = @{}
    foreach ($path in @($manifestPath, $executable, $profile, $launcher, $module)) {
        $payloadBytes[$path] = [System.IO.File]::ReadAllBytes($path)
    }

    [Environment]::SetEnvironmentVariable('AGENTSCOMMANDER_TEST_LAUNCHER_INHERITED', 'must-not-reach-child')
    [Environment]::SetEnvironmentVariable('ISOLATED_VALIDATION_TEST_PROFILE_HASH', $profileHash)
    $childSentinel = Join-Path $artifact 'child-execution-sentinel.txt'
    if (Test-Path -LiteralPath (Join-Path $fixture 'app-state')) {
        throw 'staged-artifact fixture pre-created the isolated app-state root'
    }
    $stage = 'initial staged-artifact launch'
    $first = Invoke-Launcher -Launcher $launcher -FixtureRoot $fixture -ExpectedManifestSha256 $expectedManifestHash
    if (-not $first.Succeeded) {
        throw "valid staged artifact did not launch: $($first.Output)"
    }
    $firstResult = $first.Output | ConvertFrom-Json
    Wait-ForProcessExit -ProcessId $firstResult.processId -CaseName 'initial staged artifact GUI'
    $receipt = Join-Path $fixture 'launch-receipt.json'
    if (-not (Test-Path -LiteralPath $receipt -PathType Leaf)) {
        throw 'valid staged artifact did not publish an initial receipt'
    }
    $firstReceiptBytes = [System.IO.File]::ReadAllBytes($receipt)
    $stage = 'immutable staged-artifact relaunch'
    $second = Invoke-Launcher -Launcher $launcher -FixtureRoot $fixture -ExpectedManifestSha256 $expectedManifestHash
    if (-not $second.Succeeded) {
        $diagnostic = & $launcher -Verbose -FixtureRoot $fixture -ExpectedManifestSha256 $expectedManifestHash 2>&1
        throw "valid staged artifact relaunch failed: $($second.Output); diagnostic=$($diagnostic -join [Environment]::NewLine); receipt=$(Get-Content -LiteralPath $receipt -Raw)"
    }
    Wait-ForProcessExit -ProcessId (($second.Output | ConvertFrom-Json).processId) -CaseName 'immutable receipt relaunch GUI'
    $secondReceiptBytes = [System.IO.File]::ReadAllBytes($receipt)
    if ([System.BitConverter]::ToString($firstReceiptBytes) -cne [System.BitConverter]::ToString($secondReceiptBytes)) {
        throw 'valid re-launch changed immutable receipt bytes'
    }
    $launcherChildEnvironment = [System.IO.File]::ReadAllText((Join-Path $artifact 'fixture-child-env.txt'))
    if ($launcherChildEnvironment.Contains('AGENTSCOMMANDER_TEST_LAUNCHER_INHERITED')) {
        throw 'launcher leaked parent AGENTSCOMMANDER_* state to staged child'
    }

    foreach ($tamper in @($manifestPath, $executable, $profile, $launcher, $module)) {
        $stage = "payload tamper $([System.IO.Path]::GetFileName($tamper))"
        $tamperFixture = Join-Path $testRoot ('tamper-' + [Guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Path $tamperFixture | Out-Null
        Add-Content -LiteralPath $tamper -Value 'tampered'
        $tamperExpectedHash = if ($tamper -ceq $manifestPath) { Get-Sha256 -LiteralPath $manifestPath } else { $expectedManifestHash }
        Assert-LauncherFailsBeforeChild `
            -Launcher $launcher `
            -FixtureRoot $tamperFixture `
            -ExpectedManifestSha256 $tamperExpectedHash `
            -ChildSentinel $childSentinel `
            -CaseName "payload tamper $([System.IO.Path]::GetFileName($tamper))"
        [System.IO.File]::WriteAllBytes($tamper, $payloadBytes[$tamper])
    }

    $statusChildSentinel = Join-Path $artifact 'status-child-sentinel.txt'
    [Environment]::SetEnvironmentVariable('ISOLATED_VALIDATION_TEST_STATUS_CHILD_SENTINEL_PATH', $statusChildSentinel)
    try {
        foreach ($dynamicCase in @(
                'malformed',
                'foreign',
                'mismatching',
                'missing-timestamp',
                'non-string-timestamp',
                'invalid-timestamp'
            )) {
            $stage = "near-valid receipt $dynamicCase"
            $nearFixture = Join-Path $testRoot ("near-valid-receipt-$dynamicCase")
            $nearStateRoot = Join-Path $nearFixture 'app-state'
            New-Item -ItemType Directory -Path $nearStateRoot -Force | Out-Null
            $nearReceipt = Get-Content -LiteralPath $receipt -Raw | ConvertFrom-Json
            $nearReceipt.fixtureRoot = $nearFixture
            $nearReceipt.isolatedStateRoot = $nearStateRoot
            switch ($dynamicCase) {
                'malformed' {
                    $nearReceipt.PSObject.Properties.Remove('mutexHash')
                }
                'foreign' {
                    $nearReceipt.effectiveRoot = Join-Path $testRoot 'foreign-isolated-root'
                    $nearReceipt.mutexHash = ('f' * 64) -join ''
                }
                'mismatching' {
                    $nearReceipt.effectiveRoot = $nearStateRoot
                    $nearReceipt.mutexHash = ('0' * 64) -join ''
                }
                'missing-timestamp' {
                    $nearReceipt.PSObject.Properties.Remove('utcTimestamp')
                }
                'non-string-timestamp' {
                    $nearReceipt.utcTimestamp = 1
                }
                'invalid-timestamp' {
                    $nearReceipt.utcTimestamp = 'not-a-roundtrip-utc-timestamp'
                }
            }
            $nearReceiptPath = Join-Path $nearFixture 'launch-receipt.json'
            Write-JsonFile -LiteralPath $nearReceiptPath -Value $nearReceipt
            $beforeReceiptBytes = [System.IO.File]::ReadAllBytes($nearReceiptPath)
            $nearReceiptFailureArguments = @{
                Launcher = $launcher
                FixtureRoot = $nearFixture
                ExpectedManifestSha256 = $expectedManifestHash
                ChildSentinel = $childSentinel
                StatusChildSentinel = $statusChildSentinel
                CaseName = "near-valid $dynamicCase dynamic receipt"
            }
            Assert-LauncherFailsBeforeChild @nearReceiptFailureArguments
            $afterReceiptBytes = [System.IO.File]::ReadAllBytes($nearReceiptPath)
            if ([System.BitConverter]::ToString($beforeReceiptBytes) -cne [System.BitConverter]::ToString($afterReceiptBytes)) {
                throw "near-valid $dynamicCase receipt was changed after rejection"
            }
        }
    }
    finally {
        Remove-Item Env:ISOLATED_VALIDATION_TEST_STATUS_CHILD_SENTINEL_PATH -ErrorAction SilentlyContinue
    }

    $collisionFixture = Join-Path $testRoot 'receipt-publication-collision'
    $stage = 'receipt-publication collision cleanup'
    New-Item -ItemType Directory -Path $collisionFixture | Out-Null
    [Environment]::SetEnvironmentVariable('ISOLATED_VALIDATION_TEST_RECEIPT_COLLISION', '1')
    if (Test-Path -LiteralPath $childSentinel) { Remove-Item -LiteralPath $childSentinel -Force }
    $collision = Invoke-Launcher -Launcher $launcher -FixtureRoot $collisionFixture -ExpectedManifestSha256 $expectedManifestHash -IncludeVerboseOutput
    if ($collision.Succeeded) {
        throw 'receipt-publication collision unexpectedly succeeded'
    }
    if ($collision.Output -notmatch 'owned GUI cleanup completed') {
        throw "receipt-publication collision did not complete cleanup through its original lease: $($collision.Output)"
    }
    $collisionReceipt = Join-Path $collisionFixture 'launch-receipt.json'
    if ((Get-Content -LiteralPath $collisionReceipt -Raw) -cne "concurrent winner`n") {
        throw 'receipt-publication failure changed a concurrent winner receipt'
    }
    if (@(Get-ChildItem -LiteralPath $collisionFixture -Filter '.launch-receipt-*.tmp' -Force).Count -ne 0) {
        throw 'receipt-publication failure left a temporary receipt behind'
    }

    $cleanupProbeRoot = Join-Path $testRoot 'forced-cleanup-exhaustion'
    New-Item -ItemType Directory -Path $cleanupProbeRoot -ErrorAction Stop | Out-Null
    $probePrimaryFailure = $null
    $probeObservedFailure = $null
    $probeCleanupRecords = @()
    try {
        try {
            throw 'test-local forced primary failure'
        }
        catch {
            $probePrimaryFailure = $_
            throw
        }
        finally {
            $probeCleanupRecords = @(
                Invoke-IsolatedValidationTestRootCleanup `
                    -Path $cleanupProbeRoot `
                    -RetryLimit 3 `
                    -PrimaryFailure $probePrimaryFailure `
                    -RemoveAction {
                        param([string]$Path)
                        throw 'test-local forced cleanup failure'
                    } 3>&1
            )
        }
    }
    catch {
        $probeObservedFailure = $_
    }

    $probeCleanupOutcome = @($probeCleanupRecords | Where-Object {
            $null -ne $_.PSObject.Properties['Succeeded']
        })
    $probeWarnings = @($probeCleanupRecords | Where-Object {
            $_ -is [System.Management.Automation.WarningRecord]
        })
    if ($null -eq $probeObservedFailure -or
        $probeObservedFailure.Exception.Message -cne 'test-local forced primary failure' -or
        $probeCleanupOutcome.Count -ne 1 -or
        $probeCleanupOutcome[0].Succeeded -or
        $probeCleanupOutcome[0].Attempts -ne 3 -or
        $probeCleanupOutcome[0].CleanupMessage -notlike '*after 3 attempts*' -or
        $probeWarnings.Count -ne 1 -or
        $probeWarnings[0].Message -notlike '*preserving the primary test failure*') {
        throw 'test-local cleanup exhaustion did not preserve the original primary failure'
    }
    Remove-Item -LiteralPath $cleanupProbeRoot -Recurse -Force -ErrorAction Stop
    if (Test-Path -LiteralPath $cleanupProbeRoot) {
        throw 'test-local cleanup exhaustion probe root remained after explicit cleanup'
    }

    Invoke-IsolatedValidationExtractionCleanupRegression `
        -Builder (Join-Path $PSScriptRoot 'build-isolated-validation-package.ps1') `
        -TestRoot $testRoot

    [pscustomobject]@{
        result = 'passed'
        host = "$($PSVersionTable.PSEdition) $($PSVersionTable.PSVersion)"
        cases = @(
            'AST production contract',
            'real node.exe argv oracle and bounded capture',
            'NUL, nonabsolute, and missing native inputs',
            'real Git suffixes and tauri.cmd invocation',
            'detached build revision preflight',
            'staged artifact receipt immutability and payload integrity',
            'near-valid receipt rejection before child launch',
            'receipt-publication original-handle cleanup',
            'test-local cleanup exhaustion preserves primary failure',
            'handle-bound MSI cleanup leaves replacement untouched'
        )
    } | ConvertTo-Json -Depth 4
}
catch {
    $primaryFailure = $_
    Write-Error "isolated launcher regression failed during ${stage}: $($_.Exception.Message)"
    throw
}
finally {
    foreach ($name in $savedParentEnvironment.Keys) {
        if ($null -eq $savedParentEnvironment[$name]) {
            Remove-Item -LiteralPath ("Env:$name") -ErrorAction SilentlyContinue
        }
        else {
            Set-Item -LiteralPath ("Env:$name") -Value $savedParentEnvironment[$name]
        }
    }
    if (Test-Path -LiteralPath $testRoot) {
        Invoke-IsolatedValidationTestRootCleanup `
            -Path $testRoot `
            -RetryLimit $testRootCleanupRetryLimit `
            -PrimaryFailure $primaryFailure `
            -RemoveAction {
                param([string]$Path)
                Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
            } | Out-Null
    }
}
