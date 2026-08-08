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
    param([Parameter(Mandatory)][string[]]$Arguments)

    # PowerShell's legacy native-argument marshalling strips `^` from Git
    # revision suffixes such as `^{commit}`. ArgumentList preserves the exact
    # revision bytes while also avoiding a string-built command line.
    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = 'git'
    $startInfo.WorkingDirectory = $RepoRoot
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    foreach ($argument in $Arguments) {
        [void]$startInfo.ArgumentList.Add($argument)
    }

    $process = [System.Diagnostics.Process]::new()
    $process.StartInfo = $startInfo
    if (-not $process.Start()) {
        throw 'failed to start git for isolated validation package preflight'
    }
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $process.WaitForExit()
    if ($process.ExitCode -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $($process.ExitCode): $($stderr.Trim())"
    }
    return $stdout
}

function Get-Sha256 {
    param([Parameter(Mandatory)][string]$LiteralPath)

    return (Get-FileHash -LiteralPath $LiteralPath -Algorithm SHA256).Hash.ToLowerInvariant()
}

$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path

if ($Frozen1271Commit.ToLowerInvariant() -cne $ExpectedFrozen1271Commit) {
    throw "-Frozen1271Commit must be $ExpectedFrozen1271Commit"
}

$status = Invoke-Git -Arguments @('status', '--porcelain')
if ($status) {
    throw 'a clean worktree is required before building the isolated validation package'
}

& git -C $RepoRoot symbolic-ref -q HEAD *> $null
if ($LASTEXITCODE -eq 0) {
    throw 'a detached worktree is required before building the isolated validation package'
}

$targetCommit = (Invoke-Git -Arguments @('rev-parse', '--verify', "$IsolatedStateRootCommit^{commit}")).Trim()
if ($targetCommit.ToLowerInvariant() -cne $IsolatedStateRootCommit.ToLowerInvariant()) {
    throw '-IsolatedStateRootCommit must resolve to its supplied full 40-hex SHA'
}

& git -C $RepoRoot merge-base --is-ancestor $ExpectedFrozen1271Commit $targetCommit
if ($LASTEXITCODE -ne 0) {
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

& $tauriCli build --features $PackageFeature --config $overlayPath
if ($LASTEXITCODE -ne 0) {
    throw "isolated validation package build failed with exit code $LASTEXITCODE"
}

$releaseDirectory = Join-Path $RepoRoot 'src-tauri/target/release'
$executable = Get-ChildItem -LiteralPath $releaseDirectory -Filter '*.exe' -File |
    Where-Object { $_.BaseName -eq 'agentscommander' } |
    Select-Object -First 1
if ($null -eq $executable) {
    throw "could not locate the packaged executable below $releaseDirectory"
}

$bundleDirectory = Join-Path $releaseDirectory 'bundle'
$resourceRelativePath = 'resources/isolated-validation/package-profile.toml'
$resourceTail = [System.IO.Path]::Combine('resources', 'isolated-validation', 'package-profile.toml')
$bundledProfiles = @(
    Get-ChildItem -LiteralPath $bundleDirectory -Recurse -Filter 'package-profile.toml' -File |
        Where-Object {
            $_.FullName.EndsWith(
                $resourceTail,
                [System.StringComparison]::OrdinalIgnoreCase
            )
        }
)
if ($bundledProfiles.Count -ne 1) {
    throw "expected exactly one bundle resource at $resourceRelativePath below $bundleDirectory; found $($bundledProfiles.Count)"
}
$bundledProfile = $bundledProfiles[0]

# Tauri's final installer is not itself runnable as the handoff executable. Build
# a fresh, verified portable layout from the exact resource materialized by the
# bundle, not from an arbitrary first profile match or the checked-in source.
$artifactDirectory = Join-Path $releaseDirectory ('isolated-validation-portable-' + [Guid]::NewGuid().ToString('N'))
$artifactResources = Join-Path $artifactDirectory 'resources/isolated-validation'
New-Item -ItemType Directory -Path $artifactResources -ErrorAction Stop | Out-Null
$artifactExecutable = Join-Path $artifactDirectory $executable.Name
$artifactProfile = Join-Path $artifactResources 'package-profile.toml'
Copy-Item -LiteralPath $executable.FullName -Destination $artifactExecutable -ErrorAction Stop
Copy-Item -LiteralPath $bundledProfile.FullName -Destination $artifactProfile -ErrorAction Stop

$launcherSource = Join-Path $RepoRoot 'packaging/isolated-validation/launch-isolated.ps1'
$launcherDestination = Join-Path $artifactDirectory 'launch-isolated.ps1'
Copy-Item -LiteralPath $launcherSource -Destination $launcherDestination -ErrorAction Stop

$manifestPath = Join-Path $artifactDirectory 'isolated-validation-handoff.json'
$profileHash = Get-Sha256 -LiteralPath $profilePath
$bundledProfileHash = Get-Sha256 -LiteralPath $bundledProfile.FullName
$installedProfileHash = Get-Sha256 -LiteralPath $artifactProfile
if ($profileHash -cne $bundledProfileHash -or $bundledProfileHash -cne $installedProfileHash) {
    throw 'compiled, bundled, and portable artifact profile bytes must be identical'
}

$manifest = [ordered]@{
    schemaVersion = 1
    baseSha = (Invoke-Git -Arguments @('merge-base', $ExpectedFrozen1271Commit, $targetCommit)).Trim()
    frozen1271Commit = $ExpectedFrozen1271Commit
    isolatedStateRootCommit = $targetCommit
    combinedSourceSha = (Invoke-Git -Arguments @('rev-parse', 'HEAD')).Trim()
    combinedTreeSha = (Invoke-Git -Arguments @('rev-parse', 'HEAD^{tree}')).Trim()
    cleanWorktree = $true
    artifactKind = 'portable-layout'
    executableFileName = (Split-Path -Path $artifactExecutable -Leaf)
    executableSha256 = Get-Sha256 -LiteralPath $artifactExecutable
    profileResourceRelativePath = $resourceRelativePath
    compiledProfileSha256 = $profileHash
    bundledProfileSha256 = $bundledProfileHash
    installedProfileSha256 = $installedProfileHash
    utcTimestamp = [DateTime]::UtcNow.ToString('o')
    mode = 'isolated-validation-package'
    target = $env:TARGET
    productLabel = 'Agents Commander Isolated Gates'
    bundleIdentifier = 'dev.agentscommander.isolatedgates'
    headerIdentity = 'WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated'
    launcherCommand = '.\launch-isolated.ps1 -FixtureRoot <absolute-fixture-root> -ExpectedManifestSha256 <trusted-hash>'
}

$manifestJson = $manifest | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText($manifestPath, $manifestJson + [Environment]::NewLine, [System.Text.UTF8Encoding]::new($false))
$manifestHash = Get-Sha256 -LiteralPath $manifestPath
[System.IO.File]::WriteAllText(
    "$manifestPath.sha256",
    "$manifestHash  $(Split-Path $manifestPath -Leaf)$" + [Environment]::NewLine,
    [System.Text.UTF8Encoding]::new($false)
)

[pscustomobject]@{
    artifactDirectory = $artifactDirectory
    executable = $artifactExecutable
    manifest = $manifestPath
    manifestSha256 = $manifestHash
    profile = $artifactProfile
} | ConvertTo-Json -Depth 4
