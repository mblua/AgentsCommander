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
        [string]$StatusChildSentinel
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
    if (-not [string]::IsNullOrWhiteSpace($StatusChildSentinel) -and
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
        foreach ($dynamicCase in @('malformed', 'foreign', 'mismatching')) {
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
            }
            $nearReceiptPath = Join-Path $nearFixture 'launch-receipt.json'
            Write-JsonFile -LiteralPath $nearReceiptPath -Value $nearReceipt
            $beforeReceiptBytes = [System.IO.File]::ReadAllBytes($nearReceiptPath)
            Assert-LauncherFailsBeforeChild `
                -Launcher $launcher `
                -FixtureRoot $nearFixture `
                -ExpectedManifestSha256 $expectedManifestHash `
                -ChildSentinel $childSentinel `
                -StatusChildSentinel $statusChildSentinel `
                -CaseName "near-valid $dynamicCase dynamic receipt"
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
            'receipt-publication original-handle cleanup'
        )
    } | ConvertTo-Json -Depth 4
}
catch {
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
        for ($attempt = 1; $attempt -le 60; $attempt++) {
            try {
                Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction Stop
                break
            }
            catch {
                if ($attempt -eq 20) {
                    throw
                }
                Start-Sleep -Milliseconds 100
            }
        }
    }
}
