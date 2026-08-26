[CmdletBinding()]
param(
    [Parameter(Mandatory=$true)] [string]$BinaryPath,
    [string]$Root = (Join-Path $env:TEMP "ac-ui-smoke-$([guid]::NewGuid().ToString('N'))"),
    [string]$LogDir = "artifacts\ui-automation-smoke",
    [int]$TimeoutMs = 30000,
    [switch]$AuthorizeInteractiveDesktop
)

$ErrorActionPreference = "Stop"
$scriptDir = $PSScriptRoot
$repoRoot = Split-Path -Parent $scriptDir
if (-not [System.IO.Path]::IsPathRooted($BinaryPath)) { $BinaryPath = Join-Path $repoRoot $BinaryPath }
if (-not [System.IO.Path]::IsPathRooted($Root)) { $Root = Join-Path $repoRoot $Root }
if (-not [System.IO.Path]::IsPathRooted($LogDir)) { $LogDir = Join-Path $repoRoot $LogDir }

if (-not $IsWindows -and $env:OS -ne "Windows_NT") {
    Write-Host "SKIP: UI automation smoke is Windows-only" -ForegroundColor Yellow
    exit 0
}
if (-not $AuthorizeInteractiveDesktop) {
    throw "Refusing to launch packaged GUI processes without -AuthorizeInteractiveDesktop."
}
if (-not (Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
    throw "Testable feature binary not found: $BinaryPath"
}
if ([System.IO.Path]::GetFileName($BinaryPath) -cne "agentscommander_testeable.exe") {
    throw "BinaryPath must name the exact feature artifact agentscommander_testeable.exe."
}
if ($TimeoutMs -lt 1000 -or $TimeoutMs -gt 120000) {
    throw "TimeoutMs must be between 1000 and 120000."
}

New-Item -ItemType Directory -Force -Path $Root | Out-Null
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Convert-ToCommandLineArgument {
    param([AllowEmptyString()] [string]$Value)
    if ($Value.Length -gt 0 -and $Value -notmatch '[\s"]') { return $Value }
    $builder = [Text.StringBuilder]::new()
    [void]$builder.Append('"')
    $slashes = 0
    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') {
            $slashes++
            continue
        }
        if ($character -eq '"') {
            [void]$builder.Append(('\' * (($slashes * 2) + 1)))
            [void]$builder.Append('"')
            $slashes = 0
            continue
        }
        if ($slashes -gt 0) { [void]$builder.Append(('\' * $slashes)); $slashes = 0 }
        [void]$builder.Append($character)
    }
    if ($slashes -gt 0) { [void]$builder.Append(('\' * ($slashes * 2))) }
    [void]$builder.Append('"')
    $builder.ToString()
}

$script:invocationNumber = 0
function Invoke-NativeCapture {
    param(
        [Parameter(Mandatory=$true)] [string]$Executable,
        [string[]]$Arguments = @()
    )
    $script:invocationNumber++
    $start = [Diagnostics.ProcessStartInfo]::new()
    $start.FileName = $Executable
    $start.Arguments = (($Arguments | ForEach-Object { Convert-ToCommandLineArgument $_ }) -join ' ')
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    Assert-True $process.Start() "Failed to start $Executable"
    $stdoutTask = $process.StandardOutput.ReadToEndAsync()
    $stderrTask = $process.StandardError.ReadToEndAsync()
    if (-not $process.WaitForExit($TimeoutMs)) {
        $process.Kill()
        $process.WaitForExit()
        throw "CLI invocation timed out: $($Arguments -join ' ')"
    }
    $stdout = $stdoutTask.Result
    $stderr = $stderrTask.Result
    $stem = '{0:D3}' -f $script:invocationNumber
    [IO.File]::WriteAllText((Join-Path $LogDir "$stem.stdout.txt"), $stdout, [Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText((Join-Path $LogDir "$stem.stderr.txt"), $stderr, [Text.UTF8Encoding]::new($false))
    [pscustomobject]@{ ExitCode = $process.ExitCode; Stdout = $stdout; Stderr = $stderr }
}

function Invoke-UiJson {
    param(
        [Parameter(Mandatory=$true)] [string]$Executable,
        [Parameter(Mandatory=$true)] [string[]]$Arguments,
        [int[]]$AllowedExitCodes = @(0)
    )
    $capture = Invoke-NativeCapture -Executable $Executable -Arguments $Arguments
    Assert-True ($AllowedExitCodes -contains $capture.ExitCode) "Unexpected exit $($capture.ExitCode): $($Arguments -join ' ')"
    Assert-True ($capture.Stderr.Length -eq 0) "UI CLI stderr was not empty: $($Arguments -join ' ')"
    $lines = @($capture.Stdout -split '\r?\n' | Where-Object { $_.Length -gt 0 })
    Assert-True ($lines.Count -eq 1) "UI CLI did not emit exactly one JSON line: $($Arguments -join ' ')"
    $json = $lines[0] | ConvertFrom-Json
    [pscustomobject]@{ ExitCode = $capture.ExitCode; Text = $capture.Stdout; Json = $json }
}

function Get-ConfigDir {
    param([string]$Executable)
    $stem = [IO.Path]::GetFileNameWithoutExtension($Executable)
    Join-Path (Split-Path -Parent $Executable) ".$stem"
}

function Start-UiGui {
    param([string]$Executable, [switch]$Enabled, [string]$Name)
    $stdout = Join-Path $LogDir "$Name.gui.stdout.txt"
    $stderr = Join-Path $LogDir "$Name.gui.stderr.txt"
    $arguments = if ($Enabled) { @("--ui-automation") } else { @() }
    Start-Process -FilePath $Executable -ArgumentList $arguments -WindowStyle Hidden `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr -PassThru
}

function Wait-UiSession {
    param([Diagnostics.Process]$Process, [string]$Executable)
    $path = Join-Path (Get-ConfigDir $Executable) "ui-automation\session.json"
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    do {
        $Process.Refresh()
        if ($Process.HasExited) { throw "GUI exited before publishing $path (exit=$($Process.ExitCode))" }
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            try {
                $raw = [IO.File]::ReadAllText($path)
                $session = $raw | ConvertFrom-Json
                if ($session.schemaVersion -eq 1 -and $session.pid -eq $Process.Id) {
                    $started = [DateTimeOffset]::new($Process.StartTime.ToUniversalTime()).ToUnixTimeMilliseconds()
                    Assert-True ($session.startedAtUnixMs -eq $started) "Published process creation epoch did not match Process.StartTime."
                    Assert-True ($session.exePath -ceq $Executable) "Published executable claim did not match the staged artifact."
                    return [pscustomobject]@{ Path = $path; Raw = $raw; Session = $session }
                }
            } catch {}
        }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Timed out waiting for live UI automation session: $path"
}

function Stop-UiGui {
    param([Diagnostics.Process]$Process)
    $Process.Refresh()
    if (-not $Process.HasExited) {
        Stop-Process -Id $Process.Id -Force
        $Process.WaitForExit()
    }
}

function Assert-Collision {
    param([string]$Executable, [string[]]$Arguments, [string]$Label)
    $capture = Invoke-NativeCapture -Executable $Executable -Arguments $Arguments
    $expected = '{"ok":false,"error":"automation_config_in_use","message":"Another testable AgentsCommander process already owns this configuration."}' + [Environment]::NewLine
    Assert-True ($capture.ExitCode -eq 1) "$Label did not exit 1."
    Assert-True ($capture.Stdout.Length -eq 0) "$Label wrote stdout."
    Assert-True ($capture.Stderr -ceq $expected) "$Label collision JSON framing changed."
}

function Wait-NoAutomationArtifacts {
    param([string]$Executable)
    $automation = Join-Path (Get-ConfigDir $Executable) "ui-automation"
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    do {
        $files = if (Test-Path -LiteralPath $automation) {
            @(Get-ChildItem -LiteralPath $automation -Recurse -File -ErrorAction SilentlyContinue)
        } else { @() }
        if ($files.Count -eq 0) { return }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Automation artifacts remained after shutdown: $automation"
}

function Get-SubstringPaths {
    param($Value, [string]$Needle, [string]$Path = '$')
    $found = New-Object System.Collections.Generic.List[string]
    if ($null -eq $Value) { return @() }
    if ($Value -is [string]) {
        if ($Value.Contains($Needle)) { $found.Add($Path) }
        return @($found)
    }
    if ($Value -is [Collections.IDictionary]) {
        foreach ($key in $Value.Keys) {
            foreach ($match in Get-SubstringPaths $Value[$key] $Needle "$Path.$key") { $found.Add($match) }
        }
        return @($found)
    }
    if ($Value -is [Collections.IEnumerable]) {
        $index = 0
        foreach ($item in $Value) {
            foreach ($match in Get-SubstringPaths $item $Needle "$Path[$index]") { $found.Add($match) }
            $index++
        }
        return @($found)
    }
    foreach ($property in $Value.PSObject.Properties) {
        foreach ($match in Get-SubstringPaths $property.Value $Needle "$Path.$($property.Name)") { $found.Add($match) }
    }
    @($found)
}

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class AcUiSmokeNative {
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT point);
}
'@

$caseA = Join-Path $Root "case-a"
$caseB = Join-Path $Root "case-b"
New-Item -ItemType Directory -Force -Path $caseA | Out-Null
New-Item -ItemType Directory -Force -Path $caseB | Out-Null
$binaryA = Join-Path $caseA "agentscommander_testeable.exe"
$binaryB = Join-Path $caseB "agentscommander_testeable.exe"
Copy-Item -LiteralPath $BinaryPath -Destination $binaryA -Force
Copy-Item -LiteralPath $BinaryPath -Destination $binaryB -Force

$guiA = $null
$guiB = $null
$requestless = $null
$allCli = New-Object System.Collections.Generic.List[object]
try {
    $guiA = Start-UiGui -Executable $binaryA -Enabled -Name "enabled-a"
    $sessionA = Wait-UiSession -Process $guiA -Executable $binaryA
    Assert-Collision -Executable $binaryA -Arguments @() -Label "enabled-first/requestless-second"

    $guiB = Start-UiGui -Executable $binaryB -Enabled -Name "enabled-b"
    $sessionB = Wait-UiSession -Process $guiB -Executable $binaryB
    Assert-True ($sessionA.Session.pid -ne $sessionB.Session.pid) "Distinct configs reused a process."
    Assert-True ($sessionA.Session.configDir -cne $sessionB.Session.configDir) "Distinct configs shared a config identity."

    foreach ($pair in @(@($binaryA, $sessionA), @($binaryB, $sessionB))) {
        $capabilities = Invoke-UiJson -Executable $pair[0] -Arguments @("ui-capabilities")
        $allCli.Add($capabilities) | Out-Null
        Assert-True $capabilities.Json.ok "Capabilities failed."
        Assert-True ($capabilities.Json.pid -eq $pair[1].Session.pid) "CLI crossed config/process isolation."
        Assert-True ($capabilities.Json.schemaVersion -eq 1) "Capabilities schema changed."
        Assert-True ($capabilities.Json.roles.Count -eq 25) "Capabilities role inventory changed."
    }

    $prefixes = @(
        "AC_UI_PREFIX_ORDINARY_$([guid]::NewGuid())",
        "AC_UI_PREFIX_SECRET_$([guid]::NewGuid())",
        "C:\AC_UI_PREFIX_PATH_$([guid]::NewGuid())\repo",
        "replica.AC_UI_PREFIX_PRIVATE_$([guid]::NewGuid()).contextBadge"
    )
    foreach ($prefix in $prefixes) {
        $listed = Invoke-UiJson -Executable $binaryA -Arguments @("ui-list", "--window", "main", "--prefix", $prefix)
        $allCli.Add($listed) | Out-Null
        Assert-True $listed.Json.ok "Prefix list failed."
        Assert-True ($listed.Json.filters.prefix -ceq $prefix) "Owning prefix echo changed."
        $paths = @(Get-SubstringPaths $listed.Json $prefix)
        Assert-True ($paths.Count -eq 1 -and $paths[0] -ceq '$.filters.prefix') "Prefix leaked outside the owning filters path."
        Assert-True ($listed.Text.EndsWith([Environment]::NewLine)) "List output lost its final line ending."
    }

    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    do {
        $query = Invoke-UiJson -Executable $binaryA -Arguments @("ui-query", "--window", "main", "--selector", "terminal.input", "--timeout-ms", "500") -AllowedExitCodes @(0, 1)
        if ($query.Json.ok) { break }
    } while ([DateTime]::UtcNow -lt $deadline)
    Assert-True $query.Json.ok "terminal.input never became available."
    $allCli.Add($query) | Out-Null

    $pointBefore = [AcUiSmokeNative+POINT]::new()
    Assert-True ([AcUiSmokeNative]::GetCursorPos([ref]$pointBefore)) "GetCursorPos failed."
    $foregroundBefore = [AcUiSmokeNative]::GetForegroundWindow()
    $focus = Invoke-UiJson -Executable $binaryA -Arguments @("ui-focus", "--window", "main", "--selector", "terminal.input")
    $allCli.Add($focus) | Out-Null
    Assert-True ($focus.Json.activeTestId -ceq "terminal.input") "DOM focus was not retained."
    $wait = Invoke-UiJson -Executable $binaryA -Arguments @("ui-wait", "--window", "main", "--selector", "terminal.input", "--focused", "true", "--timeout-ms", "2000")
    $allCli.Add($wait) | Out-Null
    Assert-True (($wait.Json.predicates -join ',') -ceq 'focused') "Wait predicates exposed values or changed order."

    $marker = "AC_UI_TERM_$([guid]::NewGuid())"
    $typed = Invoke-UiJson -Executable $binaryA -Arguments @("ui-type", "--window", "main", "--selector", "terminal.input", "--value", "echo $marker`n")
    $allCli.Add($typed) | Out-Null
    $pointAfter = [AcUiSmokeNative+POINT]::new()
    Assert-True ([AcUiSmokeNative]::GetCursorPos([ref]$pointAfter)) "GetCursorPos failed after UI verbs."
    Assert-True ([AcUiSmokeNative]::GetForegroundWindow() -eq $foregroundBefore) "UI verbs changed the foreground HWND."
    Assert-True ($pointAfter.X -eq $pointBefore.X -and $pointAfter.Y -eq $pointBefore.Y) "UI verbs moved the OS cursor."

    $snapshot = $null
    $deadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    do {
        $candidate = Invoke-UiJson -Executable $binaryA -Arguments @("ui-backend", "--selector", "terminal.snapshot", "--window", "main", "--session", "active", "--timeout-ms", "1000") -AllowedExitCodes @(0, 1)
        if ($candidate.Json.ok -and (@(Get-SubstringPaths $candidate.Json.terminalSnapshot.screen $marker).Count -gt 0)) {
            $snapshot = $candidate
            break
        }
    } while ([DateTime]::UtcNow -lt $deadline)
    Assert-True ($null -ne $snapshot) "Terminal marker was not observed in the semantic snapshot."
    $allCli.Add($snapshot) | Out-Null
    Assert-True ($snapshot.Json.terminalSnapshot.schemaVersion -eq 1) "Terminal snapshot schema changed."
    Assert-True ($snapshot.Json.terminalSnapshot.kind -ceq "ui-terminal-snapshot") "Terminal snapshot kind changed."
    $markerPaths = @(Get-SubstringPaths $snapshot.Json $marker)
    Assert-True ($markerPaths.Count -gt 0) "Terminal marker was absent."
    Assert-True (@($markerPaths | Where-Object { $_ -notmatch '^\$\.terminalSnapshot\.screen\.lines\[\d+\]\.cells\[\d+\]\.text$' }).Count -eq 0) "Terminal marker escaped cell text."

    $badRequestId = [guid]::NewGuid().ToString()
    $requestsDir = Join-Path (Get-ConfigDir $binaryA) "ui-automation\requests"
    [IO.File]::WriteAllText((Join-Path $requestsDir "$badRequestId.json"), "{", [Text.UTF8Encoding]::new($false))
    $logPath = Join-Path (Get-ConfigDir $binaryA) "app.log"
    $logDeadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    $automationLogLines = @()
    do {
        if (Test-Path -LiteralPath $logPath) {
            $automationLogLines = @([IO.File]::ReadAllLines($logPath) | Where-Object {
                $_ -match '^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3} \[(ERROR|WARN|INFO|DEBUG|TRACE)\] agentscommander_lib::ui_automation — \[ui-automation\] '
            })
        }
        if ($automationLogLines.Count -gt 0) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $logDeadline)
    Assert-True ($automationLogLines.Count -gt 0) "No formatter-framed automation log record was observed."

    $forbidden = @(
        $sessionA.Session.token,
        $sessionA.Session.instanceId,
        $sessionA.Session.configDir,
        $sessionA.Session.exePath,
        [string]$sessionA.Session.startedAtUnixMs
    )
    foreach ($secret in $forbidden) {
        foreach ($result in $allCli) {
            Assert-True (-not $result.Text.Contains($secret)) "Automation response leaked an identity canary."
        }
        Assert-True (-not (($automationLogLines -join "`n").Contains($secret))) "Automation log leaked an identity canary."
    }
    foreach ($result in $allCli) {
        if ($result -ne $snapshot) {
            Assert-True (-not $result.Text.Contains($marker)) "Terminal marker leaked into a non-terminal response."
        }
    }

    Stop-UiGui $guiA
    Stop-UiGui $guiB
    $guiA = $null
    $guiB = $null
    Wait-NoAutomationArtifacts $binaryA
    Wait-NoAutomationArtifacts $binaryB

    $requestless = Start-UiGui -Executable $binaryA -Name "requestless-a"
    $aliveDeadline = [DateTime]::UtcNow.AddMilliseconds($TimeoutMs)
    do {
        $requestless.Refresh()
        if ($requestless.HasExited) { throw "Requestless exact artifact exited before collision check." }
        if (Test-Path -LiteralPath (Get-ConfigDir $binaryA) -PathType Container) { break }
        Start-Sleep -Milliseconds 50
    } while ([DateTime]::UtcNow -lt $aliveDeadline)
    Assert-Collision -Executable $binaryA -Arguments @("--ui-automation") -Label "requestless-first/enabled-second"
    Stop-UiGui $requestless
    $requestless = $null
    Wait-NoAutomationArtifacts $binaryA

    [pscustomobject]@{
        status = "passed"
        binary = $BinaryPath
        roots = @($caseA, $caseB)
        cliRecords = $allCli.Count
        automationLogRecords = $automationLogLines.Count
        terminalMarker = $marker
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $LogDir "summary.json") -Encoding UTF8
    Write-Host "PASS: packaged semantic UI automation smoke" -ForegroundColor Green
} finally {
    if ($null -ne $guiA) { Stop-UiGui $guiA }
    if ($null -ne $guiB) { Stop-UiGui $guiB }
    if ($null -ne $requestless) { Stop-UiGui $requestless }
}
