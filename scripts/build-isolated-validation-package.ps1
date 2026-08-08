[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-fA-F]{40}$')]
    [string]$Frozen1271Commit,

    [Parameter(Mandatory)]
    [ValidatePattern('^[0-9a-fA-F]{40}$')]
    [string]$IsolatedStateRootCommit
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ExpectedFrozen1271Commit = 'd68495086e168e5258500832b2ef45b4337ed21a'
$PackageFeature = 'isolated-validation-package'
$ProfileRelativePath = 'packaging/isolated-validation/package-profile.toml'
$OverlayRelativePath = 'src-tauri/tauri.conf.isolated-validation.json'

function Invoke-Git {
    param([Parameter(Mandatory)][string[]]$Arguments)

    $result = & git -C $RepoRoot @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
    return $result
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
$installedProfile = Get-ChildItem -LiteralPath $bundleDirectory -Recurse -Filter 'package-profile.toml' -File |
    Select-Object -First 1
if ($null -eq $installedProfile) {
    throw "could not locate the bundled package profile below $bundleDirectory"
}

$artifactDirectory = $executable.Directory.FullName
$artifactProfileDirectory = Join-Path $artifactDirectory 'isolated-validation'
New-Item -ItemType Directory -Path $artifactProfileDirectory -Force | Out-Null
$artifactProfile = Join-Path $artifactProfileDirectory 'package-profile.toml'
Copy-Item -LiteralPath $installedProfile.FullName -Destination $artifactProfile -Force

$launcherSource = Join-Path $RepoRoot 'packaging/isolated-validation/launch-isolated.ps1'
$launcherDestination = Join-Path $artifactDirectory 'launch-isolated.ps1'
Copy-Item -LiteralPath $launcherSource -Destination $launcherDestination -Force

$manifestPath = Join-Path $artifactDirectory 'isolated-validation-handoff.json'
$profileHash = Get-Sha256 -LiteralPath $profilePath
$installedProfileHash = Get-Sha256 -LiteralPath $artifactProfile
if ($profileHash -cne $installedProfileHash) {
    throw 'bundled profile bytes differ from the compiled package profile'
}

$manifest = [ordered]@{
    schemaVersion = 1
    baseSha = (Invoke-Git -Arguments @('merge-base', $ExpectedFrozen1271Commit, $targetCommit)).Trim()
    frozen1271Commit = $ExpectedFrozen1271Commit
    isolatedStateRootCommit = $targetCommit
    combinedSourceSha = (Invoke-Git -Arguments @('rev-parse', 'HEAD')).Trim()
    combinedTreeSha = (Invoke-Git -Arguments @('rev-parse', 'HEAD^{tree}')).Trim()
    cleanWorktree = $true
    executableFileName = $executable.Name
    executableSha256 = Get-Sha256 -LiteralPath $executable.FullName
    profileResourceRelativePath = 'isolated-validation/package-profile.toml'
    compiledProfileSha256 = $profileHash
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
    executable = $executable.FullName
    manifest = $manifestPath
    manifestSha256 = $manifestHash
    profile = $artifactProfile
} | ConvertTo-Json -Depth 4
