Set-StrictMode -Version Latest

function ConvertTo-WindowsNativeArgument {
    [CmdletBinding()]
    [OutputType([string])]
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyString()]
        [string]$Value
    )

    if ($Value.Length -eq 0) {
        return '""'
    }

    if ($Value -notmatch '[\s"]') {
        return $Value
    }

    $serialized = [System.Text.StringBuilder]::new()
    [void]$serialized.Append('"')
    $backslashCount = 0

    foreach ($character in $Value.ToCharArray()) {
        if ($character -eq '\') {
            $backslashCount++
            continue
        }

        if ($character -eq '"') {
            if ($backslashCount -gt 0) {
                [void]$serialized.Append(('\' * ($backslashCount * 2)))
            }
            [void]$serialized.Append('\')
            [void]$serialized.Append('"')
            $backslashCount = 0
            continue
        }

        if ($backslashCount -gt 0) {
            [void]$serialized.Append(('\' * $backslashCount))
            $backslashCount = 0
        }
        [void]$serialized.Append($character)
    }

    if ($backslashCount -gt 0) {
        [void]$serialized.Append(('\' * ($backslashCount * 2)))
    }
    [void]$serialized.Append('"')
    return $serialized.ToString()
}

function Test-IsolatedValidationFullyQualifiedPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if ($Path.IndexOf([char]0) -ge 0) {
        return $false
    }

    $isPathFullyQualified = [System.IO.Path].GetMethod(
        'IsPathFullyQualified',
        [System.Type[]]@([string])
    )
    if ($null -ne $isPathFullyQualified) {
        return [System.IO.Path]::IsPathFullyQualified($Path)
    }

    return $Path -match '^(?:[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+(?:[\\/]|$)|\\\\\?\\(?:[A-Za-z]:[\\/]|UNC\\[^\\/]+[\\/][^\\/]+(?:[\\/]|$)))'
}

function Test-IsolatedValidationAbsoluteFile {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-IsolatedValidationFullyQualifiedPath -Path $Path)) {
        return $false
    }

    return [System.IO.File]::Exists($Path)
}

function Test-IsolatedValidationAbsoluteDirectory {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if (-not (Test-IsolatedValidationFullyQualifiedPath -Path $Path)) {
        return $false
    }

    return [System.IO.Directory]::Exists($Path)
}

function Remove-IsolatedValidationChildEnvironment {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.ProcessStartInfo]$StartInfo
    )

    $keys = @($StartInfo.EnvironmentVariables.Keys)
    foreach ($key in $keys) {
        if ([string]$key -match '(?i)^AGENTSCOMMANDER_') {
            [void]$StartInfo.EnvironmentVariables.Remove([string]$key)
        }
    }
}

function Stop-IsolatedValidationProcess {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$Process,
        [ValidateRange(1, [int]::MaxValue)]
        [int]$TimeoutMilliseconds = 3000
    )

    try {
        if (-not $Process.HasExited) {
            $Process.Kill()
        }
        if (-not $Process.WaitForExit($TimeoutMilliseconds)) {
            Write-Verbose '[isolated-validation] owned child cleanup timed out'
            throw 'owned child cleanup timed out'
        }
        if (-not $Process.HasExited) {
            throw 'owned child did not exit after termination'
        }
    }
    catch {
        Write-Verbose "[isolated-validation] owned child cleanup failed: $($_.Exception.Message)"
        throw 'E_ISOLATION_NATIVE_PROCESS'
    }
}

function Read-IsolatedValidationBoundedStreams {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)]
        [ValidateRange(1, [int]::MaxValue)]
        [int]$StandardOutputLimitBytes,
        [Parameter(Mandatory = $true)]
        [ValidateRange(1, [int]::MaxValue)]
        [int]$StandardErrorLimitBytes
    )

    $stdoutReader = $Process.StandardOutput
    $stderrReader = $Process.StandardError
    $stdoutStream = $stdoutReader.BaseStream
    $stderrStream = $stderrReader.BaseStream
    $stdoutBuffer = New-Object byte[] 8192
    $stderrBuffer = New-Object byte[] 8192
    $stdoutBytes = [System.IO.MemoryStream]::new()
    $stderrBytes = [System.IO.MemoryStream]::new()
    $stdoutDone = $false
    $stderrDone = $false
    $stdoutTask = $stdoutStream.ReadAsync($stdoutBuffer, 0, $stdoutBuffer.Length)
    $stderrTask = $stderrStream.ReadAsync($stderrBuffer, 0, $stderrBuffer.Length)
    $captureExceeded = $false

    try {
        while (-not ($stdoutDone -and $stderrDone)) {
            $pending = [System.Collections.Generic.List[System.Threading.Tasks.Task]]::new()
            if (-not $stdoutDone) {
                [void]$pending.Add($stdoutTask)
            }
            if (-not $stderrDone) {
                [void]$pending.Add($stderrTask)
            }

            $completed = [System.Threading.Tasks.Task]::WhenAny($pending.ToArray()).GetAwaiter().GetResult()
            if ($completed -eq $stdoutTask) {
                $count = $stdoutTask.GetAwaiter().GetResult()
                if ($count -eq 0) {
                    $stdoutDone = $true
                }
                else {
                    if (($stdoutBytes.Length + $count) -gt $StandardOutputLimitBytes) {
                        $captureExceeded = $true
                        break
                    }
                    $stdoutBytes.Write($stdoutBuffer, 0, $count)
                    $stdoutTask = $stdoutStream.ReadAsync($stdoutBuffer, 0, $stdoutBuffer.Length)
                }
            }
            elseif ($completed -eq $stderrTask) {
                $count = $stderrTask.GetAwaiter().GetResult()
                if ($count -eq 0) {
                    $stderrDone = $true
                }
                else {
                    if (($stderrBytes.Length + $count) -gt $StandardErrorLimitBytes) {
                        $captureExceeded = $true
                        break
                    }
                    $stderrBytes.Write($stderrBuffer, 0, $count)
                    $stderrTask = $stderrStream.ReadAsync($stderrBuffer, 0, $stderrBuffer.Length)
                }
            }
        }

        if ($captureExceeded) {
            Stop-IsolatedValidationProcess -Process $Process
            throw 'E_ISOLATION_NATIVE_PROCESS'
        }

        $Process.WaitForExit()
        return [pscustomobject]@{
            StandardOutput = [System.Text.Encoding]::UTF8.GetString($stdoutBytes.ToArray())
            StandardError  = [System.Text.Encoding]::UTF8.GetString($stderrBytes.ToArray())
        }
    }
    finally {
        $cleanupFailed = $false
        $cleanupTimedOut = $false
        $cleanupDeadline = [DateTime]::UtcNow.AddMilliseconds(3000)

        if (-not $Process.HasExited) {
            try {
                Stop-IsolatedValidationProcess -Process $Process -TimeoutMilliseconds 3000
            }
            catch {
                $cleanupFailed = $true
                Write-Verbose '[isolated-validation] owned child termination during capture cleanup failed'
            }
        }

        foreach ($reader in @($stdoutReader, $stderrReader)) {
            try {
                $reader.Dispose()
            }
            catch {
                $cleanupFailed = $true
                Write-Verbose '[isolated-validation] stream reader disposal failed'
            }
        }

        foreach ($readTask in @($stdoutTask, $stderrTask)) {
            $remainingMilliseconds = [int][Math]::Max(
                0,
                [Math]::Floor(($cleanupDeadline - [DateTime]::UtcNow).TotalMilliseconds)
            )
            if (-not $readTask.IsCompleted -and
                ($remainingMilliseconds -le 0 -or -not $readTask.Wait($remainingMilliseconds))) {
                $cleanupTimedOut = $true
                continue
            }

            if ($readTask.IsFaulted) {
                [void]$readTask.Exception
            }
        }

        foreach ($buffer in @($stdoutBytes, $stderrBytes)) {
            try {
                $buffer.Dispose()
            }
            catch {
                $cleanupFailed = $true
                Write-Verbose "[isolated-validation] stream buffer disposal failed: $($_.Exception.Message)"
            }
        }

        if ($cleanupTimedOut) {
            $cleanupFailed = $true
            Write-Verbose '[isolated-validation] native capture cleanup timed out'
        }

        if ($cleanupFailed) {
            throw 'E_ISOLATION_NATIVE_PROCESS'
        }
    }
}

function Start-IsolatedValidationNativeProcess {
    [CmdletBinding()]
    [OutputType([pscustomobject])]
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet('CaptureAndWait', 'Wait', 'Start')]
        [string]$Mode,
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(Mandatory = $true)]
        [string]$WorkingDirectory,
        [Parameter()]
        [AllowEmptyCollection()]
        [AllowEmptyString()]
        [string[]]$Arguments = @(),
        [Parameter()]
        [ValidateRange(1, [int]::MaxValue)]
        [int]$StandardOutputLimitBytes,
        [Parameter()]
        [ValidateRange(1, [int]::MaxValue)]
        [int]$StandardErrorLimitBytes,
        [Parameter(Mandatory = $true)]
        [switch]$RemoveAgentsCommanderEnvironment
    )

    $process = $null
    $transferLease = $false

    try {
        if (-not (Test-IsolatedValidationAbsoluteFile -Path $FilePath) -or
            -not (Test-IsolatedValidationAbsoluteDirectory -Path $WorkingDirectory)) {
            throw 'E_ISOLATION_NATIVE_PROCESS'
        }

        if ($Mode -eq 'CaptureAndWait' -and
            ($StandardOutputLimitBytes -lt 1 -or $StandardErrorLimitBytes -lt 1)) {
            throw 'E_ISOLATION_NATIVE_PROCESS'
        }

        $serializedArguments = [System.Collections.Generic.List[string]]::new()
        foreach ($argument in $Arguments) {
            if ($null -eq $argument -or $argument.IndexOf([char]0) -ge 0) {
                throw 'E_ISOLATION_NATIVE_PROCESS'
            }
            [void]$serializedArguments.Add((ConvertTo-WindowsNativeArgument -Value $argument))
        }

        $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
        $startInfo.FileName = $FilePath
        $startInfo.WorkingDirectory = $WorkingDirectory
        $startInfo.UseShellExecute = $false
        $startInfo.Arguments = [string]::Join(' ', $serializedArguments)
        $startInfo.RedirectStandardOutput = $Mode -eq 'CaptureAndWait'
        $startInfo.RedirectStandardError = $Mode -eq 'CaptureAndWait'

        if ($RemoveAgentsCommanderEnvironment) {
            Remove-IsolatedValidationChildEnvironment -StartInfo $startInfo
        }

        $process = [System.Diagnostics.Process]::new()
        $process.StartInfo = $startInfo
        if (-not $process.Start()) {
            throw 'E_ISOLATION_NATIVE_PROCESS'
        }

        if ($Mode -eq 'Start') {
            $transferLease = $true
            return [pscustomobject]@{
                ProcessId = $process.Id
                Process   = $process
            }
        }

        if ($Mode -eq 'Wait') {
            $process.WaitForExit()
            return [pscustomobject]@{
                ProcessId = $process.Id
                ExitCode  = $process.ExitCode
            }
        }

        $captured = Read-IsolatedValidationBoundedStreams -Process $process `
            -StandardOutputLimitBytes $StandardOutputLimitBytes `
            -StandardErrorLimitBytes $StandardErrorLimitBytes
        return [pscustomobject]@{
            ProcessId      = $process.Id
            ExitCode       = $process.ExitCode
            StandardOutput = $captured.StandardOutput
            StandardError  = $captured.StandardError
        }
    }
    catch {
        if ($null -ne $process) {
            Stop-IsolatedValidationProcess -Process $process
        }
        throw 'E_ISOLATION_NATIVE_PROCESS'
    }
    finally {
        if ($null -ne $process -and -not $transferLease) {
            $process.Dispose()
        }
    }
}

Export-ModuleMember -Function Start-IsolatedValidationNativeProcess
