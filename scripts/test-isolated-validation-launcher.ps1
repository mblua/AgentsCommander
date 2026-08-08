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
        ($Value | ConvertTo-Json -Depth 8) + [Environment]::NewLine,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Invoke-Launcher {
    param(
        [Parameter(Mandatory)][string]$Launcher,
        [Parameter(Mandatory)][string]$FixtureRoot,
        [Parameter(Mandatory)][string]$ExpectedManifestSha256
    )

    try {
        $output = & $Launcher -FixtureRoot $FixtureRoot -ExpectedManifestSha256 $ExpectedManifestSha256 2>&1
        return [pscustomobject]@{
            Succeeded = $LASTEXITCODE -eq 0
            Output = ($output -join [Environment]::NewLine)
        }
    } catch {
        return [pscustomobject]@{ Succeeded = $false; Output = $_.Exception.Message }
    }
}

function Assert-LauncherFailsBeforeReceipt {
    param(
        [Parameter(Mandatory)][string]$Launcher,
        [Parameter(Mandatory)][string]$FixtureRoot,
        [Parameter(Mandatory)][string]$ExpectedManifestSha256,
        [Parameter(Mandatory)][string]$CaseName
    )

    $receipt = Join-Path $FixtureRoot 'launch-receipt.json'
    if (Test-Path -LiteralPath $receipt) {
        throw "$CaseName test fixture unexpectedly already has a receipt"
    }
    $result = Invoke-Launcher -Launcher $Launcher -FixtureRoot $FixtureRoot -ExpectedManifestSha256 $ExpectedManifestSha256
    if ($result.Succeeded) {
        throw "$CaseName tampering unexpectedly launched: $($result.Output)"
    }
    if (Test-Path -LiteralPath $receipt) {
        throw "$CaseName tampering wrote a receipt before verification completed"
    }
}

function Wait-For-LauncherChildExit {
    param([Parameter(Mandatory)][int]$ProcessId)

    for ($attempt = 1; $attempt -le 100; $attempt++) {
        if ($null -eq (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
            return
        }
        Start-Sleep -Milliseconds 25
    }
    throw "mock launcher child process $ProcessId did not exit"
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$launcherSource = Join-Path $repoRoot 'packaging/isolated-validation/launch-isolated.ps1'
$nativeProcessModuleSource = Join-Path $repoRoot 'packaging/isolated-validation/native-process.psm1'
$profileSource = Join-Path $repoRoot 'packaging/isolated-validation/package-profile.toml'
$testRoot = Join-Path ([System.IO.Path]::GetTempPath()) ('agentscommander-isolated-launcher-test-' + [Guid]::NewGuid().ToString('N'))
$originalInherited = [Environment]::GetEnvironmentVariable('AGENTSCOMMANDER_TEST_LAUNCHER_INHERITED')

try {
    $artifact = Join-Path $testRoot 'portable-artifact'
    $resources = Join-Path $artifact 'resources'
    $fixture = Join-Path $testRoot 'fixture root; & metacharacters'
    New-Item -ItemType Directory -Path $resources -Force | Out-Null
    New-Item -ItemType Directory -Path $fixture -Force | Out-Null

    $launcher = Join-Path $artifact 'launch-isolated.ps1'
    $profile = Join-Path $resources 'package-profile.toml'
    $executable = Join-Path $artifact 'Agents Commander Isolated Gates.exe'
    $nativeProcessModule = Join-Path $artifact 'native-process.psm1'
    Copy-Item -LiteralPath $launcherSource -Destination $launcher
    Copy-Item -LiteralPath $nativeProcessModuleSource -Destination $nativeProcessModule
    Copy-Item -LiteralPath $profileSource -Destination $profile

    $mockSource = @'
using System;
using System.Collections;
using System.IO;

public static class Program {
    private static string JsonEscape(string value) {
        return value.Replace("\\", "\\\\").Replace("\"", "\\\"");
    }

    private static void WriteJsonArguments(string[] args, int start) {
        Console.Write("[");
        for (var index = start; index < args.Length; index++) {
            if (index > start) {
                Console.Write(",");
            }
            Console.Write("\"" + JsonEscape(args[index]) + "\"");
        }
        Console.Write("]");
    }

    public static int Main(string[] args) {
        var capturePath = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "mock-child-env.txt");
        using (var writer = new StreamWriter(capturePath, false)) {
            foreach (DictionaryEntry entry in Environment.GetEnvironmentVariables()) {
                var key = entry.Key.ToString();
                if (key.StartsWith("AGENTSCOMMANDER_", StringComparison.OrdinalIgnoreCase)) {
                    writer.WriteLine(key + "=" + entry.Value);
                }
            }
        }

        if (args.Length >= 1 && args[0] == "--argv-json") {
            WriteJsonArguments(args, 1);
            return 0;
        }

        if (args.Length == 1 && args[0] == "--dual-stream") {
            Console.Out.Write(new string('o', 131072));
            Console.Error.Write(new string('e', 131072));
            return 0;
        }

        if (args.Length == 3 && args[0] == "--isolated-state-root" && args[2] == "--isolation-status") {
            Directory.CreateDirectory(args[1]);
            var root = JsonEscape(Path.GetFullPath(args[1]));
            Console.Write("{\"effectiveRoot\":\"" + root + "\",\"packageId\":\"agentscommander-1271-isolated-gates\",\"profileSha256\":\"__PROFILE_HASH__\",\"workspace\":\"AgentsCommander_1271_isolated\",\"matrix\":\"WG-1271-ISOLATED-GATES\",\"replicaAgent\":\"gate-tester\",\"headerIdentity\":\"WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated\",\"bundleIdentifier\":\"dev.agentscommander.isolatedgates\",\"mutexHash\":\"test-mutex-hash\"}");
        }
        return 0;
    }
}
'@
    $profileHash = Get-Sha256 -LiteralPath $profile
    $mockSource = $mockSource.Replace('__PROFILE_HASH__', $profileHash)
    $mockProject = Join-Path $testRoot 'mock-child-project'
    $mockOutput = Join-Path $testRoot 'mock-child-output'
    New-Item -ItemType Directory -Path $mockProject -Force | Out-Null
    [System.IO.File]::WriteAllText(
        (Join-Path $mockProject 'mock-child.csproj'),
        @'
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>Exe</OutputType>
    <TargetFramework>net6.0</TargetFramework>
    <AssemblyName>agentscommander</AssemblyName>
    <ImplicitUsings>disable</ImplicitUsings>
    <Nullable>disable</Nullable>
  </PropertyGroup>
</Project>
'@,
        [System.Text.UTF8Encoding]::new($false)
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $mockProject 'Program.cs'),
        $mockSource,
        [System.Text.UTF8Encoding]::new($false)
    )
    $buildOutput = & dotnet build (Join-Path $mockProject 'mock-child.csproj') --nologo -c Release -o $mockOutput 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "failed to build launcher mock executable: $($buildOutput -join [Environment]::NewLine)"
    }
    Copy-Item -LiteralPath (Join-Path $mockOutput 'agentscommander.exe') -Destination $executable
    foreach ($supportFile in @('agentscommander.dll', 'agentscommander.deps.json', 'agentscommander.runtimeconfig.json')) {
        Copy-Item -LiteralPath (Join-Path $mockOutput $supportFile) -Destination (Join-Path $artifact $supportFile)
    }

    Import-Module -Name $nativeProcessModule -Force -ErrorAction Stop
    $exportedNativeProcessCommands = @(Get-Command -Module native-process | Select-Object -ExpandProperty Name)
    if ($exportedNativeProcessCommands.Count -ne 1 -or $exportedNativeProcessCommands[0] -cne 'Start-IsolatedValidationNativeProcess') {
        throw "native process module exported unexpected commands: $($exportedNativeProcessCommands -join ', ')"
    }

    $argvProbeValues = @(
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
    $argvProbe = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $executable `
        -WorkingDirectory $artifact `
        -Arguments (@('--argv-json') + $argvProbeValues) `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment
    if ($argvProbe.ExitCode -ne 0) {
        throw 'the native process argv probe returned a nonzero exit code'
    }
    $receivedArgvProbeValues = @($argvProbe.StandardOutput | ConvertFrom-Json)
    if ($receivedArgvProbeValues.Count -eq 1 -and $receivedArgvProbeValues[0] -is [System.Array]) {
        $receivedArgvProbeValues = $receivedArgvProbeValues[0]
    }
    if ($receivedArgvProbeValues.Count -ne $argvProbeValues.Count) {
        throw 'the native process argv probe returned an unexpected argument count'
    }
    for ($index = 0; $index -lt $argvProbeValues.Count; $index++) {
        if ($receivedArgvProbeValues[$index] -cne $argvProbeValues[$index]) {
            throw "the native process argv probe changed argument index $index"
        }
    }

    $zeroArgvProbe = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $executable `
        -WorkingDirectory $artifact `
        -Arguments @('--argv-json') `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment
    $oneEmptyArgvProbe = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $executable `
        -WorkingDirectory $artifact `
        -Arguments @('--argv-json', '') `
        -StandardOutputLimitBytes 1MB `
        -StandardErrorLimitBytes 1MB `
        -RemoveAgentsCommanderEnvironment
    if ($zeroArgvProbe.StandardOutput -cne '[]' -or $oneEmptyArgvProbe.StandardOutput -cne '[""]') {
        throw 'the native process module did not distinguish zero arguments from one empty argument'
    }

    $dualStreamProbe = Start-IsolatedValidationNativeProcess `
        -Mode CaptureAndWait `
        -FilePath $executable `
        -WorkingDirectory $artifact `
        -Arguments @('--dual-stream') `
        -StandardOutputLimitBytes 256KB `
        -StandardErrorLimitBytes 256KB `
        -RemoveAgentsCommanderEnvironment
    if ($dualStreamProbe.StandardOutput.Length -ne 131072 -or $dualStreamProbe.StandardError.Length -ne 131072) {
        throw 'the native process module did not capture both streams concurrently'
    }

    try {
        Start-IsolatedValidationNativeProcess `
            -Mode CaptureAndWait `
            -FilePath $executable `
            -WorkingDirectory $artifact `
            -Arguments @('--dual-stream') `
            -StandardOutputLimitBytes 64KB `
            -StandardErrorLimitBytes 64KB `
            -RemoveAgentsCommanderEnvironment | Out-Null
        throw 'the native process module accepted output above its configured capture ceiling'
    }
    catch {
        if ($_.Exception.Message -ne 'E_ISOLATION_NATIVE_PROCESS') {
            throw
        }
    }

    $nativeParentEnvironmentName = 'AGENTSCOMMANDER_TEST_NATIVE_PARENT'
    $nativeParentEnvironmentValue = [Environment]::GetEnvironmentVariable($nativeParentEnvironmentName)
    [Environment]::SetEnvironmentVariable($nativeParentEnvironmentName, 'must-remain-parent-only')
    try {
        Start-IsolatedValidationNativeProcess `
            -Mode CaptureAndWait `
            -FilePath $executable `
            -WorkingDirectory $artifact `
            -Arguments @('--argv-json') `
            -StandardOutputLimitBytes 1MB `
            -StandardErrorLimitBytes 1MB `
            -RemoveAgentsCommanderEnvironment | Out-Null
        $nativeChildEnvironment = [System.IO.File]::ReadAllText((Join-Path $artifact 'mock-child-env.txt'))
        if ($nativeChildEnvironment.Contains($nativeParentEnvironmentName) -or
            [Environment]::GetEnvironmentVariable($nativeParentEnvironmentName) -cne 'must-remain-parent-only') {
            throw 'native process child environment cleanup leaked or mutated an AGENTSCOMMANDER_* value'
        }
    }
    finally {
        [Environment]::SetEnvironmentVariable($nativeParentEnvironmentName, $nativeParentEnvironmentValue)
    }

    $manifestPath = Join-Path $artifact 'isolated-validation-manifest.json'
    $manifest = [ordered]@{
        schema = 'isolated-validation-handoff-v1'
        baseSha = ('0' * 40)
        frozen1271Commit = ('1' * 40)
        isolatedStateRootCommit = ('2' * 40)
        combinedSourceSha = ('3' * 40)
        combinedTreeSha = ('4' * 40)
        cleanWorktree = $true
        artifactKind = 'portable-layout'
        compiledProfileSha256 = $profileHash
        utcTimestamp = [DateTime]::UtcNow.ToString('o')
        mode = 'isolated-validation-package'
        target = 'test'
        productLabel = 'Agents Commander Isolated Gates'
        bundleIdentifier = 'dev.agentscommander.isolatedgates'
        headerIdentity = 'WG-1271-ISOLATED-GATES gate-tester@AgentsCommander_1271_isolated'
        launcherCommand = '.\launch-isolated.ps1 -FixtureRoot <absolute-fixture-root> -ExpectedManifestSha256 <trusted-hash>'
        payloads = [ordered]@{
            executable = [ordered]@{
                relativePath = 'Agents Commander Isolated Gates.exe'
                sha256 = Get-Sha256 -LiteralPath $executable
            }
            profile = [ordered]@{
                relativePath = 'resources/package-profile.toml'
                sha256 = $profileHash
            }
            launcher = [ordered]@{
                relativePath = 'launch-isolated.ps1'
                sha256 = Get-Sha256 -LiteralPath $launcher
            }
            nativeProcessModule = [ordered]@{
                relativePath = 'native-process.psm1'
                sha256 = Get-Sha256 -LiteralPath $nativeProcessModule
            }
        }
    }
    Write-JsonFile -LiteralPath $manifestPath -Value $manifest
    $expectedManifestHash = Get-Sha256 -LiteralPath $manifestPath
    $executableBytes = [System.IO.File]::ReadAllBytes($executable)
    $profileBytes = [System.IO.File]::ReadAllBytes($profile)
    $launcherBytes = [System.IO.File]::ReadAllBytes($launcher)
    $nativeProcessModuleBytes = [System.IO.File]::ReadAllBytes($nativeProcessModule)
    $manifestBytes = [System.IO.File]::ReadAllBytes($manifestPath)

    [Environment]::SetEnvironmentVariable('AGENTSCOMMANDER_TEST_LAUNCHER_INHERITED', 'must-not-reach-child')
    if (Test-Path -LiteralPath (Join-Path $fixture 'app-state')) {
        throw 'the launcher test fixture pre-created the isolated app-state root'
    }
    $first = Invoke-Launcher -Launcher $launcher -FixtureRoot $fixture -ExpectedManifestSha256 $expectedManifestHash
    if (-not $first.Succeeded) {
        throw "valid portable artifact did not launch: $($first.Output)"
    }
    Wait-For-LauncherChildExit -ProcessId (($first.Output | ConvertFrom-Json).processId)
    $receipt = Join-Path $fixture 'launch-receipt.json'
    if (-not (Test-Path -LiteralPath $receipt -PathType Leaf)) {
        throw 'valid portable artifact did not write a receipt after status validation'
    }
    $firstReceiptBytes = [System.IO.File]::ReadAllBytes($receipt)
    $second = Invoke-Launcher -Launcher $launcher -FixtureRoot $fixture -ExpectedManifestSha256 $expectedManifestHash
    if (-not $second.Succeeded) {
        throw "same-root launcher relaunch failed: $($second.Output)"
    }
    Wait-For-LauncherChildExit -ProcessId (($second.Output | ConvertFrom-Json).processId)
    $secondReceiptBytes = [System.IO.File]::ReadAllBytes($receipt)
    if ($firstReceiptBytes.Length -ne $secondReceiptBytes.Length -or
        [System.BitConverter]::ToString($firstReceiptBytes) -cne [System.BitConverter]::ToString($secondReceiptBytes)) {
        throw 'same-root launcher relaunch rewrote the immutable prior receipt'
    }
    $capturedChildEnvironment = Get-Content -LiteralPath (Join-Path $artifact 'mock-child-env.txt') -Raw
    if ($null -ne $capturedChildEnvironment -and $capturedChildEnvironment.Contains('AGENTSCOMMANDER_TEST_LAUNCHER_INHERITED')) {
        throw 'launcher leaked an AGENTSCOMMANDER_* variable into the child process'
    }

    $manifestTamperFixture = Join-Path $testRoot 'manifest-tamper'
    New-Item -ItemType Directory -Path $manifestTamperFixture | Out-Null
    Add-Content -LiteralPath $manifestPath -Value 'tampered'
    Assert-LauncherFailsBeforeReceipt -Launcher $launcher -FixtureRoot $manifestTamperFixture -ExpectedManifestSha256 $expectedManifestHash -CaseName 'manifest hash'
    [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes)

    $executableTamperFixture = Join-Path $testRoot 'executable-tamper'
    New-Item -ItemType Directory -Path $executableTamperFixture | Out-Null
    Add-Content -LiteralPath $executable -Value 'tampered'
    Assert-LauncherFailsBeforeReceipt -Launcher $launcher -FixtureRoot $executableTamperFixture -ExpectedManifestSha256 $expectedManifestHash -CaseName 'executable hash'
    [System.IO.File]::WriteAllBytes($executable, $executableBytes)

    $profileTamperFixture = Join-Path $testRoot 'profile-tamper'
    New-Item -ItemType Directory -Path $profileTamperFixture | Out-Null
    Add-Content -LiteralPath $profile -Value 'tampered'
    Assert-LauncherFailsBeforeReceipt -Launcher $launcher -FixtureRoot $profileTamperFixture -ExpectedManifestSha256 $expectedManifestHash -CaseName 'profile hash'
    [System.IO.File]::WriteAllBytes($profile, $profileBytes)

    $launcherTamperFixture = Join-Path $testRoot 'launcher-tamper'
    New-Item -ItemType Directory -Path $launcherTamperFixture | Out-Null
    Add-Content -LiteralPath $launcher -Value 'tampered'
    Assert-LauncherFailsBeforeReceipt -Launcher $launcher -FixtureRoot $launcherTamperFixture -ExpectedManifestSha256 $expectedManifestHash -CaseName 'launcher hash'
    [System.IO.File]::WriteAllBytes($launcher, $launcherBytes)

    $moduleTamperFixture = Join-Path $testRoot 'native-process-module-tamper'
    New-Item -ItemType Directory -Path $moduleTamperFixture | Out-Null
    Add-Content -LiteralPath $nativeProcessModule -Value 'tampered'
    Assert-LauncherFailsBeforeReceipt -Launcher $launcher -FixtureRoot $moduleTamperFixture -ExpectedManifestSha256 $expectedManifestHash -CaseName 'native process module hash'
    [System.IO.File]::WriteAllBytes($nativeProcessModule, $nativeProcessModuleBytes)

    $layoutTamperFixture = Join-Path $testRoot 'layout-tamper'
    New-Item -ItemType Directory -Path $layoutTamperFixture | Out-Null
    $layoutTampered = $manifest | ConvertTo-Json -Depth 8 | ConvertFrom-Json
    $layoutTampered.payloads.profile.relativePath = '../outside-profile.toml'
    Write-JsonFile -LiteralPath $manifestPath -Value $layoutTampered
    $layoutTamperedExpectedHash = Get-Sha256 -LiteralPath $manifestPath
    Assert-LauncherFailsBeforeReceipt -Launcher $launcher -FixtureRoot $layoutTamperFixture -ExpectedManifestSha256 $layoutTamperedExpectedHash -CaseName 'portable resource layout'
    [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes)

    $malformedReceiptFixture = Join-Path $testRoot 'malformed-receipt'
    New-Item -ItemType Directory -Path $malformedReceiptFixture | Out-Null
    [System.IO.File]::WriteAllText(
        (Join-Path $malformedReceiptFixture 'launch-receipt.json'),
        '{"malformed":true}',
        [System.Text.UTF8Encoding]::new($false)
    )
    $malformedReceiptResult = Invoke-Launcher -Launcher $launcher -FixtureRoot $malformedReceiptFixture -ExpectedManifestSha256 $expectedManifestHash
    if ($malformedReceiptResult.Succeeded) {
        throw 'the launcher accepted a malformed existing receipt'
    }

    $substitutedReceipt = Join-Path $testRoot 'caller-controlled-receipt.json'
    try {
        & $launcher -FixtureRoot $fixture -ExpectedManifestSha256 $expectedManifestHash -ReceiptPath $substitutedReceipt 2>$null
        throw 'launcher unexpectedly accepted a caller-controlled receipt path'
    } catch {
        if ($_.Exception.Message -eq 'launcher unexpectedly accepted a caller-controlled receipt path') {
            throw
        }
    }
    if (Test-Path -LiteralPath $substitutedReceipt) {
        throw 'launcher created a caller-controlled receipt path'
    }

    [pscustomobject]@{
        result = 'passed'
        cases = @(
            'whitespace and metacharacter fixture root',
            'same-root immutable receipt re-launch',
            'manifest, executable, profile, launcher, module, and layout tampering',
            'malformed receipt rejection before child launch',
            'caller-controlled receipt path rejection',
            'child-only AGENTSCOMMANDER_* cleanup'
        )
    } | ConvertTo-Json -Depth 4
} finally {
    [Environment]::SetEnvironmentVariable('AGENTSCOMMANDER_TEST_LAUNCHER_INHERITED', $originalInherited)
    if (Test-Path -LiteralPath $testRoot) {
        for ($attempt = 1; $attempt -le 20; $attempt++) {
            try {
                Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction Stop
                break
            } catch {
                if ($attempt -eq 20) {
                    Write-Warning "launcher regression fixture cleanup retained: $testRoot"
                } else {
                    Start-Sleep -Milliseconds 50
                }
            }
        }
    }
}
