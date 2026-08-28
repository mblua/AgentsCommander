[CmdletBinding()]
param(
    [string]$LogDir = "artifacts\cli-release-smoke",
    [string]$Token = "00000000-0000-0000-0000-000000000000",
    [string]$Root = (Join-Path $env:TEMP "ac-release-smoke-$([guid]::NewGuid().ToString('N'))")
)

$ErrorActionPreference = "Continue"

$scriptDir = $PSScriptRoot
$repoRoot = Split-Path -Parent $scriptDir
if (-not [System.IO.Path]::IsPathRooted($LogDir)) {
    $LogDir = Join-Path $repoRoot $LogDir
}
if (-not [System.IO.Path]::IsPathRooted($Root)) {
    $Root = Join-Path $repoRoot $Root
}

New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

if (-not $IsWindows -and $env:OS -ne "Windows_NT") {
    Write-Host "SKIP: Windows release CLI smoke only runs on Windows" -ForegroundColor Yellow
    [pscustomobject]@{
        status = "skipped"
        reason = "not-windows"
        logDir = $LogDir
        invocations = @()
    } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $LogDir "summary.json") -Encoding UTF8
    exit 0
}

function Convert-ToSafeName {
    param([Parameter(Mandatory=$true)] [string]$Name)
    $Name -replace '[^A-Za-z0-9_.-]', '_'
}

function Get-Issue1577FileSha256 {
    param([Parameter(Mandatory=$true)] [string]$Path)

    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $sha256 = [System.Security.Cryptography.SHA256]::Create()
        try {
            [System.BitConverter]::ToString($sha256.ComputeHash($stream)).Replace("-", "")
        } finally {
            $sha256.Dispose()
        }
    } finally {
        $stream.Dispose()
    }
}

function Get-Issue1577TreeSnapshot {
    param(
        [Parameter(Mandatory=$true)] [hashtable]$Roots
    )

    $records = New-Object System.Collections.Generic.List[object]
    foreach ($rootName in @($Roots.Keys | Sort-Object)) {
        $rootPath = [System.IO.Path]::GetFullPath([string]$Roots[$rootName])
        if (-not (Test-Path -LiteralPath $rootPath -PathType Container)) {
            throw "snapshot root is missing or not a directory: $rootPath"
        }
        $records.Add([pscustomobject]@{
            root = $rootName
            relative = "."
            kind = "directory"
            target = $null
            length = $null
            sha256 = $null
        }) | Out-Null

        $pending = New-Object 'System.Collections.Generic.Stack[string]'
        $pending.Push($rootPath)
        while ($pending.Count -gt 0) {
            $directory = $pending.Pop()
            foreach ($entryPath in [System.IO.Directory]::EnumerateFileSystemEntries($directory)) {
                $item = Get-Item -Force -LiteralPath $entryPath -ErrorAction Stop
                $fullPath = [System.IO.Path]::GetFullPath($item.FullName)
                $relative = $fullPath.Substring($rootPath.Length).TrimStart([char[]]"\/") -replace '\\', '/'
                $isReparse = (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)
                if ($isReparse) {
                    $target = @($item.Target) -join '|'
                    $records.Add([pscustomobject]@{
                        root = $rootName
                        relative = $relative
                        kind = "reparse:$($item.LinkType)"
                        target = $target
                        length = $null
                        sha256 = $null
                    }) | Out-Null
                } elseif ($item.PSIsContainer) {
                    $records.Add([pscustomobject]@{
                        root = $rootName
                        relative = $relative
                        kind = "directory"
                        target = $null
                        length = $null
                        sha256 = $null
                    }) | Out-Null
                    $pending.Push($fullPath)
                } else {
                    $records.Add([pscustomobject]@{
                        root = $rootName
                        relative = $relative
                        kind = "file"
                        target = $null
                        length = [int64]$item.Length
                        sha256 = (Get-Issue1577FileSha256 -Path $fullPath)
                    }) | Out-Null
                }
            }
        }
    }

    @($records.ToArray() | Sort-Object root, relative) | ConvertTo-Json -Depth 5 -Compress
}

function Set-Issue1577LogText {
    param(
        [Parameter(Mandatory=$true)] [string]$Path,
        [AllowEmptyString()] [string]$Text
    )
    [System.IO.File]::WriteAllText(
        $Path,
        $Text,
        [System.Text.UTF8Encoding]::new($false)
    )
}

function Invoke-Issue1577MarkerGate {
    param(
        [Parameter(Mandatory=$true)] [string]$BinaryPath,
        [Parameter(Mandatory=$true)] [string]$Token,
        [Parameter(Mandatory=$true)] [string]$Root,
        [Parameter(Mandatory=$true)] [string]$LogDir
    )

    $errors = New-Object System.Collections.Generic.List[string]
    $caseLogDir = Join-Path $LogDir "issue-1577-marker-gate"
    New-Item -ItemType Directory -Force -Path $caseLogDir | Out-Null
    $fixtureRoot = Join-Path $Root "issue-1577-marker-$([guid]::NewGuid().ToString('N'))"
    $fixtureFull = [System.IO.Path]::GetFullPath($fixtureRoot)
    $allowedRoot = [System.IO.Path]::GetFullPath($Root).TrimEnd([char[]]"\/") + [System.IO.Path]::DirectorySeparatorChar
    if (-not $fixtureFull.StartsWith($allowedRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "refusing fixture outside smoke root: fixture=$fixtureFull root=$allowedRoot"
    }

    $binDir = Join-Path $fixtureRoot "bin"
    $cliRoot = Join-Path $fixtureRoot "cli-root"
    $copiedBinary = Join-Path $binDir "agentscommander_issue1577_cli.exe"
    $candidate = Join-Path $binDir ".agentscommander_issue1577_cli"
    $marker = Join-Path $binDir "portable.txt"
    $junctionTarget = Join-Path $fixtureRoot "junction-target"
    $stdoutPath = Join-Path $caseLogDir "stdout.txt"
    $stderrPath = Join-Path $caseLogDir "stderr.txt"
    $baselinePath = Join-Path $caseLogDir "snapshot-before.json"
    $afterPath = Join-Path $caseLogDir "snapshot-after.json"
    $commandPath = Join-Path $caseLogDir "command.txt"
    $exitCode = $null
    $stdout = ""
    $stderr = ""
    $timedOut = $false
    $processStarted = $false

    try {
        New-Item -ItemType Directory -Force -Path $binDir | Out-Null
        New-Item -ItemType Directory -Force -Path $cliRoot | Out-Null
        New-Item -ItemType Directory -Force -Path $junctionTarget | Out-Null
        Copy-Item -LiteralPath $BinaryPath -Destination $copiedBinary
        if (Test-Path -LiteralPath $candidate) {
            throw "adjacent candidate was not fresh: $candidate"
        }
        New-Item -ItemType Junction -Path $marker -Target $junctionTarget -ErrorAction Stop | Out-Null
        Remove-Item -LiteralPath $junctionTarget -Force -ErrorAction Stop

        $markerItem = Get-Item -Force -LiteralPath $marker -ErrorAction Stop
        $markerIsReparse = (($markerItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0)
        if (-not $markerIsReparse -or $markerItem.LinkType -ne "Junction") {
            throw "portable marker is not the required junction/reparse entry: $marker"
        }
        if (Test-Path -LiteralPath $junctionTarget) {
            throw "junction target still exists: $junctionTarget"
        }

        $snapshotRoots = @{ bin = $binDir; cliRoot = $cliRoot }
        $before = Get-Issue1577TreeSnapshot -Roots $snapshotRoots
        Set-Issue1577LogText -Path $baselinePath -Text $before

        $psi = [System.Diagnostics.ProcessStartInfo]::new()
        $psi.FileName = $copiedBinary
        $psi.Arguments = "list-peers-lean --token `"$Token`" --root `"$cliRoot`""
        $psi.WorkingDirectory = $cliRoot
        $psi.UseShellExecute = $false
        $psi.CreateNoWindow = $true
        $psi.RedirectStandardOutput = $true
        $psi.RedirectStandardError = $true
        $psi.EnvironmentVariables.Remove("AGENTSCOMMANDER_CONFIG_DIR")
        $psi.EnvironmentVariables.Remove("AGENTSCOMMANDER_TEST_CONFIG_DIR")
        Set-Issue1577LogText -Path $commandPath -Text "$copiedBinary $($psi.Arguments)"

        $process = [System.Diagnostics.Process]::new()
        $process.StartInfo = $psi
        if (-not $process.Start()) {
            throw "Process.Start returned false for $copiedBinary"
        }
        $processStarted = $true
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()

        if (-not $process.WaitForExit(15000)) {
            $timedOut = $true
            try { $process.Kill() } catch { $errors.Add("timeout kill failed: $($_.Exception.Message)") | Out-Null }
            if (-not $process.WaitForExit(5000)) {
                $errors.Add("timed-out child did not reap within 5 seconds: pid=$($process.Id)") | Out-Null
            }
        }

        if ($process.HasExited) {
            $exitCode = $process.ExitCode
        }
        if (-not $stdoutTask.Wait(5000)) {
            $errors.Add("stdout capture did not finish within 5 seconds") | Out-Null
        } else {
            $stdout = $stdoutTask.Result
        }
        if (-not $stderrTask.Wait(5000)) {
            $errors.Add("stderr capture did not finish within 5 seconds") | Out-Null
        } else {
            $stderr = $stderrTask.Result
        }
        Set-Issue1577LogText -Path $stdoutPath -Text $stdout
        Set-Issue1577LogText -Path $stderrPath -Text $stderr

        if ($timedOut) {
            $errors.Add("copied release CLI timed out after 15 seconds") | Out-Null
        }
        if ($exitCode -ne 1) {
            $errors.Add("expected exit code 1, got $exitCode") | Out-Null
        }
        $stdoutBytes = [System.Text.Encoding]::UTF8.GetByteCount($stdout)
        if ($stdoutBytes -ne 0) {
            $errors.Add("expected byte-empty stdout, got $stdoutBytes UTF-8 byte(s)") | Out-Null
        }

        $nativeReason = ([System.ComponentModel.Win32Exception]::new(2)).Message.Trim()
        if (-not $nativeReason.EndsWith(".")) {
            $nativeReason += "."
        }
        $osReason = "$nativeReason (os error 2)"
        $expectedStderr = "AgentsCommander cannot start because configuration directory `"$candidate`" could not be safely selected: could not resolve portable marker symlink target metadata `"$marker`" after 1 attempt(s): $osReason. Set AGENTSCOMMANDER_CONFIG_DIR to a writable directory and restart. Portable marker path: `"$marker`".`n"
        if ($stderr -cne $expectedStderr) {
            $errors.Add("stderr did not match the exact marker-indeterminate startup message") | Out-Null
        }
        foreach ($forbidden in @("[log] file logging to", "[instance-gitignore]", "panicked at", "stack backtrace:", '"ok":', '"peers":')) {
            if ($stderr.Contains($forbidden)) {
                $errors.Add("stderr contained forbidden startup residue: $forbidden") | Out-Null
            }
        }

        $after = Get-Issue1577TreeSnapshot -Roots $snapshotRoots
        Set-Issue1577LogText -Path $afterPath -Text $after
        if ($after -cne $before) {
            $errors.Add("bin/cli-root snapshot changed across the child invocation") | Out-Null
        }
        if (Test-Path -LiteralPath $candidate) {
            $errors.Add("adjacent candidate appeared: $candidate") | Out-Null
        }
        $criticalNames = @("app.log", ".gitignore", "master-token.txt", "web-token.txt", "daemon.pid", "settings.json", "app-outbox-path.txt")
        foreach ($criticalName in $criticalNames) {
            if (@(Get-ChildItem -Force -Recurse -LiteralPath $binDir, $cliRoot -ErrorAction SilentlyContinue | Where-Object { $_.Name -ceq $criticalName }).Count -gt 0) {
                $errors.Add("critical startup artifact appeared: $criticalName") | Out-Null
            }
        }
        if (@(Get-ChildItem -Force -Recurse -LiteralPath $binDir, $cliRoot -ErrorAction SilentlyContinue | Where-Object { $_.Name -like ".agentscommander-write-probe-*.tmp" }).Count -gt 0) {
            $errors.Add("write-probe residue appeared") | Out-Null
        }
    } catch {
        $errors.Add($_.Exception.ToString()) | Out-Null
    } finally {
        if ($processStarted -and $null -ne $process) {
            $process.Dispose()
        }
        if (Test-Path -LiteralPath $fixtureRoot) {
            try {
                Remove-Item -LiteralPath $fixtureRoot -Recurse -Force -ErrorAction Stop
            } catch {
                $errors.Add("fixture cleanup failed for ${fixtureRoot}: $($_.Exception.Message)") | Out-Null
            }
        }
    }

    [pscustomobject]@{
        binaryPath = $copiedBinary
        shell = $null
        status = $(if ($errors.Count -eq 0) { "passed" } else { "failed" })
        exitCode = $exitCode
        root = $fixtureRoot
        logDir = $caseLogDir
        reason = $(if ($errors.Count -eq 0) { $null } else { $errors -join " | " })
        stdoutPath = $stdoutPath
        stderrPath = $stderrPath
        snapshotBeforePath = $baselinePath
        snapshotAfterPath = $afterPath
        markerPath = $marker
        candidatePath = $candidate
        cliRoot = $cliRoot
    }
}

$releaseDir = Join-Path $repoRoot "target\release"
$binaries = @(
    (Join-Path $releaseDir "agentscommander.exe"),
    (Join-Path $releaseDir "agentscommander_testeable.exe")
)
$shells = @("powershell.exe", "pwsh.exe")
$results = New-Object System.Collections.Generic.List[object]
$failed = 0
$passed = 0
$skipped = 0

$missingBinaries = @($binaries | Where-Object { -not (Test-Path -LiteralPath $_ -PathType Leaf) })
if ($missingBinaries.Count -gt 0) {
    foreach ($binary in $missingBinaries) {
        Write-Host "FAIL: expected release smoke binary missing: $binary" -ForegroundColor Red
        Write-Host "Run npm run build:prod before smoke:cli-release-windows." -ForegroundColor Red
        $results.Add([pscustomobject]@{
            binaryPath = $binary
            shell = $null
            status = "failed"
            exitCode = $null
            root = $null
            logDir = $null
            reason = "missing-binary"
        }) | Out-Null
    }
    [pscustomobject]@{
        passed = 0
        skipped = 0
        failed = $missingBinaries.Count
        logDir = $LogDir
        invocations = @($results.ToArray())
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $LogDir "summary.json") -Encoding UTF8
    exit 1
}

foreach ($binary in $binaries) {
    $binaryStem = Convert-ToSafeName -Name ([System.IO.Path]::GetFileNameWithoutExtension($binary))
    foreach ($shell in $shells) {
        $shellCmd = Get-Command $shell -ErrorAction SilentlyContinue
        $shellStem = Convert-ToSafeName -Name ([System.IO.Path]::GetFileNameWithoutExtension($shell))
        $caseLogDir = Join-Path $LogDir (Join-Path $binaryStem $shellStem)
        $caseRoot = Join-Path $Root (Join-Path $binaryStem $shellStem)
        New-Item -ItemType Directory -Force -Path $caseLogDir | Out-Null
        New-Item -ItemType Directory -Force -Path $caseRoot | Out-Null

        if ($null -eq $shellCmd) {
            if ($shell -ieq "pwsh.exe") {
                Write-Host "SKIP: optional shell not found: $shell" -ForegroundColor Yellow
                $skipped++
                $results.Add([pscustomobject]@{
                    binaryPath = $binary
                    shell = $shell
                    status = "skipped"
                    exitCode = $null
                    root = $caseRoot
                    logDir = $caseLogDir
                    reason = "optional-shell-missing"
                }) | Out-Null
                continue
            }

            Write-Host "FAIL: required shell not found: $shell" -ForegroundColor Red
            $failed++
            $results.Add([pscustomobject]@{
                binaryPath = $binary
                shell = $shell
                status = "failed"
                exitCode = $null
                root = $caseRoot
                logDir = $caseLogDir
                reason = "required-shell-missing"
            }) | Out-Null
            continue
        }

        $publicConfig = Join-Path $caseRoot "public-config"
        $debugCanary = Join-Path $caseRoot "debug-canary"
        $adjacentCandidate = Join-Path (Split-Path -Parent $binary) ".$([System.IO.Path]::GetFileNameWithoutExtension($binary))"
        $assertionFailures = New-Object System.Collections.Generic.List[string]
        foreach ($freshPath in @($publicConfig, $debugCanary, $adjacentCandidate)) {
            if (Test-Path -LiteralPath $freshPath) {
                $assertionFailures.Add("required fresh path already exists: $freshPath") | Out-Null
            }
        }

        $publicWasPresent = Test-Path Env:AGENTSCOMMANDER_CONFIG_DIR
        $publicPrevious = [System.Environment]::GetEnvironmentVariable("AGENTSCOMMANDER_CONFIG_DIR", "Process")
        $debugWasPresent = Test-Path Env:AGENTSCOMMANDER_TEST_CONFIG_DIR
        $debugPrevious = [System.Environment]::GetEnvironmentVariable("AGENTSCOMMANDER_TEST_CONFIG_DIR", "Process")
        $exitCode = $null
        if ($assertionFailures.Count -eq 0) {
            try {
                [System.Environment]::SetEnvironmentVariable("AGENTSCOMMANDER_CONFIG_DIR", $publicConfig, "Process")
                [System.Environment]::SetEnvironmentVariable("AGENTSCOMMANDER_TEST_CONFIG_DIR", $debugCanary, "Process")
                & (Join-Path $scriptDir "smoke-cli-powershell.ps1") `
                    -BinaryPath $binary `
                    -Shell $shell `
                    -Token $Token `
                    -Root $caseRoot `
                    -LogDir $caseLogDir
                $exitCode = $LASTEXITCODE
            } finally {
                if ($publicWasPresent) {
                    [System.Environment]::SetEnvironmentVariable("AGENTSCOMMANDER_CONFIG_DIR", $publicPrevious, "Process")
                } else {
                    [System.Environment]::SetEnvironmentVariable("AGENTSCOMMANDER_CONFIG_DIR", $null, "Process")
                }
                if ($debugWasPresent) {
                    [System.Environment]::SetEnvironmentVariable("AGENTSCOMMANDER_TEST_CONFIG_DIR", $debugPrevious, "Process")
                } else {
                    [System.Environment]::SetEnvironmentVariable("AGENTSCOMMANDER_TEST_CONFIG_DIR", $null, "Process")
                }
            }
        }

        $publicRestored = Test-Path Env:AGENTSCOMMANDER_CONFIG_DIR
        $debugRestored = Test-Path Env:AGENTSCOMMANDER_TEST_CONFIG_DIR
        if ($publicRestored -ne $publicWasPresent -or ($publicWasPresent -and $env:AGENTSCOMMANDER_CONFIG_DIR -cne $publicPrevious)) {
            $assertionFailures.Add("AGENTSCOMMANDER_CONFIG_DIR was not restored exactly") | Out-Null
        }
        if ($debugRestored -ne $debugWasPresent -or ($debugWasPresent -and $env:AGENTSCOMMANDER_TEST_CONFIG_DIR -cne $debugPrevious)) {
            $assertionFailures.Add("AGENTSCOMMANDER_TEST_CONFIG_DIR was not restored exactly") | Out-Null
        }
        if (-not (Test-Path -LiteralPath (Join-Path $publicConfig "app.log") -PathType Leaf)) {
            $assertionFailures.Add("public override app.log was not created: $publicConfig") | Out-Null
        }
        if (Test-Path -LiteralPath $debugCanary) {
            $assertionFailures.Add("debug override canary was created: $debugCanary") | Out-Null
        }
        if (Test-Path -LiteralPath $adjacentCandidate) {
            $assertionFailures.Add("executable-adjacent canary was created: $adjacentCandidate") | Out-Null
        }
        if ($exitCode -ne 0) {
            $assertionFailures.Add("existing CLI smoke exited $exitCode") | Out-Null
        }
        $assertionRecord = [pscustomobject]@{
            publicConfig = $publicConfig
            publicAppLog = Join-Path $publicConfig "app.log"
            debugCanary = $debugCanary
            adjacentCandidate = $adjacentCandidate
            environmentRestored = ($publicRestored -eq $publicWasPresent -and $debugRestored -eq $debugWasPresent)
            failures = @($assertionFailures.ToArray())
        }
        $assertionRecord | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $caseLogDir "issue-1577-public-override.json") -Encoding UTF8

        if ($assertionFailures.Count -eq 0) {
            Write-Host "PASS: $binary under $shell" -ForegroundColor Green
            $passed++
            $status = "passed"
        } else {
            Write-Host "FAIL: $binary under $shell exited $exitCode" -ForegroundColor Red
            $failed++
            $status = "failed"
        }

        $results.Add([pscustomobject]@{
            binaryPath = $binary
            shell = $shell
            shellPath = $shellCmd.Source
            status = $status
            exitCode = $exitCode
            root = $caseRoot
            logDir = $caseLogDir
            reason = $(if ($assertionFailures.Count -eq 0) { $null } else { $assertionFailures -join " | " })
            publicConfig = $publicConfig
            debugCanary = $debugCanary
            adjacentCandidate = $adjacentCandidate
        }) | Out-Null
    }
}

$markerGate = Invoke-Issue1577MarkerGate -BinaryPath $binaries[0] -Token $Token -Root $Root -LogDir $LogDir
$results.Add($markerGate) | Out-Null
if ($markerGate.status -eq "passed") {
    Write-Host "PASS: #1577 copied release CLI marker-indeterminate preflight" -ForegroundColor Green
    $passed++
} else {
    Write-Host "FAIL: #1577 copied release CLI marker-indeterminate preflight: $($markerGate.reason)" -ForegroundColor Red
    $failed++
}

$summaryPath = Join-Path $LogDir "summary.json"
[pscustomobject]@{
    passed = $passed
    skipped = $skipped
    failed = $failed
    logDir = $LogDir
    invocations = @($results.ToArray())
} | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $summaryPath -Encoding UTF8

Write-Host "Windows release CLI smoke summary: passed=$passed skipped=$skipped failed=$failed logs=$LogDir"
Write-Host "Wrapper summary: $summaryPath"

if ($failed -gt 0) {
    exit 1
}
exit 0
