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

$releaseDirectory = Join-Path $RepoRoot 'target/release'
$executable = Get-ChildItem -LiteralPath $releaseDirectory -Filter '*.exe' -File |
    Where-Object { $_.BaseName -eq 'agentscommander' } |
    Select-Object -First 1
if ($null -eq $executable) {
    throw "could not locate the packaged executable below $releaseDirectory"
}

$resourceRelativePath = 'resources/package-profile.toml'
$bundledProfilePath = Join-Path $releaseDirectory $resourceRelativePath
if (-not (Test-Path -LiteralPath $bundledProfilePath -PathType Leaf)) {
    throw "expected packaged resource at $bundledProfilePath"
}
$bundledProfile = Get-Item -LiteralPath $bundledProfilePath -ErrorAction Stop

# Tauri's final installer is not itself runnable as the handoff executable. Build
# a fresh, verified portable layout from the exact resource materialized by the
# bundle, not from an arbitrary first profile match or the checked-in source.
$artifactDirectory = Join-Path $releaseDirectory ('isolated-validation-portable-' + [Guid]::NewGuid().ToString('N'))
$artifactResources = Join-Path $artifactDirectory 'resources'
New-Item -ItemType Directory -Path $artifactResources -ErrorAction Stop | Out-Null
$artifactExecutable = Join-Path $artifactDirectory 'Agents Commander Isolated Gates.exe'
$artifactProfile = Join-Path $artifactResources 'package-profile.toml'
Copy-Item -LiteralPath $executable.FullName -Destination $artifactExecutable -ErrorAction Stop
Copy-Item -LiteralPath $bundledProfile.FullName -Destination $artifactProfile -ErrorAction Stop

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
$bundledProfileHash = Get-Sha256 -LiteralPath $bundledProfile.FullName
$installedProfileHash = Get-Sha256 -LiteralPath $artifactProfile
if ($profileHash -cne $bundledProfileHash -or $bundledProfileHash -cne $installedProfileHash) {
    throw 'compiled, bundled, and portable artifact profile bytes must be identical'
}
if ((Get-Sha256 -LiteralPath $launcherSource) -cne (Get-Sha256 -LiteralPath $launcherDestination) -or
    (Get-Sha256 -LiteralPath $nativeProcessModuleSource) -cne (Get-Sha256 -LiteralPath $nativeProcessModuleDestination)) {
    throw 'the staged launcher and native process module must be byte-identical copies'
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
