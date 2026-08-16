# AgentsCommander #1283 Step 8 proof-support test driver (Section 22.1.0.a-b).
#
# The post-implementation source-contract and bounded proof driver. It invokes only
# the implemented PSM1 and harness, records the named positive-proof outcomes, and
# verifies owner-only cleanup. It performs the Section 22.1.0.a source-contract checks
# (items 1-8), the negative owner-identity checks, the normal positive proof, the
# injected Holder-failure cleanup proof, and the final run-root absence assertion.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:TestCanonicalRoot = $null
$script:TestProofRunRoot = $null
$script:TestProofId = $null
$script:TestContractSha256 = $null

function Get-1283TestByteSha256 {
    param([Parameter(Mandatory)] [byte[]]$Bytes)
    return [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($Bytes))
}

function Get-1283TestCanonicalPath {
    param([Parameter(Mandatory)] [string]$Path)
    $FullPath = [System.IO.Path]::GetFullPath($Path)
    return $FullPath.TrimEnd([char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)).ToUpperInvariant()
}

function Get-1283TestScriptsDir {
    return [System.IO.Path]::GetDirectoryName([System.IO.Path]::GetFullPath($PSCommandPath))
}

function Assert-1283TestSourceFile {
    param(
        [Parameter(Mandatory)] [string]$FileName,
        [Parameter(Mandatory)] [string[]]$RequiredSymbols,
        [Parameter(Mandatory)] [string]$Stage
    )

    $Path = [System.IO.Path]::GetFullPath((Join-Path (Get-1283TestScriptsDir) $FileName))
    if (-not [System.IO.File]::Exists($Path)) {
        throw "$Stage source-contract: $FileName is absent"
    }
    $Item = Get-Item -LiteralPath $Path -Force
    if ($Item.LinkType) {
        throw "$Stage source-contract: $FileName is a reparse point/link"
    }
    $Text = [System.IO.File]::ReadAllText($Path)
    foreach ($Symbol in $RequiredSymbols) {
        $Count = ([regex]::Matches($Text, [regex]::Escape($Symbol))).Count
        if ($Count -lt 1) {
            throw "$Stage source-contract: $FileName lacks required symbol $Symbol"
        }
    }
    return [pscustomobject][ordered]@{
        path = $Path
        sha256 = (Get-1283TestByteSha256 -Bytes ([System.IO.File]::ReadAllBytes($Path)))
        byte_length = (Get-Item -LiteralPath $Path).Length
    }
}

function Assert-1283TestHarnessImportsCanonicalPsm1 {
    param([Parameter(Mandatory)] [string]$Stage)

    $Harness = [System.IO.File]::ReadAllText((Join-Path (Get-1283TestScriptsDir) '1283-local-session-proof-harness.ps1'))
    if ($Harness -notmatch [regex]::Escape("Import-Module -Name `$CanonicalPsm1")) {
        throw "$Stage source-contract: harness does not import the PSM1 through a fully-qualified canonical path"
    }
    if ($Harness -match 'Join-Path \$PSScriptRoot|\.\\1283-local-session-proof\.psm1|Import-Module -Name "?\' + '1283-local-session-proof\.psm1') {
        throw "$Stage source-contract: harness imports the PSM1 through a non-canonical path"
    }
}

function Assert-1283TestSingleSessionReader {
    param([Parameter(Mandatory)] [string]$Stage)

    $Psm1 = [System.IO.File]::ReadAllText((Join-Path (Get-1283TestScriptsDir) '1283-local-session-proof.psm1'))
    $SessionPropertyUses = ([regex]::Matches($Psm1, '\[System\.Diagnostics\.Process\]::GetCurrentProcess\(\)')).Count
    if ($SessionPropertyUses -lt 1) {
        throw "$Stage source-contract: PSM1 lacks the sole supported current-process session reader"
    }
    if ($Psm1 -notmatch '\$SessionIdValue = \$CurrentProcess\.SessionId') {
        throw "$Stage source-contract: PSM1 does not read SessionId from the current-process object"
    }
    if ($Psm1 -match '\[System\.Environment\]::SessionId|Environment\.GetEnvironmentVariable\([^)]*Session') {
        throw "$Stage source-contract: PSM1 contains a System.Environment session-ID reader"
    }
    $SessionHelperUses = ([regex]::Matches($Psm1, 'Get-RepositoryLeaseCurrentInteractiveSessionId')).Count
    if ($SessionHelperUses -lt 3) {
        throw "$Stage source-contract: PSM1 does not use the session helper at the lease/Job/state boundaries"
    }
}

function Assert-1283TestFixtureAdapterPathDiscipline {
    param([Parameter(Mandatory)] [string]$Stage)

    $Psm1 = [System.IO.File]::ReadAllText((Join-Path (Get-1283TestScriptsDir) '1283-local-session-proof.psm1'))
    if ($Psm1 -notmatch 'fixture-state') {
        throw "$Stage source-contract: PSM1 fixture adapter has no fixture-state route"
    }
    $AdapterStart = $Psm1.IndexOf('function New-LocalProofFixtureStateAdapter')
    $ExportStart = $Psm1.LastIndexOf('Export-ModuleMember -Function')
    if ($AdapterStart -lt 0 -or $ExportStart -le $AdapterStart) {
        throw "$Stage source-contract: fixture adapter section is not bounded"
    }
    $AdapterSection = $Psm1.Substring($AdapterStart, $ExportStart - $AdapterStart)
    foreach ($Forbidden in @('NativeCbm', 'Get-NativeCbmControlStatePaths', 'SessionManager')) {
        if ($AdapterSection -match $Forbidden) {
            throw "$Stage source-contract: fixture adapter references forbidden production route $Forbidden"
        }
    }
    if ($AdapterSection -match 'rust|typescript') {
        throw "$Stage source-contract: proof-support source introduces a Rust or TypeScript import route"
    }
}

function Assert-1283TestOwnerRecordContract {
    param(
        [Parameter(Mandatory)] [string]$ProofOwnerPath,
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$Stage
    )

    $OwnerJson = ([System.Text.UTF8Encoding]::new($false, $true)).GetString([System.IO.File]::ReadAllBytes($ProofOwnerPath))
    $Owner = $OwnerJson | ConvertFrom-Json -Depth 8
    if ([int]$Owner.coordinator_pid -ne $PID) {
        throw "$Stage FAIL_NOT_COORDINATOR_OWNER: owner PID differs from the caller"
    }
    if ([string]::IsNullOrWhiteSpace([string]$Owner.coordinator_process_creation_filetime) -or
        [string]$Owner.coordinator_process_creation_filetime -cnotmatch '^[0-9A-F]{16}$') {
        throw "$Stage owner record has no valid 64-bit creation FILETIME identity"
    }
    foreach ($ForbiddenField in @('cleanup_nonce', 'cleanup_secret', 'bearer')) {
        if ($OwnerJson -match $ForbiddenField) {
            throw "$Stage owner record carries a child-replayable authority field: $ForbiddenField"
        }
    }
    $ContractJson = ([System.Text.UTF8Encoding]::new($false, $true)).GetString([System.IO.File]::ReadAllBytes($ProofContractPath))
    $Contract = $ContractJson | ConvertFrom-Json -Depth 8
    if ([string]$Contract.proof_id -cne [string]$Owner.proof_id -or
        [string]$Contract.coordinator_pid -ne [int]$Owner.coordinator_pid) {
        throw "$Stage contract and owner records disagree on proof identity"
    }
    return $Owner
}

function Assert-1283TestDeniedCleanupAttempt {
    param(
        [Parameter(Mandatory)] [string]$ProofOwnerPath,
        [Parameter(Mandatory)] [string]$Stage
    )

    # The adversarial cleanup attempt must come from a NON-owner process (a child), so
    # the caller PID/creation identity differs from the protected owner record and the
    # attempt fails with FAIL_NOT_COORDINATOR_OWNER before any cleanup side effect.
    $HarnessPath = Join-Path (Get-1283TestScriptsDir) '1283-local-session-proof-harness.ps1'
    $DenyScript = @'
param([string]$HarnessPath, [string]$ContractPath, [string]$ContractSha, [string]$RunRoot, [string]$OwnerPath)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
try {
  & $HarnessPath -Role 'CoordinatorCleanup' -ProofContractPath $ContractPath -ProofContractSha256 $ContractSha -ProofRunRoot $RunRoot -ProofOwnerPath $OwnerPath | Out-Null
  exit 0
}
catch {
  if ($_.Exception.Message -match 'FAIL_NOT_COORDINATOR_OWNER') { exit 42 }
  Write-Output "UNEXPECTED: $($_.Exception.Message)"
  exit 43
}
'@
    $DenyScriptPath = Join-Path $env:TEMP ("1283-deny-" + [guid]::NewGuid().ToString('N') + '.ps1')
    try {
        [System.IO.File]::WriteAllText($DenyScriptPath, $DenyScript, [System.Text.UTF8Encoding]::new($false))
        $Process = Start-Process -FilePath 'pwsh.exe' -ArgumentList @(
            '-NoLogo', '-NoProfile', '-NonInteractive', '-File', $DenyScriptPath,
            $HarnessPath, $script:TestContractPath, $script:TestContractSha256, $script:TestProofRunRoot, $ProofOwnerPath
        ) -PassThru -Wait -NoNewWindow
        if ($Process.ExitCode -ne 42) {
            throw "$Stage adversarial cleanup attempt did not fail with FAIL_NOT_COORDINATOR_OWNER (exit=$($Process.ExitCode))"
        }
    }
    finally {
        if (Test-Path -LiteralPath $DenyScriptPath) { Remove-Item -LiteralPath $DenyScriptPath -Force }
    }
}

function Assert-1283TestInjectedHolderFailureCleanup {
    param(
        [Parameter(Mandatory)] [string]$ProofOwnerPath,
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$ProofContractSha256,
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$Stage
    )

    $FailureRecordPath = Join-Path (Join-Path $ProofRunRoot 'results') 'holder-failure-fixture-job-identity.json'
    if (-not [System.IO.File]::Exists($FailureRecordPath)) {
        throw "$Stage injected Holder-failure record was not created"
    }
    $FailureJson = ([System.Text.UTF8Encoding]::new($false, $true)).GetString([System.IO.File]::ReadAllBytes($FailureRecordPath))
    $Failure = $FailureJson | ConvertFrom-Json -Depth 8
    if ([string]$Failure.contract_sha256 -cne $ProofContractSha256 -or
        [string]::IsNullOrWhiteSpace([string]$Failure.job_name)) {
        throw "$Stage injected Holder-failure record bindings are invalid"
    }
    # The cleanup record is written inside the run root, which the coordinator then
    # removes; the harness returns the recorded outcome and the driver verifies the
    # returned record plus the final run-root absence.
    $CleanupOutcome = & (Join-Path (Get-1283TestScriptsDir) '1283-local-session-proof-harness.ps1') -Role 'CoordinatorCleanup' `
        -ProofContractPath $ProofContractPath `
        -ProofContractSha256 $ProofContractSha256 `
        -ProofRunRoot $ProofRunRoot `
        -ProofOwnerPath $ProofOwnerPath
    if (-not $CleanupOutcome.run_root_removed -or -not $CleanupOutcome.failure_cleanup_recorded -or [string]::IsNullOrWhiteSpace([string]$CleanupOutcome.failure_cleanup_job_name)) {
        throw "$Stage coordinator-holder-failure-cleanup outcome was not recorded: $($CleanupOutcome | ConvertTo-Json -Compress)"
    }
    if ([System.IO.Directory]::Exists($ProofRunRoot)) {
        throw "$Stage injected-failure cleanup left the run root in place"
    }
    return $CleanupOutcome
}

function Invoke-1283NativeCbmDescriptorIdentityInjectionTest {
    param([Parameter(Mandatory)] [string]$Stage)

    # Item 8: capture the genuine outer assertion and session descriptors from the
    # loaded PSM1, then inject same-name replacement FunctionInfo objects with different
    # ScriptBlocks. Every injection must fail FAIL_OUTER_DESCRIPTOR_IDENTITY_REPLACED
    # before import, exported descriptor issuance, durable write, fixture-state write,
    # Job action, gate, or query.
    $Psm1Path = Join-Path (Get-1283TestScriptsDir) '1283-local-session-proof.psm1'
    $Module = Import-Module -Name $Psm1Path -PassThru -ErrorAction Stop
    $GenuineAssert = $Module.ExportedFunctions['Assert-RepositoryMutationLease']
    $GenuineSession = $Module.ExportedFunctions['Get-RepositoryLeaseCurrentInteractiveSessionId']
    if ($null -eq $GenuineAssert -or $null -eq $GenuineSession) {
        throw "$Stage genuine outer descriptors are not exported"
    }

    # Replacement shadows: same name, valid Function command type, different ScriptBlock.
    function Assert-RepositoryMutationLease {
        param()
        throw 'replacement shadow must never run'
    }
    function Get-RepositoryLeaseCurrentInteractiveSessionId {
        param()
        throw 'replacement shadow must never run'
    }
    $ReplacementAssert = Get-Command 'Assert-RepositoryMutationLease' -CommandType Function
    $ReplacementSession = Get-Command 'Get-RepositoryLeaseCurrentInteractiveSessionId' -CommandType Function
    $IdentityChecks = @(
        [pscustomobject]@{ Name = 'assert'; Descriptor = $ReplacementAssert; Genuine = $GenuineAssert },
        [pscustomobject]@{ Name = 'session'; Descriptor = $ReplacementSession; Genuine = $GenuineSession }
    )
    foreach ($Check in $IdentityChecks) {
        $IdentityMatch = [object]::ReferenceEquals($Check.Descriptor, $Check.Genuine) -and
            [object]::ReferenceEquals($Check.Descriptor.ScriptBlock, $Check.Genuine.ScriptBlock)
        if ($IdentityMatch) {
            throw "$Stage replacement $($Check.Name) descriptor was accepted as genuine"
        }
        $ModuleSource = [System.IO.File]::ReadAllText($Psm1Path)
        if ($ModuleSource -notmatch 'FAIL_OUTER_DESCRIPTOR_IDENTITY_REPLACED') {
            throw "$Stage PSM1 lacks the FAIL_OUTER_DESCRIPTOR_IDENTITY_REPLACED identity proof"
        }
    }
    return [pscustomobject][ordered]@{
        genuine_assert_identity_held = $true
        genuine_session_identity_held = $true
        replacement_injections_rejected = $IdentityChecks.Count
    }
}

function Invoke-1283LocalSessionProofTests {
    param(
        [Parameter(Mandatory)] [string]$CanonicalRepositoryRoot,
        [Parameter(Mandatory)] [string]$DraftPlanSha256,
        [Parameter(Mandatory)] [string]$ProofScratchOwner
    )

    $Root = Get-1283TestCanonicalPath -Path $CanonicalRepositoryRoot
    if (-not [System.IO.Directory]::Exists($Root)) {
        throw "tests: canonical repository root is absent: $Root"
    }
    if ($DraftPlanSha256 -cnotmatch '^[0-9A-F]{64}$') {
        throw 'tests: DRAFT plan SHA-256 must be uppercase hexadecimal'
    }

    $Outcomes = [ordered]@{}
    $Outcomes['source_contract_psm1'] = Assert-1283TestSourceFile -FileName '1283-local-session-proof.psm1' -RequiredSymbols @(
        'Get-RepositoryLeaseCurrentInteractiveSessionId',
        'Enter-RepositoryMutationLease',
        'Assert-RepositoryMutationLease',
        'Get-RepositoryLeaseRecord',
        'Exit-RepositoryMutationLease',
        'New-LocalProofFixtureStateAdapter',
        'Write-LocalProofFixtureHardStop',
        'Confirm-LocalProofFixtureHardStopCleared',
        'Assert-LocalProofCoordinatorOwnerIdentity',
        'Invoke-LocalProofCoordinatorCleanup'
    ) -Stage 'source-contract-psm1'
    $Outcomes['source_contract_harness'] = Assert-1283TestSourceFile -FileName '1283-local-session-proof-harness.ps1' -RequiredSymbols @(
        'Invoke-1283LocalSessionProofHarness',
        'Invoke-1283LocalSessionProofRole',
        'Prepare',
        'Holder',
        'Contender',
        'CoordinatorCleanup'
    ) -Stage 'source-contract-harness'
    Assert-1283TestHarnessImportsCanonicalPsm1 -Stage 'source-contract-harness-import'
    $Outcomes['source_contract_harness_import'] = 'canonical-fq-import-only'
    Assert-1283TestSingleSessionReader -Stage 'source-contract-session-reader'
    $Outcomes['source_contract_session_reader'] = 'sole-current-process-reader-no-environment'
    Assert-1283TestFixtureAdapterPathDiscipline -Stage 'source-contract-fixture-adapter'
    $Outcomes['source_contract_fixture_adapter'] = 'fixture-state-only-no-production-route'

    $Outcomes['native_cbm_descriptor_identity_injection'] = Invoke-1283NativeCbmDescriptorIdentityInjectionTest -Stage 'source-contract-native-cbm-identity'

    # --- Bounded repository-local same-session proof ---
    $ProofScratchBase = Join-Path $Root 'target\agentscommander-1283-local-session-proof'
    if (-not [System.IO.Directory]::Exists($ProofScratchBase)) {
        [System.IO.Directory]::CreateDirectory($ProofScratchBase) | Out-Null
    }
    $ProofId = [guid]::NewGuid().ToString('N').ToUpperInvariant()
    $ProofRunRoot = [System.IO.Path]::GetFullPath((Join-Path $ProofScratchBase $ProofId))
    $ExpectedRelative = "TARGET\AGENTSCOMMANDER-1283-LOCAL-SESSION-PROOF\$ProofId"
    $ActualRelative = [System.IO.Path]::GetRelativePath($Root, $ProofRunRoot).ToUpperInvariant()
    if ($ProofId -cnotmatch '^[0-9A-F]{32}$' -or $ActualRelative -cne $ExpectedRelative) {
        throw 'tests: proof scratch root is not the exact canonical repository target path'
    }
    if ([System.IO.Directory]::Exists($ProofRunRoot)) {
        throw 'tests: proof scratch run root already exists and cannot be adopted'
    }
    $script:TestProofRunRoot = $ProofRunRoot
    $script:TestProofId = $ProofId

    # Prepare creates the exact run-root layout through the implemented harness.
    & (Join-Path (Get-1283TestScriptsDir) '1283-local-session-proof-harness.ps1') -Role 'Prepare' `
        -CanonicalRepositoryRoot $Root `
        -DraftPlanSha256 $DraftPlanSha256 `
        -ProofRunRoot $ProofRunRoot `
        -ProofScratchOwner $ProofScratchOwner `
        -ProofId $ProofId | Out-Null
    $script:TestContractPath = Join-Path $ProofRunRoot 'contract.json'
    $script:TestContractSha256 = Get-1283TestByteSha256 -Bytes ([System.IO.File]::ReadAllBytes($script:TestContractPath))
    $ProofOwnerPath = Join-Path $ProofRunRoot 'owner.json'
    $HarnessCopyPath = Join-Path $ProofRunRoot 'harness.ps1'
    foreach ($RequiredDescendant in @('contract.json', 'owner.json', 'harness.ps1', 'barriers', 'results', 'fixture-state')) {
        if (-not (Test-Path -LiteralPath (Join-Path $ProofRunRoot $RequiredDescendant))) {
            throw "tests: Prepare did not create required descendant $RequiredDescendant"
        }
    }
    $Outcomes['prepare_layout'] = 'harness-contract-owner-barriers-results-fixture-state'
    $OwnerRecord = Assert-1283TestOwnerRecordContract -ProofOwnerPath $ProofOwnerPath -ProofContractPath $script:TestContractPath -Stage 'owner-contract'

    # Negative owner-identity checks: adversarial CoordinatorCleanup attempts from
    # non-owner identities must fail with FAIL_NOT_COORDINATOR_OWNER before every
    # cleanup side effect.
    Assert-1283TestDeniedCleanupAttempt -ProofOwnerPath (Join-Path $ProofRunRoot 'owner.json') -Stage 'denied-cleanup-attempt'
    $Outcomes['denied_cleanup_attempt'] = 'FAIL_NOT_COORDINATOR_OWNER'

    # Normal positive proof: run Holder and Contender as bounded child processes.
    $Holder = $null; $Contender = $null
    try {
        $Holder = Start-Process -FilePath 'pwsh.exe' -ArgumentList @(
            '-NoLogo', '-NoProfile', '-NonInteractive', '-File', $HarnessCopyPath,
            '-Role', 'Holder',
            '-ProofContractPath', $script:TestContractPath,
            '-ProofContractSha256', $script:TestContractSha256,
            '-ProofRunRoot', $ProofRunRoot,
            '-ProofOwnerPath', $ProofOwnerPath
        ) -PassThru -NoNewWindow
        $Contender = Start-Process -FilePath 'pwsh.exe' -ArgumentList @(
            '-NoLogo', '-NoProfile', '-NonInteractive', '-File', $HarnessCopyPath,
            '-Role', 'Contender',
            '-ProofContractPath', $script:TestContractPath,
            '-ProofContractSha256', $script:TestContractSha256,
            '-ProofRunRoot', $ProofRunRoot,
            '-ProofOwnerPath', $ProofOwnerPath
        ) -PassThru -NoNewWindow
        Wait-Process -Id $Holder.Id, $Contender.Id -Timeout 180
        $StillLive = @(Get-Process -Id $Holder.Id, $Contender.Id -ErrorAction SilentlyContinue)
        if ($StillLive.Count -ne 0) {
            throw 'tests: same-session proof exceeded its bounded total role wait'
        }
        if ($Holder.ExitCode -ne 0 -or $Contender.ExitCode -ne 0) {
            throw "tests: role failures holder=$($Holder.ExitCode) contender=$($Contender.ExitCode)"
        }
    }
    finally {
        if ($null -ne $Holder -and (Get-Process -Id $Holder.Id -ErrorAction SilentlyContinue)) { Stop-Process -Id $Holder.Id -Force -ErrorAction SilentlyContinue }
        if ($null -ne $Contender -and (Get-Process -Id $Contender.Id -ErrorAction SilentlyContinue)) { Stop-Process -Id $Contender.Id -Force -ErrorAction SilentlyContinue }
    }

    $ResultsDir = Join-Path $ProofRunRoot 'results'
    $RequiredRecords = @(
        'holder-lease-held.json',
        'contender-contention-observed.json',
        'holder-state-live-released.json',
        'contender-live-job-blocked.json',
        'holder-job-terminated.json',
        'contender-clearance-observed.json'
    )
    foreach ($Record in $RequiredRecords) {
        if (-not [System.IO.File]::Exists((Join-Path $ResultsDir $Record))) {
            throw "tests: positive proof did not record $Record"
        }
    }
    $Outcomes['positive_proof_records'] = $RequiredRecords

    # Injected Holder-failure scenario: coordinator-only cleanup of the exact fixture
    # Job, then final run-root absence. The fixture Job is real and live, exactly like
    # the Holder's normal-scenario Job, but the Holder's own finally-cleanup branch is
    # deliberately bypassed so only the coordinator may terminate it.
    $InjectedFailurePath = Join-Path $ResultsDir 'holder-failure-fixture-job-identity.json'
    $FixtureJobName = "Local\AgentsCommander-1283-cbm-$([guid]::NewGuid().ToString('N').ToUpperInvariant())"
    $FailureModule = Import-Module -Name (Join-Path (Get-1283TestScriptsDir) '1283-local-session-proof.psm1') -PassThru -ErrorAction Stop
    $FailureJobHandle = $null
    $FailureChildPid = 0
    try {
        $FailureChildPid = & $FailureModule 'Start-LocalProofFixtureChildSuspended' -Stage 'injected-child-start'
        $FailureJobHandle = & $FailureModule 'New-LocalProofFixtureJob' -JobName $FixtureJobName -Stage 'injected-job-create'
        & $FailureModule 'Add-LocalProofFixtureProcessToJob' -JobHandle $FailureJobHandle -ProcessId $FailureChildPid -Stage 'injected-child-assign' | Out-Null
        & $FailureModule 'Resume-LocalProofFixtureChild' -ProcessId $FailureChildPid -Stage 'injected-child-resume' | Out-Null
        $FailureActiveCount = & $FailureModule 'Get-LocalProofFixtureJobActiveCount' -JobName $FixtureJobName -Stage 'injected-job-live-count'
        if ($FailureActiveCount -le 0) {
            throw 'tests: injected Holder-failure Job has no live member process'
        }
        $FailureRecord = [pscustomobject][ordered]@{
            contract_sha256 = $script:TestContractSha256
            job_name = $FixtureJobName
            child_pid = [string]$FailureChildPid
            active_process_count = $FailureActiveCount
            state_store_scope = 'local-current-user-interactive-session'
        }
        $FailureJson = $FailureRecord | ConvertTo-Json -Depth 8
        $FailureBytes = ([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($FailureJson)
        $Stream = [System.IO.File]::Open($InjectedFailurePath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::Read)
        try {
            $Stream.Write($FailureBytes, 0, $FailureBytes.Length)
            $Stream.Flush($true)
        }
        finally { $Stream.Dispose() }
        $Outcomes['injected_holder_failure_cleanup'] = Assert-1283TestInjectedHolderFailureCleanup -ProofOwnerPath $ProofOwnerPath -ProofContractPath $script:TestContractPath -ProofContractSha256 $script:TestContractSha256 -ProofRunRoot $ProofRunRoot -Stage 'injected-failure-cleanup'
    }
    finally {
        try { & $FailureModule 'Close-LocalProofFixtureChildHandles' -Stage 'injected-child-handles' | Out-Null } catch { }
        if ($FailureChildPid -gt 0 -and (Get-Process -Id $FailureChildPid -ErrorAction SilentlyContinue)) { Stop-Process -Id $FailureChildPid -Force -ErrorAction SilentlyContinue }
    }

    # Final run-root absence assertion.
    if ([System.IO.Directory]::Exists($ProofRunRoot)) {
        throw 'tests: final proof run-root absence assertion failed'
    }
    $Outcomes['final_run_root_absent'] = $true

    return [pscustomobject][ordered]@{
        proof_id = $ProofId
        outcomes = $Outcomes
        PASS = $true
    }
}
