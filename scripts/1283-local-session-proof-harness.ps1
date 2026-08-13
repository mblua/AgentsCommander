# AgentsCommander #1283 Step 8 proof-support harness (Section 22.1.0.a-b).
#
# The only role dispatcher and process launcher for the repository-local same-session
# proof. It imports the implemented PSM1 through its canonical fully-qualified sibling
# path, supplies no caller-selected root, and may place only a byte-for-byte
# hash-recorded copy of this implemented harness in a proof run root. CoordinatorCleanup
# obtains authority only by current-process owner-identity validation, never a
# child-supplied nonce or path argument.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:HarnessCanonicalPath = $null
$script:HarnessSourceSha256 = $null
$script:HarnessSourceBytes = $null
$script:ProofSupportModule = $null

function Get-1283LocalSessionProofSiblingPath {
    param([Parameter(Mandatory)] [string]$FileName)

    $ThisPath = [System.IO.Path]::GetFullPath($PSCommandPath)
    $Sibling = [System.IO.Path]::GetFullPath((Join-Path ([System.IO.Path]::GetDirectoryName($ThisPath)) $FileName))
    $Base = [System.IO.Path]::GetFullPath((Join-Path ([System.IO.Path]::GetDirectoryName($ThisPath)) ''))
    if (-not $Sibling.StartsWith($Base, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'proof-support sibling path escapes the scripts directory'
    }
    return $Sibling
}

function Assert-1283PhysicalReparseChain {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [string]$Stage
    )

    $FullPath = [System.IO.Path]::GetFullPath($Path)
    if (-not [System.IO.File]::Exists($FullPath)) {
        throw "$Stage canonical proof-support file is absent: $FullPath"
    }
    $Item = Get-Item -LiteralPath $FullPath -Force
    if ($Item.LinkType) {
        throw "$Stage proof-support path is a reparse point/link: $FullPath"
    }
    # Walk every parent component and reject any reparse point (junction/symlink).
    $Current = [System.IO.Path]::GetDirectoryName($FullPath)
    while ($Current -and -not [System.IO.Directory]::GetParent($Current) -eq $null) {
        $Directory = Get-Item -LiteralPath $Current -Force -ErrorAction Stop
        if ($Directory.LinkType) {
            throw "$Stage proof-support directory chain contains a reparse point: $Current"
        }
        $Parent = [System.IO.Directory]::GetParent($Current)
        if ($null -eq $Parent) { break }
        $Current = $Parent.FullName
    }
    return $FullPath
}

function Get-1283ByteSha256 {
    param([Parameter(Mandatory)] [byte[]]$Bytes)
    return [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($Bytes))
}

function Initialize-1283LocalSessionProofModule {
    param(
        [Parameter(Mandatory)] [string]$Stage,
        [string]$CanonicalWorkspaceRoot
    )

    if ($null -ne $script:ProofSupportModule) { return $script:ProofSupportModule }
    if (-not [string]::IsNullOrWhiteSpace($CanonicalWorkspaceRoot)) {
        # The proof-run harness copy imports the implemented PSM1 through the recorded
        # fully-qualified canonical path beneath the canonical repository root.
        $Psm1Path = [System.IO.Path]::GetFullPath((Join-Path $CanonicalWorkspaceRoot 'scripts\1283-local-session-proof.psm1'))
    }
    else {
        $Psm1Path = Get-1283LocalSessionProofSiblingPath -FileName '1283-local-session-proof.psm1'
    }
    $CanonicalPsm1 = Assert-1283PhysicalReparseChain -Path $Psm1Path -Stage "$Stage-psm1-path"
    $script:ProofSupportModule = Import-Module -Name $CanonicalPsm1 -PassThru -ErrorAction Stop
    return $script:ProofSupportModule
}

function Get-1283CanonicalHarnessIdentity {
    if ($null -eq $script:HarnessCanonicalPath) {
        $script:HarnessCanonicalPath = Assert-1283PhysicalReparseChain -Path $PSCommandPath -Stage 'harness-self-path'
        $script:HarnessSourceBytes = [System.IO.File]::ReadAllBytes($script:HarnessCanonicalPath)
        $script:HarnessSourceSha256 = Get-1283ByteSha256 -Bytes $script:HarnessSourceBytes
    }
    return [pscustomobject][ordered]@{
        canonical_path = $script:HarnessCanonicalPath
        source_sha256 = $script:HarnessSourceSha256
        byte_length = $script:HarnessSourceBytes.LongLength
    }
}

function Assert-1283CanonicalRootPreflight {
    param(
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$ProofContractSha256,
        [Parameter(Mandatory)] [string]$ProofOwnerPath,
        [Parameter(Mandatory)] [string]$Stage
    )

    $RunRoot = [System.IO.Path]::GetFullPath($ProofRunRoot)
    if (-not [System.IO.Directory]::Exists($RunRoot)) {
        throw "$Stage proof run root is absent: $RunRoot"
    }
    if (-not [System.IO.File]::Exists($ProofContractPath)) {
        throw "$Stage proof contract is absent"
    }
    $ContractBytes = [System.IO.File]::ReadAllBytes($ProofContractPath)
    $ActualSha256 = Get-1283ByteSha256 -Bytes $ContractBytes
    if ($ActualSha256 -cne $ProofContractSha256) {
        throw "$Stage proof contract hash mismatch"
    }
    if (-not [System.IO.File]::Exists($ProofOwnerPath)) {
        throw "$Stage proof owner record is absent"
    }
    $HarnessIdentity = Get-1283CanonicalHarnessIdentity
    $HarnessCopyPath = Join-Path $RunRoot 'harness.ps1'
    if (-not [System.IO.File]::Exists($HarnessCopyPath)) {
        throw "$Stage proof-run harness copy is absent"
    }
    $CopyBytes = [System.IO.File]::ReadAllBytes($HarnessCopyPath)
    if ((Get-1283ByteSha256 -Bytes $CopyBytes) -cne $HarnessIdentity.source_sha256 -or $CopyBytes.LongLength -ne $HarnessIdentity.byte_length) {
        throw "$Stage proof-run harness copy is not byte-for-byte identical to the implemented harness"
    }
    $OwnerBytes = [System.IO.File]::ReadAllBytes($ProofOwnerPath)
    $OwnerJson = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($OwnerBytes)
    $OwnerRecord = $OwnerJson | ConvertFrom-Json -Depth 8
    if ([string]::IsNullOrWhiteSpace([string]$OwnerRecord.proof_id) -or
        [string]::IsNullOrWhiteSpace([string]$OwnerRecord.proof_run_root) -or
        [string]::IsNullOrWhiteSpace([string]$OwnerRecord.owner_name) -or
        [int]$OwnerRecord.coordinator_pid -le 0 -or
        [string]::IsNullOrWhiteSpace([string]$OwnerRecord.coordinator_process_creation_filetime)) {
        throw "$Stage owner record is malformed"
    }
    if ([string]$OwnerRecord.proof_run_root -cne $RunRoot) {
        throw "$Stage owner record run root differs from the supplied run root"
    }
    if (-not [string]::IsNullOrWhiteSpace([string]$OwnerRecord.canonical_workspace_root) -and
        -not [System.IO.Directory]::Exists([string]$OwnerRecord.canonical_workspace_root)) {
        throw "$Stage owner record canonical workspace root is absent"
    }
    $OwnerRecord | Add-Member -NotePropertyName canonical_workspace_root -NotePropertyValue ([string]$OwnerRecord.canonical_workspace_root) -Force
    return [pscustomobject][ordered]@{
        run_root = $RunRoot
        contract_sha256 = $ActualSha256
        owner_name = [string]$OwnerRecord.owner_name
        coordinator_pid = [int]$OwnerRecord.coordinator_pid
        canonical_workspace_root = [string]$OwnerRecord.canonical_workspace_root
    }
}

function Get-1283CanonicalContractScope {
    param(
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$Stage
    )

    $ContractBytes = [System.IO.File]::ReadAllBytes($ProofContractPath)
    $ContractJson = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($ContractBytes)
    $Contract = $ContractJson | ConvertFrom-Json -Depth 8
    $Sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $SidHash = [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData(([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($Sid)))
    $SessionId = [System.Diagnostics.Process]::GetCurrentProcess().SessionId
    if ($SessionId -isnot [int] -or $SessionId -le 0) { throw "$Stage current-process session ID is absent or non-positive" }
    $BindingMaterial = 'local-v2' + [char]0 + [System.Environment]::MachineName.ToUpperInvariant() + [char]0 + $Sid + [char]0 + $SessionId.ToString([System.Globalization.CultureInfo]::InvariantCulture)
    $BindingSha256 = [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData(([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($BindingMaterial)))
    if ([string]$Contract.sid_hash -cne $SidHash -or
        [string]$Contract.machine_name -cne [System.Environment]::MachineName.ToUpperInvariant() -or
        [int]$Contract.interactive_session_id -ne $SessionId -or
        [string]$Contract.interactive_session_binding_sha256 -cne $BindingSha256) {
        throw "$Stage FOREIGN_INTERACTIVE_SESSION_HARD_STOP: contract scope differs from the current session"
    }
    return [pscustomobject][ordered]@{
        sid_hash = $SidHash
        machine_name = [System.Environment]::MachineName.ToUpperInvariant()
        interactive_session_id = $SessionId
        interactive_session_binding_sha256 = $BindingSha256
    }
}

function Write-1283CreateNewRecord {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] $Record,
        [Parameter(Mandatory)] [string]$Stage
    )

    $Directory = [System.IO.Path]::GetDirectoryName($Path)
    if (-not [System.IO.Directory]::Exists($Directory)) {
        [System.IO.Directory]::CreateDirectory($Directory) | Out-Null
    }
    if ([System.IO.File]::Exists($Path)) {
        throw "$Stage create-new record already exists: $Path"
    }
    $Json = $Record | ConvertTo-Json -Depth 8
    $Bytes = ([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($Json)
    $Stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::Read)
    try {
        $Stream.Write($Bytes, 0, $Bytes.Length)
        $Stream.Flush($true)
    }
    finally { $Stream.Dispose() }
    return $Path
}

function Wait-1283CreateNewBarrier {
    param(
        [Parameter(Mandatory)] [string]$Path,
        [Parameter(Mandatory)] [int]$TimeoutSeconds,
        [Parameter(Mandatory)] [string]$Stage
    )

    $Deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $Deadline) {
        if ([System.IO.File]::Exists($Path)) { return }
        Start-Sleep -Milliseconds 100
    }
    throw "$Stage barrier timeout waiting for $Path"
}

function Invoke-1283LocalSessionProofRole {
    param(
        [Parameter(Mandatory)] [string]$Role,
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$ProofContractSha256,
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$ProofOwnerPath
    )

    $Preflight = Assert-1283CanonicalRootPreflight -ProofRunRoot $ProofRunRoot -ProofContractPath $ProofContractPath -ProofContractSha256 $ProofContractSha256 -ProofOwnerPath $ProofOwnerPath -Stage "role-$Role"
    $Module = Initialize-1283LocalSessionProofModule -Stage "role-$Role" -CanonicalWorkspaceRoot $Preflight.canonical_workspace_root
    switch ($Role) {
        'Prepare' { throw 'Prepare is a coordinator-side role and never runs as a child role' }
        'Holder' { Invoke-1283LocalSessionProofHolder -Module $Module -Preflight $Preflight -ProofContractPath $ProofContractPath -ProofRunRoot $ProofRunRoot -ProofOwnerPath $ProofOwnerPath }
        'Contender' { Invoke-1283LocalSessionProofContender -Module $Module -Preflight $Preflight -ProofContractPath $ProofContractPath -ProofRunRoot $ProofRunRoot -ProofOwnerPath $ProofOwnerPath }
        'CoordinatorCleanup' { Invoke-1283LocalSessionProofCoordinatorCleanup -Module $Module -Preflight $Preflight -ProofContractPath $ProofContractPath -ProofRunRoot $ProofRunRoot -ProofOwnerPath $ProofOwnerPath -Holder $null -Contender $null }
        default { throw "unknown proof role: $Role" }
    }
}

function Invoke-1283LocalSessionProofHolder {
    param(
        [Parameter(Mandatory)] $Module,
        [Parameter(Mandatory)] $Preflight,
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$ProofOwnerPath
    )

    $Scope = Get-1283CanonicalContractScope -ProofContractPath $ProofContractPath -Stage 'holder-scope'
    $ResultsDir = Join-Path $ProofRunRoot 'results'
    $BarriersDir = Join-Path $ProofRunRoot 'barriers'
    $HolderIdentityPath = Join-Path $BarriersDir 'holder-identity.json'
    $ContenderIdentityPath = Join-Path $BarriersDir 'contender-identity.json'

    $HolderIdentity = [pscustomobject][ordered]@{
        proof_id = [string]$Preflight.owner_name
        run_root_hash = $Preflight.run_root
        sid_hash = $Scope.sid_hash
        machine_name = $Scope.machine_name
        interactive_session_id = $Scope.interactive_session_id
        interactive_session_binding_sha256 = $Scope.interactive_session_binding_sha256
        process_id = $PID
        utc = [DateTime]::UtcNow.ToString('O')
    }
    Write-1283CreateNewRecord -Path $HolderIdentityPath -Record $HolderIdentity -Stage 'holder-identity'
    Wait-1283CreateNewBarrier -Path $ContenderIdentityPath -TimeoutSeconds 60 -Stage 'holder-wait-contender'

    $ContractBytes = [System.IO.File]::ReadAllBytes($ProofContractPath)
    $Contract = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($ContractBytes) | ConvertFrom-Json -Depth 8
    $CanonicalWorkspaceRoot = [string]$Contract.canonical_workspace_root

    # 1. Holder enters the exact workspace lease and records it.
    $Lease = & $Module 'Enter-WorkspaceMutationLease' -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Purpose '1283-local-proof-holder' -AcquireTimeout ([TimeSpan]::FromSeconds(30))
    $LeaseRecord = & $Module 'Assert-WorkspaceMutationLease' -Lease $Lease -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Stage 'holder-lease-held'
    Write-1283CreateNewRecord -Path (Join-Path $ResultsDir 'holder-lease-held.json') -Record ([pscustomobject][ordered]@{
        lease_name = $LeaseRecord.lease_name
        principal_sid = $LeaseRecord.principal_sid
        machine_name = $LeaseRecord.machine_name
        interactive_session_id = $LeaseRecord.interactive_session_id
        interactive_logon_luid = $LeaseRecord.interactive_logon_luid
        interactive_session_binding_sha256 = $LeaseRecord.interactive_session_binding_sha256
        mutex_creation_state = $LeaseRecord.mutex_creation_state
        utc = [DateTime]::UtcNow.ToString('O')
    }) -Stage 'holder-lease-held'

    # 2. Fixture Job with a live child, protected create-new identity record, and one
    #    strict schema-4 unconfirmed-tree-termination record through the adapter. The
    #    child is started suspended, assigned to the Job BEFORE it can run, and only
    #    then resumed (Section 22.1.0.b fixture-Job proof).
    $FixtureJobName = "Local\AgentsCommander-1283-cbm-$([guid]::NewGuid().ToString('N').ToUpperInvariant())"
    $JobHandle = $null
    $FixtureChildPid = 0
    try {
        $FixtureChildPid = & $Module 'Start-LocalProofFixtureChildSuspended' -Stage 'holder-fixture-child-start'
        $JobHandle = & $Module 'New-LocalProofFixtureJob' -JobName $FixtureJobName -Stage 'holder-fixture-job-create'
        & $Module 'Add-LocalProofFixtureProcessToJob' -JobHandle $JobHandle -ProcessId $FixtureChildPid -Stage 'holder-fixture-child-assign' | Out-Null
        & $Module 'Resume-LocalProofFixtureChild' -ProcessId $FixtureChildPid -Stage 'holder-fixture-child-resume' | Out-Null
        $LiveActiveCount = & $Module 'Get-LocalProofFixtureJobActiveCount' -JobName $FixtureJobName -Stage 'holder-fixture-job-live-count'
        if ($LiveActiveCount -le 0) {
            throw 'holder fixture Job has no live member process'
        }
        $Adapter = & $Module 'New-LocalProofFixtureStateAdapter' -ProofRunRoot $ProofRunRoot -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -ContractSha256 $ProofContractSha256 -WorkspaceLease $Lease -Stage 'holder-adapter'
        & $Module 'Write-LocalProofFixtureHardStop' -Adapter $Adapter -ContractSha256 $ProofContractSha256 -JobName $FixtureJobName -ChildPid ([string]$FixtureChildPid) -Stage 'holder-hard-stop' | Out-Null
        Write-1283CreateNewRecord -Path (Join-Path $ResultsDir 'holder-fixture-job-identity.json') -Record ([pscustomobject][ordered]@{
            job_name = $FixtureJobName
            contract_sha256 = $ProofContractSha256
            child_pid = [string]$FixtureChildPid
            active_process_count = $LiveActiveCount
            state_store_scope = 'local-current-user-interactive-session'
            utc = [DateTime]::UtcNow.ToString('O')
        }) -Stage 'holder-fixture-job-identity'
    }
    catch {
        try { & $Module 'Exit-WorkspaceMutationLease' -Lease $Lease -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Stage 'holder-failure-lease-exit' | Out-Null } catch { }
        try { & $Module 'Close-LocalProofFixtureChildHandles' -Stage 'holder-failure-child-handles' | Out-Null } catch { }
        if ($FixtureChildPid -gt 0) { try { Stop-Process -Id $FixtureChildPid -Force -ErrorAction SilentlyContinue } catch { } }
        throw
    }

    # 3. Holder KEEPS the lease held (fixture Job and child stay live) until Contender
    #    has observed the contention timeout, then releases while the Job stays live.
    Wait-1283CreateNewBarrier -Path (Join-Path $ResultsDir 'contender-contention-observed.json') -TimeoutSeconds 30 -Stage 'holder-wait-contention-observed'
    & $Module 'Exit-WorkspaceMutationLease' -Lease $Lease -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Stage 'holder-state-live-released' | Out-Null
    Write-1283CreateNewRecord -Path (Join-Path $ResultsDir 'holder-state-live-released.json') -Record ([pscustomobject][ordered]@{
        job_name = $FixtureJobName
        child_pid = [string]$FixtureChildPid
        utc = [DateTime]::UtcNow.ToString('O')
    }) -Stage 'holder-state-live-released'

    # 4. Normal positive scenario: Holder is the sole fixture cleanup owner. It waits
    #    for Contender's live-Job observation, then reacquires, terminates its own Job,
    #    waits for zero active processes and child exit, and closes its owned handles.
    Wait-1283CreateNewBarrier -Path (Join-Path $ResultsDir 'contender-live-job-blocked.json') -TimeoutSeconds 30 -Stage 'holder-wait-live-job-blocked'
    $Lease2 = & $Module 'Enter-WorkspaceMutationLease' -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Purpose '1283-local-proof-holder-reacquire' -AcquireTimeout ([TimeSpan]::FromSeconds(30))
    try {
        [AgentsCommander.Review1283.ProofJobInterop]::TerminateJob($FixtureJobName) | Out-Null
        try { Stop-Process -Id $FixtureChildPid -Force -ErrorAction SilentlyContinue } catch { }
        $Deadline = [DateTime]::UtcNow.AddSeconds(15)
        $ActiveAfter = -1
        while ([DateTime]::UtcNow -lt $Deadline) {
            $ActiveAfter = & $Module 'Get-LocalProofFixtureJobActiveCount' -JobName $FixtureJobName -Stage 'holder-job-terminated-count'
            if ($ActiveAfter -le 0 -and -not (Get-Process -Id $FixtureChildPid -ErrorAction SilentlyContinue)) { break }
            Start-Sleep -Milliseconds 100
        }
        & $Module 'Close-LocalProofFixtureChildHandles' -Stage 'holder-job-terminated-handles' | Out-Null
        Write-1283CreateNewRecord -Path (Join-Path $ResultsDir 'holder-job-terminated.json') -Record ([pscustomobject][ordered]@{
            job_name = $FixtureJobName
            child_pid = [string]$FixtureChildPid
            active_process_count_after = $ActiveAfter
            child_exited = (-not (Get-Process -Id $FixtureChildPid -ErrorAction SilentlyContinue))
            utc = [DateTime]::UtcNow.ToString('O')
        }) -Stage 'holder-job-terminated'
    }
    finally {
        try { & $Module 'Exit-WorkspaceMutationLease' -Lease $Lease2 -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Stage 'holder-final-lease-exit' | Out-Null } catch { }
    }
}

function Invoke-1283LocalSessionProofContender {
    param(
        [Parameter(Mandatory)] $Module,
        [Parameter(Mandatory)] $Preflight,
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$ProofOwnerPath
    )

    $Scope = Get-1283CanonicalContractScope -ProofContractPath $ProofContractPath -Stage 'contender-scope'
    $ResultsDir = Join-Path $ProofRunRoot 'results'
    $BarriersDir = Join-Path $ProofRunRoot 'barriers'
    $HolderIdentityPath = Join-Path $BarriersDir 'holder-identity.json'
    $ContenderIdentityPath = Join-Path $BarriersDir 'contender-identity.json'

    $ContenderIdentity = [pscustomobject][ordered]@{
        proof_id = [string]$Preflight.owner_name
        run_root_hash = $Preflight.run_root
        sid_hash = $Scope.sid_hash
        machine_name = $Scope.machine_name
        interactive_session_id = $Scope.interactive_session_id
        interactive_session_binding_sha256 = $Scope.interactive_session_binding_sha256
        process_id = $PID
        utc = [DateTime]::UtcNow.ToString('O')
    }
    Write-1283CreateNewRecord -Path $ContenderIdentityPath -Record $ContenderIdentity -Stage 'contender-identity'
    Wait-1283CreateNewBarrier -Path $HolderIdentityPath -TimeoutSeconds 60 -Stage 'contender-wait-holder'
    Wait-1283CreateNewBarrier -Path (Join-Path $ResultsDir 'holder-lease-held.json') -TimeoutSeconds 30 -Stage 'contender-wait-lease-held'

    $ContractBytes = [System.IO.File]::ReadAllBytes($ProofContractPath)
    $Contract = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($ContractBytes) | ConvertFrom-Json -Depth 8
    $CanonicalWorkspaceRoot = [string]$Contract.canonical_workspace_root

    # 2. Contender contends for the same lease with the contract's five-second timeout.
    $ContentionStart = [DateTime]::UtcNow
    $ContentionError = $null
    try {
        $Lease = & $Module 'Enter-WorkspaceMutationLease' -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Purpose '1283-local-proof-contender' -AcquireTimeout ([TimeSpan]::FromSeconds(5))
        $Acquired = $true
    }
    catch {
        $Acquired = $false
        $ContentionError = $_.Exception.Message
    }
    $ContentionElapsed = ([DateTime]::UtcNow - $ContentionStart).TotalMilliseconds
    if ($Acquired) {
        & $Module 'Exit-WorkspaceMutationLease' -Lease $Lease -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Stage 'contender-unexpected-acquire-exit' | Out-Null
        throw "contender acquired the held lease; expected contention timeout"
    }
    if ($ContentionError -notmatch 'Timed out acquiring workspace lease') {
        throw "contender did not time out cleanly: $ContentionError"
    }
    Write-1283CreateNewRecord -Path (Join-Path $ResultsDir 'contender-contention-observed.json') -Record ([pscustomobject][ordered]@{
        elapsed_ms = [math]::Round($ContentionElapsed, 1)
        timed_out = $true
        utc = [DateTime]::UtcNow.ToString('O')
    }) -Stage 'contender-contention-observed'

    # 3. After holder releases, contender acquires, opens the exact recorded fixture Job
    #    and must observe the live-Job hard block.
    Wait-1283CreateNewBarrier -Path (Join-Path $ResultsDir 'holder-state-live-released.json') -TimeoutSeconds 30 -Stage 'contender-wait-live-released'
    $Lease2 = & $Module 'Enter-WorkspaceMutationLease' -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Purpose '1283-local-proof-contender-live-job' -AcquireTimeout ([TimeSpan]::FromSeconds(30))
    try {
        $FixtureBytes = [System.IO.File]::ReadAllBytes((Join-Path $ResultsDir 'holder-fixture-job-identity.json'))
        $FixtureRecord = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($FixtureBytes) | ConvertFrom-Json -Depth 8
        $Adapter = & $Module 'New-LocalProofFixtureStateAdapter' -ProofRunRoot $ProofRunRoot -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -ContractSha256 $ProofContractSha256 -WorkspaceLease $Lease2 -Stage 'contender-adapter'
        $Clearance = & $Module 'Confirm-LocalProofFixtureHardStopCleared' -Adapter $Adapter -ContractSha256 $ProofContractSha256 -Stage 'contender-live-job-blocked'
        if ($Clearance.state -cne 'live-job-blocked') {
            throw "contender received unexpected clearance state: $($Clearance.state)"
        }
        Write-1283CreateNewRecord -Path (Join-Path $ResultsDir 'contender-live-job-blocked.json') -Record ([pscustomobject][ordered]@{
            job_name = [string]$FixtureRecord.job_name
            state = [string]$Clearance.state
            active_process_count = [int64]$Clearance.active_process_count
            utc = [DateTime]::UtcNow.ToString('O')
        }) -Stage 'contender-live-job-blocked'
    }
    finally {
        & $Module 'Exit-WorkspaceMutationLease' -Lease $Lease2 -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Stage 'contender-live-job-lease-exit' | Out-Null
    }

    # 5. After holder terminates its Job, contender reacquires and observes the cleared
    #    transition through the adapter.
    Wait-1283CreateNewBarrier -Path (Join-Path $ResultsDir 'holder-job-terminated.json') -TimeoutSeconds 30 -Stage 'contender-wait-job-terminated'
    $Lease3 = & $Module 'Enter-WorkspaceMutationLease' -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Purpose '1283-local-proof-contender-clearance' -AcquireTimeout ([TimeSpan]::FromSeconds(30))
    try {
        $Adapter3 = & $Module 'New-LocalProofFixtureStateAdapter' -ProofRunRoot $ProofRunRoot -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -ContractSha256 $ProofContractSha256 -WorkspaceLease $Lease3 -Stage 'contender-clearance-adapter'
        $Cleared = & $Module 'Confirm-LocalProofFixtureHardStopCleared' -Adapter $Adapter3 -ContractSha256 $ProofContractSha256 -Stage 'contender-clearance'
        if ($Cleared.state -cne 'cleared') {
            throw "contender did not observe the cleared transition: $($Cleared.state)"
        }
        Write-1283CreateNewRecord -Path (Join-Path $ResultsDir 'contender-clearance-observed.json') -Record ([pscustomobject][ordered]@{
            state = [string]$Cleared.state
            job_name = [string]$Cleared.job_name
            clearance_session_id = [int]$Cleared.clearance_session_id
            utc = [DateTime]::UtcNow.ToString('O')
        }) -Stage 'contender-clearance-observed'
    }
    finally {
        & $Module 'Exit-WorkspaceMutationLease' -Lease $Lease3 -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot -Stage 'contender-clearance-lease-exit' | Out-Null
    }
}

function Invoke-1283LocalSessionProofCoordinatorCleanup {
    param(
        [Parameter(Mandatory)] $Module,
        [Parameter(Mandatory)] $Preflight,
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$ProofOwnerPath,
        [AllowNull()] $Holder,
        [AllowNull()] $Contender
    )

    $ResultsDir = Join-Path $ProofRunRoot 'results'
    # Owner-only authority: current-process identity must match the protected owner
    # record before any cleanup enumeration, fixture-state access, Job open, process
    # control, root mutation, or peer observation - including the ordinary path.
    & $Module 'Assert-LocalProofCoordinatorOwnerIdentity' -ProofOwnerPath $ProofOwnerPath -Stage 'coordinator-cleanup-owner-identity' | Out-Null
    $FailureRecordPath = Join-Path $ResultsDir 'holder-failure-fixture-job-identity.json'
    $FailureCleanupRecord = $null
    if ([System.IO.File]::Exists($FailureRecordPath)) {
        $FailureCleanupRecord = & $Module 'Invoke-LocalProofCoordinatorCleanup' -ProofRunRoot $ProofRunRoot -ProofOwnerPath $ProofOwnerPath -ProofContractPath $ProofContractPath -ProofContractSha256 ([string]$Preflight.contract_sha256) -FailureRecordPath $FailureRecordPath -CleanupTimeoutSeconds 15 -Stage 'coordinator-injected-failure-cleanup'
    }
    else {
        if ([System.IO.Directory]::Exists($ProofRunRoot)) {
            [System.IO.Directory]::Delete($ProofRunRoot, $true)
        }
    }
    if ([System.IO.Directory]::Exists($ProofRunRoot)) {
        throw 'coordinator cleanup did not remove the exact proof run root'
    }
    return [pscustomobject][ordered]@{
        state = 'cleanup-complete'
        run_root_removed = $true
        failure_cleanup_recorded = ($null -ne $FailureCleanupRecord)
        failure_cleanup_state = if ($null -ne $FailureCleanupRecord) { [string]$FailureCleanupRecord.state } else { $null }
        failure_cleanup_job_name = if ($null -ne $FailureCleanupRecord -and -not [string]::IsNullOrWhiteSpace([string]$FailureCleanupRecord.job_name)) { [string]$FailureCleanupRecord.job_name } else { $null }
        utc = [DateTime]::UtcNow.ToString('O')
    }
}

function Get-1283ProtectedCurrentUserAcl {
    param([Parameter(Mandatory)] [string]$Stage)

    $Identity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
    if ($null -eq $Identity -or $null -eq $Identity.User -or [string]::IsNullOrWhiteSpace($Identity.User.Value)) {
        throw "$Stage cannot establish the current-user SID for a protected ACL"
    }
    $Sid = [System.Security.Principal.SecurityIdentifier]::new($Identity.User.Value)
    $Acl = [System.Security.AccessControl.DirectorySecurity]::new()
    $Acl.SetOwner($Sid)
    $Acl.SetAccessRuleProtection($true, $false)
    [void]$Acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new(
        $Sid,
        [System.Security.AccessControl.FileSystemRights]::FullControl,
        [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
            [System.Security.AccessControl.InheritanceFlags]::ObjectInherit,
        [System.Security.AccessControl.PropagationFlags]::None,
        [System.Security.AccessControl.AccessControlType]::Allow
    ))
    return $Acl
}

function Invoke-1283LocalSessionProofPrepare {
    param(
        [Parameter(Mandatory)] [string]$CanonicalWorkspaceRoot,
        [Parameter(Mandatory)] [string]$DraftPlanSha256,
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$ProofScratchOwner,
        [Parameter(Mandatory)] [string]$ProofId
    )

    $Module = Initialize-1283LocalSessionProofModule -Stage 'prepare' -CanonicalWorkspaceRoot $CanonicalWorkspaceRoot
    if ([System.IO.Directory]::Exists($ProofRunRoot)) {
        throw 'proof scratch run root already exists and cannot be adopted'
    }
    [System.IO.Directory]::CreateDirectory($ProofRunRoot) | Out-Null
    $Acl = Get-1283ProtectedCurrentUserAcl -Stage 'prepare-run-root-acl'
    Set-Acl -LiteralPath $ProofRunRoot -AclObject $Acl
    foreach ($Child in @('barriers', 'results', 'fixture-state')) {
        $Path = Join-Path $ProofRunRoot $Child
        [System.IO.Directory]::CreateDirectory($Path) | Out-Null
        Set-Acl -LiteralPath $Path -AclObject $Acl
    }

    $HarnessIdentity = Get-1283CanonicalHarnessIdentity
    $HarnessCopyPath = Join-Path $ProofRunRoot 'harness.ps1'
    $CopyStream = [System.IO.File]::Open($HarnessCopyPath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::Read)
    try {
        $CopyStream.Write($script:HarnessSourceBytes, 0, $script:HarnessSourceBytes.Length)
        $CopyStream.Flush($true)
    }
    finally { $CopyStream.Dispose() }

    $Sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $SidHash = [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData(([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($Sid)))
    $SessionId = [System.Diagnostics.Process]::GetCurrentProcess().SessionId
    $BindingMaterial = 'local-v2' + [char]0 + [System.Environment]::MachineName.ToUpperInvariant() + [char]0 + $Sid + [char]0 + $SessionId.ToString([System.Globalization.CultureInfo]::InvariantCulture)
    $BindingSha256 = [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData(([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($BindingMaterial)))

    $Psm1Path = Get-1283LocalSessionProofSiblingPath -FileName '1283-local-session-proof.psm1'
    $Psm1Bytes = [System.IO.File]::ReadAllBytes((Assert-1283PhysicalReparseChain -Path $Psm1Path -Stage 'prepare-psm1'))
    $Psm1Sha256 = Get-1283ByteSha256 -Bytes $Psm1Bytes

    $OwnerRecord = [pscustomobject][ordered]@{
        proof_id = $ProofId
        proof_run_root = $ProofRunRoot
        owner_name = $ProofScratchOwner
        canonical_workspace_root = $CanonicalWorkspaceRoot
        coordinator_pid = $PID
        coordinator_process_creation_filetime = (Get-1283ProcessCreationFileTime -ProcessId $PID -Stage 'prepare-owner-creation')
        sid_hash = $SidHash
        machine_name = [System.Environment]::MachineName.ToUpperInvariant()
        interactive_session_id = $SessionId
        interactive_session_binding_sha256 = $BindingSha256
    }
    Write-1283CreateNewRecord -Path (Join-Path $ProofRunRoot 'owner.json') -Record $OwnerRecord -Stage 'prepare-owner'

    $ContractRecord = [pscustomobject][ordered]@{
        canonical_workspace_root = $CanonicalWorkspaceRoot
        canonical_plan_path = (Join-Path $CanonicalWorkspaceRoot 'plans/1283-prevent-terminal-renderer-saturation.md')
        draft_plan_sha256 = $DraftPlanSha256
        proof_id = $ProofId
        proof_run_root = $ProofRunRoot
        owner_name = $ProofScratchOwner
        coordinator_pid = $PID
        coordinator_process_creation_filetime = [string]$OwnerRecord.coordinator_process_creation_filetime
        sid_hash = $SidHash
        machine_name = [System.Environment]::MachineName.ToUpperInvariant()
        interactive_session_id = $SessionId
        interactive_session_binding_sha256 = $BindingSha256
        expected_state_store_scope = 'local-current-user-interactive-session'
        psm1_sha256 = $Psm1Sha256
        harness_source_sha256 = $HarnessIdentity.source_sha256
        proof_run_harness_sha256 = $HarnessIdentity.source_sha256
        identity_barrier_wait_seconds = 60
        expected_contention_wait_seconds = 5
        post_release_acquire_wait_seconds = 30
        live_job_observation_wait_seconds = 30
        owner_cleanup_wait_seconds = 15
        total_role_wait_seconds = 180
    }
    Write-1283CreateNewRecord -Path (Join-Path $ProofRunRoot 'contract.json') -Record $ContractRecord -Stage 'prepare-contract'
    return [pscustomobject][ordered]@{
        proof_id = $ProofId
        proof_run_root = $ProofRunRoot
        contract_sha256 = (Get-1283ByteSha256 -Bytes ([System.IO.File]::ReadAllBytes((Join-Path $ProofRunRoot 'contract.json'))))
    }
}

function Get-1283ProcessCreationFileTime {
    param(
        [Parameter(Mandatory)] [int]$ProcessId,
        [Parameter(Mandatory)] [string]$Stage
    )
    if (-not ('AgentsCommander.Review1283.ProofProcessInterop' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace AgentsCommander.Review1283 {
  public static class ProofProcessInterop {
    [StructLayout(LayoutKind.Sequential)] public struct ProofFileTime { public uint Low; public uint High; }
    [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool GetProcessTimes(IntPtr h, out ProofFileTime creation, out ProofFileTime exit, out ProofFileTime kernel, out ProofFileTime user);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
    public static string GetCreationFileTime(int pid) {
      IntPtr h = OpenProcess(0x0400 | 0x0010, false, pid);
      if (h == IntPtr.Zero) throw new InvalidOperationException("OpenProcess failed: " + Marshal.GetLastWin32Error());
      try {
        ProofFileTime creation; ProofFileTime exit; ProofFileTime kernel; ProofFileTime user;
        if (!GetProcessTimes(h, out creation, out exit, out kernel, out user)) throw new InvalidOperationException("GetProcessTimes failed: " + Marshal.GetLastWin32Error());
        return ((ulong)creation.High << 32 | (ulong)creation.Low).ToString("X16");
      } finally { CloseHandle(h); }
    }
  }
}
'@ -ErrorAction Stop | Out-Null
    }
    $Type = 'AgentsCommander.Review1283.ProofProcessInterop' -as [type]
    if ($null -eq $Type) { throw "$Stage cannot load the proof process interop" }
    return $Type::GetCreationFileTime($ProcessId)
}

function Invoke-1283LocalSessionProofHarness {
    param(
        [Parameter(Mandatory)] [string]$Role,
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$ProofContractSha256,
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$ProofOwnerPath
    )

    switch ($Role) {
        'Prepare' {
            throw 'Prepare is invoked by the coordinator-side Prepare dispatcher with the canonical workspace identity; it never runs as a child role'
        }
        'Holder' { Invoke-1283LocalSessionProofRole -Role Holder -ProofContractPath $ProofContractPath -ProofContractSha256 $ProofContractSha256 -ProofRunRoot $ProofRunRoot -ProofOwnerPath $ProofOwnerPath }
        'Contender' { Invoke-1283LocalSessionProofRole -Role Contender -ProofContractPath $ProofContractPath -ProofContractSha256 $ProofContractSha256 -ProofRunRoot $ProofRunRoot -ProofOwnerPath $ProofOwnerPath }
        'CoordinatorCleanup' { Invoke-1283LocalSessionProofRole -Role CoordinatorCleanup -ProofContractPath $ProofContractPath -ProofContractSha256 $ProofContractSha256 -ProofRunRoot $ProofRunRoot -ProofOwnerPath $ProofOwnerPath }
        default { throw "unknown proof role: $Role" }
    }
}

# ---------------------------------------------------------------------------
# CLI entry point: -Role Prepare|Holder|Contender|CoordinatorCleanup.
# ---------------------------------------------------------------------------
$ParamRole = $null
$ParamProofContractPath = $null
$ParamProofContractSha256 = $null
$ParamProofRunRoot = $null
$ParamProofOwnerPath = $null
$ParamCanonicalWorkspaceRoot = $null
$ParamDraftPlanSha256 = $null
$ParamProofId = $null
$ParamProofScratchOwner = $null
for ($Index = 0; $Index -lt $args.Count; $Index++) {
    switch -Regex ($args[$Index]) {
        '^-Role$' { $ParamRole = $args[++$Index] }
        '^-ProofContractPath$' { $ParamProofContractPath = $args[++$Index] }
        '^-ProofContractSha256$' { $ParamProofContractSha256 = $args[++$Index] }
        '^-ProofRunRoot$' { $ParamProofRunRoot = $args[++$Index] }
        '^-ProofOwnerPath$' { $ParamProofOwnerPath = $args[++$Index] }
        '^-CanonicalWorkspaceRoot$' { $ParamCanonicalWorkspaceRoot = $args[++$Index] }
        '^-DraftPlanSha256$' { $ParamDraftPlanSha256 = $args[++$Index] }
        '^-ProofId$' { $ParamProofId = $args[++$Index] }
        '^-ProofScratchOwner$' { $ParamProofScratchOwner = $args[++$Index] }
        default { throw "unexpected harness argument: $($args[$Index])" }
    }
}
if ($ParamRole -eq 'Prepare') {
    Invoke-1283LocalSessionProofPrepare -CanonicalWorkspaceRoot $ParamCanonicalWorkspaceRoot -DraftPlanSha256 $ParamDraftPlanSha256 -ProofRunRoot $ParamProofRunRoot -ProofScratchOwner $ParamProofScratchOwner -ProofId $ParamProofId
}
elseif ($ParamRole -in @('Holder', 'Contender', 'CoordinatorCleanup')) {
    Invoke-1283LocalSessionProofHarness -Role $ParamRole -ProofContractPath $ParamProofContractPath -ProofContractSha256 $ParamProofContractSha256 -ProofRunRoot $ParamProofRunRoot -ProofOwnerPath $ParamProofOwnerPath
}
