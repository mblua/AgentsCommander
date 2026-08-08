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
        return [pscustomobject]@{ Succeeded = $true; Output = ($output -join [Environment]::NewLine) }
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

    $manifestPath = Join-Path $artifact 'isolated-validation-handoff.json'
    $manifest = [ordered]@{
        schemaVersion = 1
        artifactKind = 'portable-layout'
        executableFileName = 'agentscommander.exe'
        executableSha256 = Get-Sha256 -LiteralPath $executable
        profileResourceRelativePath = 'resources/isolated-validation/package-profile.toml'
        compiledProfileSha256 = $profileHash
        bundledProfileSha256 = $profileHash
        installedProfileSha256 = $profileHash
    }
    Write-JsonFile -LiteralPath $manifestPath -Value $manifest
    $expectedManifestHash = Get-Sha256 -LiteralPath $manifestPath
    $executableBytes = [System.IO.File]::ReadAllBytes($executable)
    $profileBytes = [System.IO.File]::ReadAllBytes($profile)
    $manifestBytes = [System.IO.File]::ReadAllBytes($manifestPath)

    [Environment]::SetEnvironmentVariable('AGENTSCOMMANDER_TEST_LAUNCHER_INHERITED', 'must-not-reach-child')
    $first = Invoke-Launcher -Launcher $launcher -FixtureRoot $fixture -ExpectedManifestSha256 $expectedManifestHash
    if (-not $first.Succeeded) {
        throw "valid portable artifact did not launch: $($first.Output)"
    }
    Wait-For-LauncherChildExit -ProcessId (($first.Output | ConvertFrom-Json).processId)
    $receipt = Join-Path $fixture 'launch-receipt.json'
    if (-not (Test-Path -LiteralPath $receipt -PathType Leaf)) {
        throw 'valid portable artifact did not write a receipt after status validation'
    }
    [System.IO.File]::WriteAllText($receipt, '{"sentinel":"must-be-replaced"}', [System.Text.UTF8Encoding]::new($false))
    $second = Invoke-Launcher -Launcher $launcher -FixtureRoot $fixture -ExpectedManifestSha256 $expectedManifestHash
    if (-not $second.Succeeded) {
        throw "same-root launcher relaunch failed: $($second.Output)"
    }
    Wait-For-LauncherChildExit -ProcessId (($second.Output | ConvertFrom-Json).processId)
    if ((Get-Content -LiteralPath $receipt -Raw).Contains('must-be-replaced')) {
        throw 'same-root launcher relaunch did not atomically replace the prior receipt'
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

    $layoutTamperFixture = Join-Path $testRoot 'layout-tamper'
    New-Item -ItemType Directory -Path $layoutTamperFixture | Out-Null
    $layoutTampered = [ordered]@{}
    foreach ($property in $manifest.GetEnumerator()) {
        $layoutTampered[$property.Key] = $property.Value
    }
    $layoutTampered.profileResourceRelativePath = '../outside-profile.toml'
    Write-JsonFile -LiteralPath $manifestPath -Value $layoutTampered
    $layoutTamperedExpectedHash = Get-Sha256 -LiteralPath $manifestPath
    Assert-LauncherFailsBeforeReceipt -Launcher $launcher -FixtureRoot $layoutTamperFixture -ExpectedManifestSha256 $layoutTamperedExpectedHash -CaseName 'portable resource layout'
    [System.IO.File]::WriteAllBytes($manifestPath, $manifestBytes)

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
            'same-root receipt replacement',
            'manifest, executable, profile, and layout tampering',
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
