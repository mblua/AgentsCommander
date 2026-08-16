# AgentsCommander #1283 Step 8 proof-support module (Section 22.1.0.a-b).
#
# This module is the exact Local repository-mutation lease, private held-capability
# registry, current-process-session descriptor binding, LocalProofFixtureStateAdapter,
# protected fixture-Job identity record validation, and genuine coordinator-only
# cleanup interface. It introduces no Rust or TypeScript import, no Rust module arc,
# no product state-root override, and no SessionManager route. Its only writable root
# is the canonical <repo-AgentsCommander-root>\target\agentscommander-1283-local-session-proof\<proof-id>
# run root created by the Prepare role.

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepositoryLeaseAcquireTimeout = [TimeSpan]::FromSeconds(30)
$CanonicalPlanRelativePath = 'plans/1283-prevent-terminal-renderer-saturation.md'
$script:RepositoryLeaseCapabilityRegistry = [System.Runtime.CompilerServices.ConditionalWeakTable[object, object]]::new()
$script:RepositoryLeaseCapabilityIssuer = [object]::new()
$script:RepositoryLeaseKernelObjectNamespace = 'Local'
$script:RepositoryLeaseStateStoreScope = 'local-current-user-interactive-session'
$script:RepositoryLeaseMutexAclHostContract = 'Core-PowerShell-7.6.3/.NET-10.0.9/Windows/System.Threading.AccessControl'
if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows) -or -not [System.Environment]::UserInteractive) {
    throw 'Repository-mutation lease requires the current interactive Windows session'
}
$RepositoryLeaseCurrentIdentity = [System.Security.Principal.WindowsIdentity]::GetCurrent()
if ($null -eq $RepositoryLeaseCurrentIdentity -or $null -eq $RepositoryLeaseCurrentIdentity.User -or [string]::IsNullOrWhiteSpace($RepositoryLeaseCurrentIdentity.User.Value)) {
    throw 'Repository-mutation lease cannot establish the current-user Local SID scope'
}
function Get-RepositoryLeaseCurrentInteractiveSessionId {
    param([Parameter(Mandatory)] [string]$Stage)

    $CurrentProcess = $null
    $SessionIdValue = $null
    try {
        $CurrentProcess = [System.Diagnostics.Process]::GetCurrentProcess()
        if ($null -eq $CurrentProcess) { throw 'GetCurrentProcess returned no process' }
        $SessionIdValue = $CurrentProcess.SessionId
    }
    catch {
        throw "$Stage cannot obtain the current-process interactive session ID: $($_.Exception.Message)"
    }
    finally {
        if ($null -ne $CurrentProcess) { $CurrentProcess.Dispose() }
    }
    if ($null -eq $SessionIdValue -or $SessionIdValue -isnot [int]) {
        throw "$Stage current-process interactive session ID is absent or non-integral"
    }
    if ($SessionIdValue -le 0) {
        throw "$Stage current-process interactive session ID must be positive"
    }
    return $SessionIdValue
}
function Get-RepositoryLeaseCurrentLogonLuid {
    param([Parameter(Mandatory)] [System.Security.Principal.WindowsIdentity]$Identity)

    if ($null -eq ('AgentsCommander.Review1283.RepositoryLeaseSessionInterop' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace AgentsCommander.Review1283 {
  [StructLayout(LayoutKind.Sequential)] public struct RepositoryLeaseLuid { public uint LowPart; public int HighPart; }
  [StructLayout(LayoutKind.Sequential)] public struct RepositoryLeaseTokenStatistics {
    public RepositoryLeaseLuid TokenId; public RepositoryLeaseLuid AuthenticationId; public long ExpirationTime;
    public int TokenType; public int ImpersonationLevel; public uint DynamicCharged; public uint DynamicAvailable;
    public uint GroupCount; public uint PrivilegeCount; public RepositoryLeaseLuid ModifiedId;
  }
  public static class RepositoryLeaseSessionInterop {
    const int TokenStatistics = 10; const int ErrorInsufficientBuffer = 122;
    [DllImport("advapi32.dll", SetLastError=true)] static extern bool GetTokenInformation(IntPtr token, int informationClass, IntPtr information, int informationLength, out int returnLength);
    public static string GetAuthenticationLuid(IntPtr token) {
      int length; GetTokenInformation(token, TokenStatistics, IntPtr.Zero, 0, out length);
      if (length <= 0 || Marshal.GetLastWin32Error() != ErrorInsufficientBuffer) throw new InvalidOperationException("GetTokenInformation TokenStatistics length probe failed");
      IntPtr buffer = Marshal.AllocHGlobal(length);
      try {
        if (!GetTokenInformation(token, TokenStatistics, buffer, length, out length)) throw new InvalidOperationException("GetTokenInformation TokenStatistics read failed: " + Marshal.GetLastWin32Error());
        var statistics = Marshal.PtrToStructure<RepositoryLeaseTokenStatistics>(buffer);
        return ((uint)statistics.AuthenticationId.HighPart).ToString("X8") + statistics.AuthenticationId.LowPart.ToString("X8");
      } finally { Marshal.FreeHGlobal(buffer); }
    }
  }
}
'@ -ErrorAction Stop | Out-Null
    }
    $Type = 'AgentsCommander.Review1283.RepositoryLeaseSessionInterop' -as [type]
    if ($null -eq $Type) { throw 'Repository-mutation lease cannot load the local logon-session helper' }
    $Luid = $Type::GetAuthenticationLuid($Identity.AccessToken.DangerousGetHandle())
    if ($Luid -cnotmatch '^[0-9A-F]{16}$') { throw 'Repository-mutation lease cannot establish the current logon-session LUID' }
    return $Luid
}
$script:RepositoryLeasePrincipalSid = $RepositoryLeaseCurrentIdentity.User.Value
$script:RepositoryLeaseMachineName = [System.Environment]::MachineName.ToUpperInvariant()
$script:RepositoryLeaseInteractiveSessionId = Get-RepositoryLeaseCurrentInteractiveSessionId -Stage 'repository-lease-initial-current-process-session'
$script:RepositoryLeaseInteractiveLogonLuid = Get-RepositoryLeaseCurrentLogonLuid -Identity $RepositoryLeaseCurrentIdentity
if ([string]::IsNullOrWhiteSpace($script:RepositoryLeaseMachineName) -or $script:RepositoryLeaseInteractiveSessionId -le 0) {
    throw 'Repository-mutation lease cannot establish the current interactive-session identity'
}
$script:RepositoryLeasePrincipalSidHash = [System.Convert]::ToHexString(
    [System.Security.Cryptography.SHA256]::HashData(
        ([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($script:RepositoryLeasePrincipalSid)
    )
)
$RepositoryLeaseSessionBindingMaterial = 'local-v2' + [char]0 + $script:RepositoryLeaseMachineName + [char]0 + $script:RepositoryLeasePrincipalSid + [char]0 + $script:RepositoryLeaseInteractiveSessionId.ToString([System.Globalization.CultureInfo]::InvariantCulture) + [char]0 + $script:RepositoryLeaseInteractiveLogonLuid
$script:RepositoryLeaseInteractiveSessionBindingSha256 = [System.Convert]::ToHexString(
    [System.Security.Cryptography.SHA256]::HashData(
        ([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($RepositoryLeaseSessionBindingMaterial)
    )
)

function Assert-SupportedRepositoryMutexAclRuntime {
    param([Parameter(Mandatory)] [string]$Stage)

    if ($PSVersionTable.PSEdition -cne 'Core' -or $PSVersionTable.PSVersion -ne [version]'7.6.3' -or [System.Environment]::Version -ne [version]'10.0.9') {
        throw "$Stage requires the tested $script:RepositoryLeaseMutexAclHostContract host; no compatibility fallback is permitted"
    }
    try { Add-Type -AssemblyName 'System.Threading.AccessControl' -ErrorAction Stop | Out-Null }
    catch { throw "$Stage cannot load System.Threading.AccessControl: $($_.Exception.Message)" }
    $MutexAclType = 'System.Threading.MutexAcl' -as [type]
    $MutexSecurityType = 'System.Security.AccessControl.MutexSecurity' -as [type]
    $MutexRightsType = 'System.Security.AccessControl.MutexRights' -as [type]
    $SectionsType = 'System.Security.AccessControl.AccessControlSections' -as [type]
    if ($null -eq $MutexAclType -or $null -eq $MutexSecurityType -or $null -eq $MutexRightsType -or $null -eq $SectionsType -or $MutexAclType.Assembly.GetName().Name -cne 'System.Threading.AccessControl') {
        throw "$Stage lacks the tested MutexAcl access-control types"
    }
    $Flags = [System.Reflection.BindingFlags]::Public -bor [System.Reflection.BindingFlags]::Static
    $CreateMethods = @($MutexAclType.GetMethods($Flags) | Where-Object {
        $Parameters = @($_.GetParameters())
        $_.Name -ceq 'Create' -and $Parameters.Count -eq 4 -and $Parameters[0].ParameterType -eq [bool] -and $Parameters[1].ParameterType -eq [string] -and $Parameters[2].ParameterType.IsByRef -and $Parameters[2].ParameterType.GetElementType() -eq [bool] -and $Parameters[3].ParameterType -eq $MutexSecurityType
    })
    $OpenMethods = @($MutexAclType.GetMethods($Flags) | Where-Object {
        $Parameters = @($_.GetParameters())
        $_.Name -ceq 'OpenExisting' -and $Parameters.Count -eq 2 -and $Parameters[0].ParameterType -eq [string] -and $Parameters[1].ParameterType -eq $MutexRightsType
    })
    $SecurityConstructors = @($MutexSecurityType.GetConstructors() | Where-Object {
        $Parameters = @($_.GetParameters())
        $Parameters.Count -eq 2 -and $Parameters[0].ParameterType -eq [string] -and $Parameters[1].ParameterType -eq $SectionsType
    })
    if ($CreateMethods.Count -ne 1 -or $OpenMethods.Count -ne 1 -or $SecurityConstructors.Count -ne 1) {
        throw "$Stage lacks the required MutexAcl.Create, MutexAcl.OpenExisting, or named MutexSecurity constructor"
    }
}

function ConvertTo-CanonicalAbsolutePath {
    param([Parameter(Mandatory)] [string]$Path)

    $FullPath = [System.IO.Path]::GetFullPath($Path)
    $TrimCharacters = [char[]]@(
        [System.IO.Path]::DirectorySeparatorChar,
        [System.IO.Path]::AltDirectorySeparatorChar
    )
    return $FullPath.TrimEnd($TrimCharacters).ToUpperInvariant()
}

function Get-CanonicalRepositoryRoot {
    param([Parameter(Mandatory)] [string]$RepositoryRoot)

    $ResolvedRoot = (Resolve-Path -LiteralPath $RepositoryRoot -ErrorAction Stop).Path
    $GitRoot = (& git -C $ResolvedRoot rev-parse --show-toplevel).Trim()
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($GitRoot)) {
        throw "Could not resolve canonical Git worktree root for $RepositoryRoot"
    }
    return ConvertTo-CanonicalAbsolutePath -Path $GitRoot
}

function Resolve-CanonicalPlanPath {
    param(
        [Parameter(Mandatory)] [string]$CanonicalRepositoryRoot,
        [Parameter(Mandatory)] [string]$RepositoryRelativePlanPath
    )

    if ($RepositoryRelativePlanPath -cne $CanonicalPlanRelativePath) {
        throw "Unexpected plan-relative path: $RepositoryRelativePlanPath"
    }
    $ReverifiedRoot = Get-CanonicalRepositoryRoot -RepositoryRoot $CanonicalRepositoryRoot
    if ($ReverifiedRoot -cne $CanonicalRepositoryRoot) {
        throw 'Canonical repository root changed while resolving the plan path'
    }
    $ExpectedPlanPath = ConvertTo-CanonicalAbsolutePath -Path (
        Join-Path $CanonicalRepositoryRoot $RepositoryRelativePlanPath
    )
    $RootPrefix = $CanonicalRepositoryRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $ExpectedPlanPath.StartsWith($RootPrefix, [System.StringComparison]::Ordinal)) {
        throw 'Canonical plan path escapes the canonical repository root'
    }
    $ResolvedPlanPath = ConvertTo-CanonicalAbsolutePath -Path (
        (Resolve-Path -LiteralPath $ExpectedPlanPath -ErrorAction Stop).Path
    )
    if ($ResolvedPlanPath -cne $ExpectedPlanPath) {
        throw "Resolved plan path differs from canonical plan path: $ResolvedPlanPath"
    }
    return $ExpectedPlanPath
}

function Resolve-PriorReadyIdentityBinding {
    param(
        [Parameter(Mandatory)] [string]$Stage,
        [Parameter(Mandatory)] $PriorReadyIdentityRecord
    )

    if ($null -eq $PriorReadyIdentityRecord) {
        throw "$Stage requires the prior READY identity record"
    }
    foreach ($Field in @('canonical_repository_root', 'canonical_plan_path', 'ready_sha256')) {
        $Property = $PriorReadyIdentityRecord.PSObject.Properties[$Field]
        if ($null -eq $Property -or $Property.Value -isnot [string] -or [string]::IsNullOrWhiteSpace($Property.Value)) {
            throw "$Stage prior READY identity record has no nonempty string $Field"
        }
    }

    $RecordedRoot = [string]$PriorReadyIdentityRecord.canonical_repository_root
    $RecordedPlanPath = [string]$PriorReadyIdentityRecord.canonical_plan_path
    $RecordedReadySha256 = [string]$PriorReadyIdentityRecord.ready_sha256
    if (-not [System.IO.Path]::IsPathFullyQualified($RecordedRoot)) {
        throw "$Stage prior READY canonical root must be absolute"
    }
    if (-not [System.IO.Path]::IsPathFullyQualified($RecordedPlanPath)) {
        throw "$Stage prior READY canonical plan path must be absolute"
    }
    if ($RecordedReadySha256 -cnotmatch '^[0-9A-F]{64}$') {
        throw "$Stage prior READY SHA-256 must be uppercase hexadecimal"
    }
    if ((ConvertTo-CanonicalAbsolutePath -Path $RecordedRoot) -cne $RecordedRoot) {
        throw "$Stage prior READY root is not canonical"
    }
    if ((ConvertTo-CanonicalAbsolutePath -Path $RecordedPlanPath) -cne $RecordedPlanPath) {
        throw "$Stage prior READY plan path is not canonical"
    }

    $RevalidatedRoot = Get-CanonicalRepositoryRoot -RepositoryRoot $RecordedRoot
    if ($RevalidatedRoot -cne $RecordedRoot) {
        throw "$Stage prior READY root differs from its canonical Git worktree root"
    }
    $RevalidatedPlanPath = Resolve-CanonicalPlanPath `
        -CanonicalRepositoryRoot $RevalidatedRoot `
        -RepositoryRelativePlanPath $CanonicalPlanRelativePath
    if ($RevalidatedPlanPath -cne $RecordedPlanPath) {
        throw "$Stage prior READY plan path differs from the canonical plan path"
    }

    return [pscustomobject][ordered]@{
        canonical_repository_root = $RevalidatedRoot
        canonical_plan_path = $RevalidatedPlanPath
        ready_sha256 = $RecordedReadySha256
    }
}

function Get-RepositoryLeaseName {
    param([Parameter(Mandatory)] [string]$CanonicalRepositoryRoot)

    if ($script:RepositoryLeaseKernelObjectNamespace -cne 'Local' -or $script:RepositoryLeaseStateStoreScope -cne 'local-current-user-interactive-session' -or $script:RepositoryLeasePrincipalSid -cnotmatch '^S-\d-(?:\d+-)+\d+$' -or $script:RepositoryLeaseInteractiveSessionId -le 0 -or $script:RepositoryLeaseInteractiveLogonLuid -cnotmatch '^[0-9A-F]{16}$' -or $script:RepositoryLeaseInteractiveSessionBindingSha256 -cnotmatch '^[0-9A-F]{64}$') {
        throw 'Repository lease has no trusted Local current-user interactive-session scope'
    }
    $Utf8WithoutBom = [System.Text.UTF8Encoding]::new($false, $true)
    $RootHash = [System.Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData(
            $Utf8WithoutBom.GetBytes($CanonicalRepositoryRoot)
        )
    )
    return "Local\AgentsCommander-1283-repository-$($RootHash.Substring(0, 40))-$($script:RepositoryLeasePrincipalSidHash.Substring(0, 16))-$($script:RepositoryLeaseInteractiveSessionId)-$($script:RepositoryLeaseInteractiveSessionBindingSha256.Substring(0, 24))"
}

function New-RepositoryMutationMutexSecurity {
    $Sid = [System.Security.Principal.SecurityIdentifier]::new($script:RepositoryLeasePrincipalSid)
    $Security = [System.Security.AccessControl.MutexSecurity]::new()
    $Security.SetOwner($Sid)
    $Security.SetAccessRuleProtection($true, $false)
    [void]$Security.AddAccessRule([System.Security.AccessControl.MutexAccessRule]::new(
        $Sid,
        [System.Security.AccessControl.MutexRights]::FullControl,
        [System.Security.AccessControl.AccessControlType]::Allow
    ))
    return $Security
}

function Assert-RepositoryMutationMutexSecurity {
    param(
        [Parameter(Mandatory)] [string]$ExpectedLeaseName,
        [Parameter(Mandatory)] [string]$Stage
    )

    $ExpectedNamePattern = '^Local\\AgentsCommander-1283-repository-[0-9A-F]{40}-' + $script:RepositoryLeasePrincipalSidHash.Substring(0, 16) + '-' + [regex]::Escape($script:RepositoryLeaseInteractiveSessionId.ToString([System.Globalization.CultureInfo]::InvariantCulture)) + '-' + $script:RepositoryLeaseInteractiveSessionBindingSha256.Substring(0, 24) + '$'
    if ($ExpectedLeaseName -cnotmatch $ExpectedNamePattern) {
        throw "$Stage mutex name is not in the required Local current-user interactive-session scope"
    }
    Assert-SupportedRepositoryMutexAclRuntime -Stage "$Stage-host-contract"
    $RequiredRights = [System.Security.AccessControl.MutexRights]::Modify -bor [System.Security.AccessControl.MutexRights]::Synchronize -bor [System.Security.AccessControl.MutexRights]::ReadPermissions
    $Probe = $null
    try {
        $Probe = [System.Threading.MutexAcl]::OpenExisting($ExpectedLeaseName, $RequiredRights)
        if ($null -eq $Probe -or $Probe.SafeWaitHandle.IsClosed -or $Probe.SafeWaitHandle.IsInvalid) {
            throw 'MutexAcl.OpenExisting did not return a usable mutex handle'
        }
        $Sections = [System.Security.AccessControl.AccessControlSections]::Owner -bor [System.Security.AccessControl.AccessControlSections]::Access
        $Security = [System.Security.AccessControl.MutexSecurity]::new($ExpectedLeaseName, $Sections)
    }
    catch {
        throw "$Stage cannot open and inspect the Local repository-mutex ACL through MutexAcl: $($_.Exception.Message)"
    }
    finally {
        if ($null -ne $Probe) { $Probe.Dispose() }
    }
    $Owner = $Security.GetOwner([System.Security.Principal.SecurityIdentifier])
    if ($null -eq $Owner -or $Owner.Value -cne $script:RepositoryLeasePrincipalSid -or -not $Security.AreAccessRulesProtected) {
        throw "$Stage Local repository-mutex owner or DACL is not trusted"
    }
    $ExpectedAllowCount = 0
    foreach ($Rule in @($Security.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))) {
        if ($Rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow -or $Rule.IdentityReference.Value -cne $script:RepositoryLeasePrincipalSid) {
            throw "$Stage Local repository-mutex ACL has an unexpected principal or rule"
        }
        if (($Rule.MutexRights -band [System.Security.AccessControl.MutexRights]::FullControl) -ne [System.Security.AccessControl.MutexRights]::FullControl) {
            throw "$Stage Local repository-mutex ACL lacks current-user full control"
        }
        $ExpectedAllowCount++
    }
    if ($ExpectedAllowCount -ne 1) {
        throw "$Stage Local repository-mutex ACL is ambiguous"
    }
}

function Get-HeldRepositoryMutationLeaseCapability {
    param(
        [Parameter(Mandatory)] $Lease,
        [Parameter(Mandatory)] [string]$CanonicalRepositoryRoot,
        [Parameter(Mandatory)] [string]$Stage
    )

    $CurrentProcessSessionId = Get-RepositoryLeaseCurrentInteractiveSessionId -Stage "$Stage-current-process-session"
    if ($CurrentProcessSessionId -ne $script:RepositoryLeaseInteractiveSessionId) {
        throw "$Stage current-process interactive session differs from the Local lease scope"
    }
    $Capability = $null
    if ($null -eq $Lease -or -not $script:RepositoryLeaseCapabilityRegistry.TryGetValue($Lease, [ref]$Capability)) {
        throw "$Stage lease has no registered in-memory capability"
    }
    $ExpectedLeaseName = Get-RepositoryLeaseName -CanonicalRepositoryRoot $CanonicalRepositoryRoot
    if ($null -eq $Capability -or -not [object]::ReferenceEquals($Capability.issuer, $script:RepositoryLeaseCapabilityIssuer) -or $Capability.state -cne 'held' -or $Capability.lease_id -cne $Lease.lease_id -or $Capability.lease_name -cne $ExpectedLeaseName -or $Lease.lease_name -cne $ExpectedLeaseName -or $Capability.repository_root -cne $CanonicalRepositoryRoot -or $Lease.repository_root -cne $CanonicalRepositoryRoot -or $Capability.kernel_namespace -cne $script:RepositoryLeaseKernelObjectNamespace -or $Lease.kernel_namespace -cne $script:RepositoryLeaseKernelObjectNamespace -or $Capability.principal_sid -cne $script:RepositoryLeasePrincipalSid -or $Lease.principal_sid -cne $script:RepositoryLeasePrincipalSid -or $Capability.machine_name -cne $script:RepositoryLeaseMachineName -or $Lease.machine_name -cne $script:RepositoryLeaseMachineName -or $Capability.interactive_session_id -ne $script:RepositoryLeaseInteractiveSessionId -or $Lease.interactive_session_id -ne $script:RepositoryLeaseInteractiveSessionId -or $Capability.interactive_logon_luid -cne $script:RepositoryLeaseInteractiveLogonLuid -or $Lease.interactive_logon_luid -cne $script:RepositoryLeaseInteractiveLogonLuid -or $Capability.interactive_session_binding_sha256 -cne $script:RepositoryLeaseInteractiveSessionBindingSha256 -or $Lease.interactive_session_binding_sha256 -cne $script:RepositoryLeaseInteractiveSessionBindingSha256 -or $Capability.state_store_scope -cne $script:RepositoryLeaseStateStoreScope -or $Lease.state_store_scope -cne $script:RepositoryLeaseStateStoreScope -or $Capability.mutex_creation_state -cne $Lease.mutex_creation_state -or $Lease.mutex_creation_state -cnotin @('created-and-reopened-verified', 'opened-existing-and-reopened-verified') -or $Capability.owner_thread_id -ne [System.Threading.Thread]::CurrentThread.ManagedThreadId -or $Lease.owner_thread_id -ne [System.Threading.Thread]::CurrentThread.ManagedThreadId) {
        throw "$Stage lease differs from its registered held capability"
    }
    if ($Capability.mutex.SafeWaitHandle.IsClosed -or $Capability.mutex.SafeWaitHandle.IsInvalid) {
        throw "$Stage held-capability mutex handle is unavailable"
    }
    Assert-RepositoryMutationMutexSecurity -ExpectedLeaseName $ExpectedLeaseName -Stage "$Stage-mutex-acl"
    return $Capability
}

function Revoke-RepositoryMutationLeaseCapability {
    param(
        [Parameter(Mandatory)] $Lease,
        [Parameter(Mandatory)] [string]$State,
        [Parameter(Mandatory)] [string]$Stage
    )

    $Capability = $null
    if ($null -ne $Lease -and $script:RepositoryLeaseCapabilityRegistry.TryGetValue($Lease, [ref]$Capability)) {
        $Capability.state = $State
        if (-not $script:RepositoryLeaseCapabilityRegistry.Remove($Lease)) {
            throw "$Stage could not remove the repository-lease capability"
        }
    }
}

function Get-RepositoryLeaseRecord {
    param(
        [Parameter(Mandatory)]$Lease,
        [Parameter(Mandatory)] [string]$CanonicalRepositoryRoot
    )

    if ($Lease.repository_root -cne $CanonicalRepositoryRoot) {
        throw 'Repository lease record root differs from the supplied canonical root'
    }

    return [pscustomobject][ordered]@{
        lease_name = $Lease.lease_name
        lease_id = $Lease.lease_id
        repository_root = $Lease.repository_root
        kernel_namespace = $Lease.kernel_namespace
        principal_sid = $Lease.principal_sid
        machine_name = $Lease.machine_name
        interactive_session_id = $Lease.interactive_session_id
        interactive_logon_luid = $Lease.interactive_logon_luid
        interactive_session_binding_sha256 = $Lease.interactive_session_binding_sha256
        state_store_scope = $Lease.state_store_scope
        mutex_creation_state = $Lease.mutex_creation_state
        purpose = $Lease.purpose
        owner_thread_id = $Lease.owner_thread_id
        acquired_utc = $Lease.acquired_utc
        released = [bool]$Lease.released
        release_state = $Lease.release_state
        release_confirmed = [bool]$Lease.release_confirmed
        released_utc = $Lease.released_utc
        release_error = $Lease.release_error
    }
}

function Enter-RepositoryMutationLease {
    param(
        [Parameter(Mandatory)] [string]$CanonicalRepositoryRoot,
        [Parameter(Mandatory)] [string]$Purpose,
        [Parameter(Mandatory)] [TimeSpan]$AcquireTimeout
    )

    if ($AcquireTimeout -le [TimeSpan]::Zero) {
        throw 'Repository lease timeout must be positive'
    }
    Assert-SupportedRepositoryMutexAclRuntime -Stage 'repository-lease-enter-host-contract'
    $CurrentProcessSessionId = Get-RepositoryLeaseCurrentInteractiveSessionId -Stage 'repository-lease-enter-current-process-session'
    if ($CurrentProcessSessionId -ne $script:RepositoryLeaseInteractiveSessionId) {
        throw 'Current-process interactive session differs from the Local repository-lease scope'
    }
    $ReverifiedRoot = Get-CanonicalRepositoryRoot -RepositoryRoot $CanonicalRepositoryRoot
    if ($ReverifiedRoot -cne $CanonicalRepositoryRoot) {
        throw 'Canonical repository root changed before lease acquisition'
    }
    $LeaseName = Get-RepositoryLeaseName -CanonicalRepositoryRoot $CanonicalRepositoryRoot
    $ExpectedLeaseNamePattern = '^Local\\AgentsCommander-1283-repository-[0-9A-F]{40}-' + $script:RepositoryLeasePrincipalSidHash.Substring(0, 16) + '-' + [regex]::Escape($script:RepositoryLeaseInteractiveSessionId.ToString([System.Globalization.CultureInfo]::InvariantCulture)) + '-' + $script:RepositoryLeaseInteractiveSessionBindingSha256.Substring(0, 24) + '$'
    if ($LeaseName -cnotmatch $ExpectedLeaseNamePattern) {
        throw 'Repository lease name is not a Local current-user interactive-session mutex name'
    }
    $Mutex = $null
    $CreatedMutex = $null
    $OpenedMutex = $null
    [bool]$CreatedNew = $false
    $Acquired = $false
    $Lease = $null
    $CapabilityIssued = $false
    try {
        $Security = New-RepositoryMutationMutexSecurity
        try {
            $CreatedMutex = [System.Threading.MutexAcl]::Create($false, $LeaseName, [ref]$CreatedNew, $Security)
        }
        catch {
            throw "Local repository-mutex MutexAcl.Create failed: $($_.Exception.Message)"
        }
        try {
            $RequiredRights = [System.Security.AccessControl.MutexRights]::Modify -bor [System.Security.AccessControl.MutexRights]::Synchronize -bor [System.Security.AccessControl.MutexRights]::ReadPermissions
            $OpenedMutex = [System.Threading.MutexAcl]::OpenExisting($LeaseName, $RequiredRights)
            if ($null -eq $OpenedMutex -or $OpenedMutex.SafeWaitHandle.IsClosed -or $OpenedMutex.SafeWaitHandle.IsInvalid) { throw 'MutexAcl.OpenExisting did not return a usable mutex handle' }
            Assert-RepositoryMutationMutexSecurity -ExpectedLeaseName $LeaseName -Stage 'repository-lease-reopened-mutex'
        }
        catch {
            throw "Local repository-mutex ACL or reopen verification failed: $($_.Exception.Message)"
        }
        finally {
            if ($null -ne $CreatedMutex) { $CreatedMutex.Dispose(); $CreatedMutex = $null }
        }
        if ($null -eq $OpenedMutex) {
            throw 'Local repository-mutex reopen produced no handle'
        }
        $Mutex = $OpenedMutex
        $OpenedMutex = $null
        $MutexCreationState = if ($CreatedNew) { 'created-and-reopened-verified' } else { 'opened-existing-and-reopened-verified' }
        try {
            $Acquired = $Mutex.WaitOne($AcquireTimeout)
        }
        catch [System.Threading.AbandonedMutexException] {
            $Acquired = $true
            throw "Repository lease $LeaseName was abandoned; its write state is not trusted"
        }
        if (-not $Acquired) {
            throw "Timed out acquiring repository lease $LeaseName"
        }
        $Lease = [pscustomobject]@{
            lease_name = $LeaseName
            lease_id = [guid]::NewGuid().ToString('N')
            repository_root = $CanonicalRepositoryRoot
            kernel_namespace = $script:RepositoryLeaseKernelObjectNamespace
            principal_sid = $script:RepositoryLeasePrincipalSid
            machine_name = $script:RepositoryLeaseMachineName
            interactive_session_id = $script:RepositoryLeaseInteractiveSessionId
            interactive_logon_luid = $script:RepositoryLeaseInteractiveLogonLuid
            interactive_session_binding_sha256 = $script:RepositoryLeaseInteractiveSessionBindingSha256
            state_store_scope = $script:RepositoryLeaseStateStoreScope
            mutex_creation_state = $MutexCreationState
            purpose = $Purpose
            owner_thread_id = [System.Threading.Thread]::CurrentThread.ManagedThreadId
            acquired_utc = [DateTime]::UtcNow.ToString('O')
            released = $false
            release_state = 'held'
            release_confirmed = $false
            released_utc = $null
            release_error = $null
        }
        if ($Mutex.SafeWaitHandle.IsClosed -or $Mutex.SafeWaitHandle.IsInvalid) {
            throw 'Repository lease mutex became unavailable before capability issuance'
        }
        $Capability = [pscustomobject]@{
            issuer = $script:RepositoryLeaseCapabilityIssuer
            lease_id = [string]$Lease.lease_id
            lease_name = [string]$Lease.lease_name
            repository_root = $CanonicalRepositoryRoot
            kernel_namespace = $script:RepositoryLeaseKernelObjectNamespace
            principal_sid = $script:RepositoryLeasePrincipalSid
            machine_name = $script:RepositoryLeaseMachineName
            interactive_session_id = $script:RepositoryLeaseInteractiveSessionId
            interactive_logon_luid = $script:RepositoryLeaseInteractiveLogonLuid
            interactive_session_binding_sha256 = $script:RepositoryLeaseInteractiveSessionBindingSha256
            state_store_scope = $script:RepositoryLeaseStateStoreScope
            mutex_creation_state = $MutexCreationState
            owner_thread_id = [int]$Lease.owner_thread_id
            mutex = $Mutex
            state = 'held'
        }
        $script:RepositoryLeaseCapabilityRegistry.Add($Lease, $Capability)
        $CapabilityIssued = $true
        return $Lease
    }
    catch {
        if ($CapabilityIssued) {
            try { Revoke-RepositoryMutationLeaseCapability -Lease $Lease -State 'enter-failed' -Stage 'repository-lease-enter-failure' } catch { }
        }
        if ($Acquired) {
            try { $Mutex.ReleaseMutex() } catch { }
        }
        if ($null -ne $Mutex) { $Mutex.Dispose() }
        if ($null -ne $OpenedMutex) { $OpenedMutex.Dispose() }
        if ($null -ne $CreatedMutex) { $CreatedMutex.Dispose() }
        throw
    }
}

function Assert-RepositoryMutationLease {
    param(
        [Parameter(Mandatory)]$Lease,
        [Parameter(Mandatory)] [string]$CanonicalRepositoryRoot,
        [Parameter(Mandatory)] [string]$Stage
    )

    $Capability = Get-HeldRepositoryMutationLeaseCapability -Lease $Lease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-held-capability"
    if ($Lease.release_state -cne 'held' -or [bool]$Lease.released -or [bool]$Lease.release_confirmed) { throw "$Stage uses a non-held repository lease" }
    $ActualRoot = Get-CanonicalRepositoryRoot -RepositoryRoot $CanonicalRepositoryRoot
    if ($ActualRoot -cne $CanonicalRepositoryRoot -or $ActualRoot -cne $Lease.repository_root) {
        throw "$Stage repository root differs from the held lease root"
    }
    if ($Capability.repository_root -cne $ActualRoot) { throw "$Stage registered held capability root differs after root validation" }
    return Get-RepositoryLeaseRecord -Lease $Lease -CanonicalRepositoryRoot $CanonicalRepositoryRoot
}

function Exit-RepositoryMutationLease {
    param(
        [Parameter(Mandatory)]$Lease,
        [Parameter(Mandatory)] [string]$CanonicalRepositoryRoot,
        [Parameter(Mandatory)] [string]$Stage
    )

    $Capability = Get-HeldRepositoryMutationLeaseCapability -Lease $Lease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-held-capability"
    if ($Lease.release_state -cne 'held' -or [bool]$Lease.released -or [bool]$Lease.release_confirmed) { throw "$Stage cannot release a non-held repository lease" }
    $ActualRoot = Get-CanonicalRepositoryRoot -RepositoryRoot $CanonicalRepositoryRoot
    if ($ActualRoot -cne $CanonicalRepositoryRoot -or $ActualRoot -cne $Lease.repository_root) {
        throw "$Stage repository root differs from the held lease root during release"
    }
    $ReleaseMutexSucceeded = $false
    try {
        $Capability.mutex.ReleaseMutex()
        $ReleaseMutexSucceeded = $true
        $Capability.mutex.Dispose()
        Revoke-RepositoryMutationLeaseCapability -Lease $Lease -State 'revoked' -Stage "$Stage-capability-revoke"
        $Lease.released = $true
        $Lease.release_state = 'released-confirmed'
        $Lease.release_confirmed = $true
        $Lease.released_utc = [DateTime]::UtcNow.ToString('O')
        return Get-RepositoryLeaseRecord -Lease $Lease -CanonicalRepositoryRoot $CanonicalRepositoryRoot
    }
    catch {
        try { Revoke-RepositoryMutationLeaseCapability -Lease $Lease -State 'revoked-untrusted' -Stage "$Stage-capability-revoke-failure" } catch { }
        $Lease.released = $ReleaseMutexSucceeded
        $Lease.release_state = if ($ReleaseMutexSucceeded) { 'release-dispose-failed-untrusted' } else { 'release-failed-untrusted' }
        $Lease.release_confirmed = $false
        $Lease.release_error = $_.Exception.Message
        throw
    }
}

function Get-ByteSha256 {
    param([Parameter(Mandatory)] [byte[]]$Bytes)

    return [System.Convert]::ToHexString(
        [System.Security.Cryptography.SHA256]::HashData($Bytes)
    )
}

$NativeCbmEvidenceControlModuleSource = {
  param(
    [Parameter(Mandatory)] [System.Management.Automation.FunctionInfo]$RepositoryLeaseAssertDescriptor,
    [Parameter(Mandatory)] [System.Management.Automation.FunctionInfo]$RepositoryLeaseSessionIdDescriptor,
    [Parameter(Mandatory)] [System.Management.Automation.FunctionInfo]$GenuineRepositoryLeaseAssertDescriptor,
    [Parameter(Mandatory)] [System.Management.Automation.ScriptBlock]$GenuineRepositoryLeaseAssertScriptBlock,
    [Parameter(Mandatory)] [System.Management.Automation.FunctionInfo]$GenuineRepositoryLeaseSessionIdDescriptor,
    [Parameter(Mandatory)] [System.Management.Automation.ScriptBlock]$GenuineRepositoryLeaseSessionIdScriptBlock,
    [Parameter(Mandatory)] [string]$LoaderProvidedSkillMarkdownPath,
    [Parameter(Mandatory)] [string]$TrustedInstalledSkillsRootPath,
    [Parameter(Mandatory)] [string]$KernelObjectNamespace,
    [Parameter(Mandatory)] [string]$PrincipalSid,
    [Parameter(Mandatory)] [string]$MachineName,
    [Parameter(Mandatory)] [int]$InteractiveSessionId,
    [Parameter(Mandatory)] [string]$InteractiveLogonLuid,
    [Parameter(Mandatory)] [string]$InteractiveSessionBindingSha256,
    [Parameter(Mandatory)] [string]$StateStoreScope
  )
  Set-StrictMode -Version Latest
  $ErrorActionPreference = 'Stop'
  if ($RepositoryLeaseAssertDescriptor.Name -cne 'Assert-RepositoryMutationLease' -or $RepositoryLeaseAssertDescriptor.CommandType -ne [System.Management.Automation.CommandTypes]::Function -or [object]::ReferenceEquals($RepositoryLeaseAssertDescriptor, $GenuineRepositoryLeaseAssertDescriptor) -eq $false -or [object]::ReferenceEquals($RepositoryLeaseAssertDescriptor.ScriptBlock, $GenuineRepositoryLeaseAssertScriptBlock) -eq $false) {
    throw 'FAIL_OUTER_DESCRIPTOR_IDENTITY_REPLACED: outer lease assertion is not the genuine captured descriptor'
  }
  if ($RepositoryLeaseSessionIdDescriptor.Name -cne 'Get-RepositoryLeaseCurrentInteractiveSessionId' -or $RepositoryLeaseSessionIdDescriptor.CommandType -ne [System.Management.Automation.CommandTypes]::Function -or [object]::ReferenceEquals($RepositoryLeaseSessionIdDescriptor, $GenuineRepositoryLeaseSessionIdDescriptor) -eq $false -or [object]::ReferenceEquals($RepositoryLeaseSessionIdDescriptor.ScriptBlock, $GenuineRepositoryLeaseSessionIdScriptBlock) -eq $false) {
    throw 'FAIL_OUTER_DESCRIPTOR_IDENTITY_REPLACED: outer current-process session helper is not the genuine captured descriptor'
  }
  foreach ($BoundPath in @([pscustomobject]@{ value = $LoaderProvidedSkillMarkdownPath; label = 'loader SKILL.md' }, [pscustomobject]@{ value = $TrustedInstalledSkillsRootPath; label = 'installed-skills root' })) {
    if ([string]::IsNullOrWhiteSpace($BoundPath.value)) { throw "NativeCbm control module requires the pre-bound fully-qualified $($BoundPath.label) path" }
    if (-not [System.IO.Path]::IsPathFullyQualified([string]$BoundPath.value)) {
      if ([string]$BoundPath.value -cmatch '^[A-Za-z]:(?![\\/])') { throw "NativeCbm control module rejects drive-relative $($BoundPath.label) paths" }
      if ([string]$BoundPath.value -cmatch '^[\\/](?![\\/])') { throw "NativeCbm control module rejects root-relative $($BoundPath.label) paths" }
      throw "NativeCbm control module requires the pre-bound fully-qualified $($BoundPath.label) path"
    }
  }
  # The held Local lease bootstrap has already created this verified outer
  # helper before it can import the native control module.
  if ($null -eq ('AgentsCommander.Review1283.RepositoryLeaseSessionInterop' -as [type])) {
    throw 'NativeCbm control module requires the pre-bound current logon-session helper'
  }
  $CurrentMachineName = [System.Environment]::MachineName.ToUpperInvariant()
  $CurrentSessionId = & $RepositoryLeaseSessionIdDescriptor -Stage 'native-cbm-control-module-current-process-session'
  if ($CurrentSessionId -isnot [int] -or $CurrentSessionId -le 0) {
    throw 'NativeCbm control module received an invalid current-process interactive session ID'
  }
  $CurrentLogonLuid = [AgentsCommander.Review1283.RepositoryLeaseSessionInterop]::GetAuthenticationLuid(([System.Security.Principal.WindowsIdentity]::GetCurrent().AccessToken.DangerousGetHandle()))
  $SessionBindingMaterial = 'local-v2' + [char]0 + $CurrentMachineName + [char]0 + $PrincipalSid + [char]0 + $CurrentSessionId.ToString([System.Globalization.CultureInfo]::InvariantCulture) + [char]0 + $CurrentLogonLuid
  $ExpectedSessionBindingSha256 = [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData(([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($SessionBindingMaterial)))
  if ($KernelObjectNamespace -cne 'Local' -or $PrincipalSid -cnotmatch '^S-\d-(?:\d+-)+\d+$' -or [string]::IsNullOrWhiteSpace($MachineName) -or $MachineName -cne $CurrentMachineName -or $InteractiveSessionId -le 0 -or $InteractiveSessionId -ne $CurrentSessionId -or $InteractiveLogonLuid -cnotmatch '^[0-9A-F]{16}$' -or $InteractiveLogonLuid -cne $CurrentLogonLuid -or $InteractiveSessionBindingSha256 -cnotmatch '^[0-9A-F]{64}$' -or $InteractiveSessionBindingSha256 -cne $ExpectedSessionBindingSha256 -or $StateStoreScope -cne 'local-current-user-interactive-session') {
    throw 'NativeCbm control module requires the exact Local current-user interactive-session scope'
  }
  $script:NativeCbmGenuineOuterAssertionDescriptor = $GenuineRepositoryLeaseAssertDescriptor
  $script:NativeCbmGenuineOuterAssertionScriptBlock = $GenuineRepositoryLeaseAssertScriptBlock
  $script:NativeCbmGenuineOuterSessionIdDescriptor = $GenuineRepositoryLeaseSessionIdDescriptor
  $script:NativeCbmGenuineOuterSessionIdScriptBlock = $GenuineRepositoryLeaseSessionIdScriptBlock
  $script:NativeCbmOriginalLoaderSkillMarkdownPath = $LoaderProvidedSkillMarkdownPath
  $script:NativeCbmTrustedInstalledSkillsRootPath = $TrustedInstalledSkillsRootPath
  $script:NativeCbmKernelObjectNamespace = $KernelObjectNamespace
  $script:NativeCbmPrincipalSid = $PrincipalSid
  $script:NativeCbmPrincipalSidHash = [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData(([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($PrincipalSid)))
  $script:NativeCbmMachineName = $MachineName
  $script:NativeCbmInteractiveSessionId = $InteractiveSessionId
  $script:NativeCbmInteractiveLogonLuid = $InteractiveLogonLuid
  $script:NativeCbmInteractiveSessionBindingSha256 = $InteractiveSessionBindingSha256
  $script:NativeCbmStateStoreScope = $StateStoreScope
  $script:NativeCbmWrapperCapabilityRegistry = [System.Runtime.CompilerServices.ConditionalWeakTable[object, object]]::new()
  $script:NativeCbmWrapperCapabilityIssuer = [object]::new()
  $script:NativeCbmRebindWrapperCapability = $null
  $script:NativeCbmEvidenceControlSchemaVersion = 4
  $script:NativeCbmUnconfirmedJobs = [System.Collections.Generic.List[object]]::new()
  $script:NativeCbmControlModuleName = $ExecutionContext.SessionState.Module.Name
  $script:NativeCbmControlModuleGuid = $ExecutionContext.SessionState.Module.Guid.ToString('D')

  if (-not [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform([System.Runtime.InteropServices.OSPlatform]::Windows)) {
    throw 'Native Codebase Memory containment is supported only on Windows'
  }
  if (-not [System.Environment]::UserInteractive -or [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value -cne $script:NativeCbmPrincipalSid -or $CurrentSessionId -ne $script:NativeCbmInteractiveSessionId -or $CurrentLogonLuid -cne $script:NativeCbmInteractiveLogonLuid) {
    throw 'NativeCbm control module current SID or interactive session differs from the Local lease scope'
  }
  if ($null -ne ('AgentsCommander.Review1283.NativeCbmJobInterop' -as [type])) {
    throw 'A preexisting NativeCbm interop type makes control-module identity untrusted'
  }

  Add-Type -TypeDefinition @'
using System;
using System.IO;
using System.Text;
using System.Runtime.InteropServices;
using System.Threading;
using System.Threading.Tasks;

namespace AgentsCommander.Review1283 {
  [StructLayout(LayoutKind.Sequential)]
  public struct JOBOBJECT_BASIC_ACCOUNTING_INFORMATION {
    public long TotalUserTime; public long TotalKernelTime; public long ThisPeriodTotalUserTime; public long ThisPeriodTotalKernelTime;
    public uint TotalPageFaultCount; public uint TotalProcesses; public uint ActiveProcesses; public uint TotalTerminatedProcesses;
  }
  [StructLayout(LayoutKind.Sequential)]
  public struct IO_COUNTERS {
    public ulong ReadOperationCount; public ulong WriteOperationCount; public ulong OtherOperationCount;
    public ulong ReadTransferCount; public ulong WriteTransferCount; public ulong OtherTransferCount;
  }
  [StructLayout(LayoutKind.Sequential)]
  public struct JOBOBJECT_BASIC_LIMIT_INFORMATION {
    public long PerProcessUserTimeLimit; public long PerJobUserTimeLimit; public uint LimitFlags;
    public UIntPtr MinimumWorkingSetSize; public UIntPtr MaximumWorkingSetSize; public uint ActiveProcessLimit;
    public UIntPtr Affinity; public uint PriorityClass; public uint SchedulingClass;
  }
  [StructLayout(LayoutKind.Sequential)]
  public struct JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
    public JOBOBJECT_BASIC_LIMIT_INFORMATION BasicLimitInformation; public IO_COUNTERS IoInfo;
    public UIntPtr ProcessMemoryLimit; public UIntPtr JobMemoryLimit; public UIntPtr PeakProcessMemoryUsed; public UIntPtr PeakJobMemoryUsed;
  }
  [StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)]
  public struct STARTUPINFOW {
    public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
    public int dwX; public int dwY; public int dwXSize; public int dwYSize; public int dwXCountChars; public int dwYCountChars;
    public int dwFillAttribute; public uint dwFlags; public short wShowWindow; public short cbReserved2;
    public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError;
  }
  [StructLayout(LayoutKind.Sequential)]
  public struct PROCESS_INFORMATION {
    public IntPtr hProcess; public IntPtr hThread; public uint dwProcessId; public uint dwThreadId;
  }
  [StructLayout(LayoutKind.Sequential)]
  public struct FILETIME_NATIVE { public uint dwLowDateTime; public uint dwHighDateTime; }
  [StructLayout(LayoutKind.Sequential)]
  public struct BY_HANDLE_FILE_INFORMATION {
    public uint FileAttributes; public FILETIME_NATIVE CreationTime; public FILETIME_NATIVE LastAccessTime; public FILETIME_NATIVE LastWriteTime;
    public uint VolumeSerialNumber; public uint FileSizeHigh; public uint FileSizeLow; public uint NumberOfLinks;
    public uint FileIndexHigh; public uint FileIndexLow;
  }
  public struct NativeFileIdentity {
    public uint VolumeSerialNumber; public ulong FileIndex;
  }
  [StructLayout(LayoutKind.Sequential)]
  public struct SECURITY_ATTRIBUTES_NATIVE {
    public int nLength; public IntPtr lpSecurityDescriptor; [MarshalAs(UnmanagedType.Bool)] public bool bInheritHandle;
  }
  public static class NativeCbmJobInterop {
    public const uint JOB_OBJECT_ASSIGN_PROCESS = 0x0001;
    public const uint JOB_OBJECT_QUERY = 0x0004;
    public const uint JOB_OBJECT_TERMINATE = 0x0008;
    public const uint JOB_OBJECT_LIMIT_BREAKAWAY_OK = 0x00000800;
    public const uint JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK = 0x00001000;
    public const uint JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE = 0x00002000;
    public const int JobObjectBasicAccountingInformation = 1;
    public const int JobObjectExtendedLimitInformation = 9;
    public const uint CREATE_SUSPENDED = 0x00000004;
    public const uint CREATE_NO_WINDOW = 0x08000000;
    public const uint STARTF_USESTDHANDLES = 0x00000100;
    public const uint WAIT_OBJECT_0 = 0x00000000;
    public const uint WAIT_TIMEOUT = 0x00000102;
    public const uint WAIT_FAILED = 0xFFFFFFFF;
    public const uint PROCESS_TERMINATE = 0x0001;
    public const uint PROCESS_QUERY_LIMITED_INFORMATION = 0x1000;
    public const uint SYNCHRONIZE = 0x00100000;
    public const uint FILE_READ_ATTRIBUTES = 0x00000080;
    public const uint FILE_SHARE_READ = 0x00000001;
    public const uint FILE_SHARE_WRITE = 0x00000002;
    public const uint FILE_SHARE_DELETE = 0x00000004;
    public const uint OPEN_EXISTING = 3;
    public const uint FILE_FLAG_BACKUP_SEMANTICS = 0x02000000;
    public const uint FILE_FLAG_OPEN_REPARSE_POINT = 0x00200000;
    public const uint FILE_ATTRIBUTE_REPARSE_POINT = 0x00000400;
    public const int ERROR_FILE_NOT_FOUND = 2;
    public const int ERROR_ACCESS_DENIED = 5;
    public const int ERROR_INVALID_PARAMETER = 87;
    public const int ERROR_ALREADY_EXISTS = 183;

    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern IntPtr CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern IntPtr OpenJobObject(uint access, bool inherit, string name);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool IsProcessInJob(IntPtr process, IntPtr job, out bool result);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool SetInformationJobObject(IntPtr job, int infoClass, IntPtr info, uint length);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool QueryInformationJobObject(IntPtr job, int infoClass, out JOBOBJECT_BASIC_ACCOUNTING_INFORMATION info, uint length, out uint returned);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool TerminateJobObject(IntPtr job, uint exitCode);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true, EntryPoint="CreateProcessW")]
    public static extern bool CreateProcessW(string applicationName, StringBuilder commandLine, IntPtr processAttributes, IntPtr threadAttributes, bool inheritHandles, uint creationFlags, IntPtr environment, string currentDirectory, ref STARTUPINFOW startupInfo, out PROCESS_INFORMATION processInformation);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool TerminateProcess(IntPtr process, uint exitCode);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool GetExitCodeProcess(IntPtr process, out uint exitCode);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool GetFileInformationByHandle(IntPtr handle, out BY_HANDLE_FILE_INFORMATION info);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true, EntryPoint="CreateFileW")]
    public static extern IntPtr CreateFileW(string path, uint desiredAccess, uint shareMode, IntPtr securityAttributes, uint creationDisposition, uint flagsAndAttributes, IntPtr templateFile);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern uint GetFinalPathNameByHandleW(IntPtr handle, StringBuilder path, uint length, uint flags);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true, EntryPoint="CreateJobObjectW")]
    public static extern IntPtr CreateJobObjectWithSecurity(ref SECURITY_ATTRIBUTES_NATIVE attributes, string name);
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true, EntryPoint="ConvertStringSecurityDescriptorToSecurityDescriptorW")]
    public static extern bool ConvertStringSecurityDescriptorToSecurityDescriptorW(string sddl, uint revision, out IntPtr descriptor, out uint descriptorSize);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern IntPtr LocalFree(IntPtr memory);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern void SetLastError(uint errorCode);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern IntPtr OpenProcess(uint access, bool inherit, uint processId);
    [DllImport("kernel32.dll", SetLastError=true)]
    public static extern bool GetProcessTimes(IntPtr process, out FILETIME_NATIVE creation, out FILETIME_NATIVE exit, out FILETIME_NATIVE kernel, out FILETIME_NATIVE user);

    public static NativeFileIdentity GetFileIdentity(IntPtr handle) {
      BY_HANDLE_FILE_INFORMATION info;
      if (!GetFileInformationByHandle(handle, out info)) throw new IOException("GetFileInformationByHandle failed: " + Marshal.GetLastWin32Error());
      NativeFileIdentity identity = new NativeFileIdentity();
      identity.VolumeSerialNumber = info.VolumeSerialNumber;
      identity.FileIndex = ((ulong)info.FileIndexHigh << 32) | info.FileIndexLow;
      return identity;
    }
    public static IntPtr OpenPathForPhysicalVerification(string path, bool directory) {
      uint flags = FILE_FLAG_OPEN_REPARSE_POINT | (directory ? FILE_FLAG_BACKUP_SEMANTICS : 0u);
      IntPtr handle = CreateFileW(path, FILE_READ_ATTRIBUTES, FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, IntPtr.Zero, OPEN_EXISTING, flags, IntPtr.Zero);
      if (handle == new IntPtr(-1)) throw new IOException("CreateFileW physical verification failed: " + Marshal.GetLastWin32Error());
      return handle;
    }
    public static uint GetPathFileAttributes(IntPtr handle) {
      BY_HANDLE_FILE_INFORMATION info;
      if (!GetFileInformationByHandle(handle, out info)) throw new IOException("GetFileInformationByHandle failed: " + Marshal.GetLastWin32Error());
      return info.FileAttributes;
    }
    public static string GetFinalPathByHandle(IntPtr handle) {
      StringBuilder path = new StringBuilder(32768);
      uint written = GetFinalPathNameByHandleW(handle, path, (uint)path.Capacity, 0);
      if (written == 0 || written >= path.Capacity) throw new IOException("GetFinalPathNameByHandleW failed: " + Marshal.GetLastWin32Error());
      string value = path.ToString();
      if (value.StartsWith(@"\\?\UNC\", StringComparison.OrdinalIgnoreCase)) return @"\\" + value.Substring(8);
      if (value.StartsWith(@"\\?\", StringComparison.OrdinalIgnoreCase)) return value.Substring(4);
      return value;
    }
    public static IntPtr CreateJobObjectWithSddl(string name, string sddl, out int createError) {
      IntPtr descriptor = IntPtr.Zero;
      createError = 0;
      if (!ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl, 1, out descriptor, out uint ignored)) throw new IOException("ConvertStringSecurityDescriptorToSecurityDescriptorW failed: " + Marshal.GetLastWin32Error());
      try {
        SECURITY_ATTRIBUTES_NATIVE attributes = new SECURITY_ATTRIBUTES_NATIVE();
        attributes.nLength = Marshal.SizeOf(typeof(SECURITY_ATTRIBUTES_NATIVE));
        attributes.lpSecurityDescriptor = descriptor;
        attributes.bInheritHandle = false;
        SetLastError(0);
        IntPtr job = CreateJobObjectWithSecurity(ref attributes, name);
        createError = Marshal.GetLastWin32Error();
        return job;
      }
      finally { LocalFree(descriptor); }
    }
    public static long GetProcessCreationFileTime(IntPtr process) {
      FILETIME_NATIVE creation, exit, kernel, user;
      if (!GetProcessTimes(process, out creation, out exit, out kernel, out user)) throw new IOException("GetProcessTimes failed: " + Marshal.GetLastWin32Error());
      return ((long)creation.dwHighDateTime << 32) | creation.dwLowDateTime;
    }
    public static string QuoteWindowsArgument(string value) {
      if (value == null) throw new ArgumentNullException(nameof(value));
      if (value.Length == 0) return "\"\"";
      bool quote = false;
      for (int i = 0; i < value.Length; i++) if (Char.IsWhiteSpace(value[i]) || value[i] == '"') { quote = true; break; }
      if (!quote) return value;
      StringBuilder result = new StringBuilder();
      result.Append('"');
      int slashes = 0;
      foreach (char c in value) {
        if (c == '\\') { slashes++; continue; }
        if (c == '"') { result.Append('\\', slashes * 2 + 1); result.Append('"'); slashes = 0; continue; }
        result.Append('\\', slashes); slashes = 0; result.Append(c);
      }
      result.Append('\\', slashes * 2); result.Append('"');
      return result.ToString();
    }
    public static StringBuilder BuildCommandLine(string executable, string[] arguments) {
      StringBuilder result = new StringBuilder(QuoteWindowsArgument(executable));
      foreach (string argument in arguments) { result.Append(' '); result.Append(QuoteWindowsArgument(argument)); }
      return result;
    }
    public static async Task<byte[]> ReadBoundedAsync(Stream stream, int maximumBytes, CancellationToken cancellationToken) {
      if (maximumBytes < 0) throw new ArgumentOutOfRangeException(nameof(maximumBytes));
      byte[] buffer = new byte[8192];
      using (MemoryStream destination = new MemoryStream()) {
        while (true) {
          int read = await stream.ReadAsync(buffer, 0, buffer.Length, cancellationToken).ConfigureAwait(false);
          if (read == 0) return destination.ToArray();
          if (destination.Length + read > maximumBytes) throw new InvalidDataException("native stream exceeded its byte limit");
          destination.Write(buffer, 0, read);
        }
      }
    }
    public static async Task WriteBoundedAsync(Stream stream, byte[] payload, int maximumBytes, CancellationToken cancellationToken) {
      if (payload == null) throw new ArgumentNullException(nameof(payload));
      if (maximumBytes < 1) throw new ArgumentOutOfRangeException(nameof(maximumBytes));
      if (payload.Length > maximumBytes) throw new InvalidDataException("native stdin payload exceeded its byte limit");
      await stream.WriteAsync(payload, 0, payload.Length, cancellationToken).ConfigureAwait(false);
      await stream.FlushAsync(cancellationToken).ConfigureAwait(false);
    }
  }
}
'@ -ErrorAction Stop

  function ConvertTo-NativeCbmControlCanonicalPath {
    param([Parameter(Mandatory)] [string]$Path)
    ([System.IO.Path]::GetFullPath($Path)).TrimEnd([char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)).ToUpperInvariant()
  }
  function Get-NativeCbmControlRootHash {
    param([Parameter(Mandatory)] [string]$CanonicalRepositoryRoot)
    [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData(([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($CanonicalRepositoryRoot)))
  }
  function Get-NativeCbmControlBytesSha256 {
    param([Parameter(Mandatory)] [byte[]]$Bytes)
    [System.Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData($Bytes))
  }
  function Get-NativeCbmSha256FromStream {
    param([Parameter(Mandatory)] [System.IO.Stream]$Stream)
    if (-not $Stream.CanSeek) { throw 'NativeCbm stream is not seekable' }
    $Stream.Position = 0
    $Hasher = [System.Security.Cryptography.SHA256]::Create()
    try { [System.Convert]::ToHexString($Hasher.ComputeHash($Stream)) }
    finally { $Hasher.Dispose(); $Stream.Position = 0 }
  }
  function Assert-NativeCbmControlLease {
    param([object]$RepositoryLease, [string]$CanonicalRepositoryRoot, [string]$Stage)
    $LeaseRecord = & $script:NativeCbmGenuineOuterAssertionDescriptor -Lease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-outer-held-capability"
    if ($null -eq $LeaseRecord -or $LeaseRecord.repository_root -cne $CanonicalRepositoryRoot -or $LeaseRecord.lease_id -cne $RepositoryLease.lease_id -or $LeaseRecord.release_state -cne 'held' -or [bool]$LeaseRecord.released -or [bool]$LeaseRecord.release_confirmed -or $LeaseRecord.kernel_namespace -cne $script:NativeCbmKernelObjectNamespace -or $LeaseRecord.principal_sid -cne $script:NativeCbmPrincipalSid -or $LeaseRecord.machine_name -cne $script:NativeCbmMachineName -or $LeaseRecord.interactive_session_id -ne $script:NativeCbmInteractiveSessionId -or $LeaseRecord.interactive_logon_luid -cne $script:NativeCbmInteractiveLogonLuid -or $LeaseRecord.interactive_session_binding_sha256 -cne $script:NativeCbmInteractiveSessionBindingSha256 -or $LeaseRecord.state_store_scope -cne $script:NativeCbmStateStoreScope) {
      throw "$Stage outer held-lease capability record differs from the supplied lease"
    }
    return $LeaseRecord
  }
  function Get-NativeCbmControlStatePaths {
    param([string]$CanonicalRepositoryRoot)
    $RootHash = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRootHash'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot
    $SessionDirectory = "$($script:NativeCbmInteractiveSessionId)-$($script:NativeCbmInteractiveSessionBindingSha256.Substring(0, 24))"
    $Directory = Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)) ("AgentsCommander/review-state/1283/native-cbm/local/" + $script:NativeCbmPrincipalSidHash + "/" + $SessionDirectory)
    [pscustomobject]@{ root_hash = $RootHash; kernel_namespace = $script:NativeCbmKernelObjectNamespace; principal_sid = $script:NativeCbmPrincipalSid; machine_name = $script:NativeCbmMachineName; interactive_session_id = $script:NativeCbmInteractiveSessionId; interactive_logon_luid = $script:NativeCbmInteractiveLogonLuid; interactive_session_binding_sha256 = $script:NativeCbmInteractiveSessionBindingSha256; state_store_scope = $script:NativeCbmStateStoreScope; directory = $Directory; head_path = Join-Path $Directory "$RootHash.json" }
  }
  function Assert-NativeCbmLocalSessionStateStore {
    param([psobject]$Paths, [bool]$CreateIfMissing, [string]$Stage)
    if ($Paths.kernel_namespace -cne 'Local' -or $Paths.principal_sid -cne $script:NativeCbmPrincipalSid -or $Paths.machine_name -cne $script:NativeCbmMachineName -or $Paths.interactive_session_id -ne $script:NativeCbmInteractiveSessionId -or $Paths.interactive_logon_luid -cne $script:NativeCbmInteractiveLogonLuid -or $Paths.interactive_session_binding_sha256 -cne $script:NativeCbmInteractiveSessionBindingSha256 -or $Paths.state_store_scope -cne 'local-current-user-interactive-session') { throw "$Stage state store scope differs from the Local interactive-session lease scope" }
    $Sid = [System.Security.Principal.SecurityIdentifier]::new($script:NativeCbmPrincipalSid)
    if (-not [System.IO.Directory]::Exists($Paths.directory)) {
      if (-not $CreateIfMissing) { return $false }
      $Security = [System.Security.AccessControl.DirectorySecurity]::new()
      $Security.SetOwner($Sid)
      $Security.SetAccessRuleProtection($true, $false)
      $Rights = [System.Security.AccessControl.FileSystemRights]::FullControl
      $Inheritance = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
      [void]$Security.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new($Sid, $Rights, $Inheritance, [System.Security.AccessControl.PropagationFlags]::None, [System.Security.AccessControl.AccessControlType]::Allow))
      [System.IO.Directory]::CreateDirectory($Paths.directory, $Security) | Out-Null
    }
    $Existing = [System.IO.Directory]::GetAccessControl($Paths.directory, [System.Security.AccessControl.AccessControlSections]::Owner -bor [System.Security.AccessControl.AccessControlSections]::Access)
    if ($Existing.GetOwner([System.Security.Principal.SecurityIdentifier]).Value -cne $script:NativeCbmPrincipalSid -or -not $Existing.AreAccessRulesProtected) { throw "$Stage Local interactive-session state-store owner or DACL is not trusted" }
    $Allowed = $false
    foreach ($Rule in @($Existing.GetAccessRules($true, $true, [System.Security.Principal.SecurityIdentifier]))) {
      if ($Rule.AccessControlType -ne [System.Security.AccessControl.AccessControlType]::Allow -or $Rule.IdentityReference.Value -cne $script:NativeCbmPrincipalSid) { throw "$Stage Local interactive-session state-store ACL has an unexpected principal or rule" }
      if (($Rule.FileSystemRights -band [System.Security.AccessControl.FileSystemRights]::FullControl) -eq [System.Security.AccessControl.FileSystemRights]::FullControl) { $Allowed = $true }
    }
    if (-not $Allowed) { throw "$Stage Local interactive-session state-store ACL lacks current-user full control" }
    return $true
  }
  function Get-NativeCbmControlRequiredString {
    param([System.Collections.IDictionary]$Record, [string]$Field, [string]$Stage)
    if (-not $Record.Contains($Field) -or $Record[$Field] -isnot [string] -or [string]::IsNullOrWhiteSpace([string]$Record[$Field])) { throw "$Stage requires nonempty $Field" }
    [string]$Record[$Field]
  }
  function Read-NativeCbmControlState {
    param([string]$CanonicalRepositoryRoot, [string]$CanonicalPlanPath, [object]$RepositoryLease, [string]$Stage)
    & $script:NativeCbmPrivateControls['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-before-state-read-held-capability" | Out-Null
    $Paths = & $script:NativeCbmPrivateControls['Get-NativeCbmControlStatePaths'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot
    if (-not (& $script:NativeCbmPrivateControls['Assert-NativeCbmLocalSessionStateStore'] -Paths $Paths -CreateIfMissing $false -Stage "$Stage-state-store-read")) { & $script:NativeCbmPrivateControls['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-after-empty-state-store-held-capability" | Out-Null; return $null }
    if (-not [System.IO.File]::Exists($Paths.head_path)) { & $script:NativeCbmPrivateControls['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-after-empty-state-file-held-capability" | Out-Null; return $null }
    $Bytes = [System.IO.File]::ReadAllBytes($Paths.head_path)
    try {
      if ($Bytes.Length -lt 1 -or $Bytes.Length -gt 1048576) { throw 'state byte length is outside the strict bound' }
      $Record = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($Bytes) | ConvertFrom-Json -AsHashtable -Depth 32
    }
    catch { throw "$Stage durable state is unreadable: $($_.Exception.Message)" }
    if ($Record -isnot [System.Collections.IDictionary]) { throw "$Stage durable state is not an object record" }
    $RequireExactKeys = {
      param([string[]]$ExpectedKeys)
      $ActualKeys = @($Record.Keys | ForEach-Object { [string]$_ } | Sort-Object)
      $RequiredKeys = @($ExpectedKeys | Sort-Object)
      if ($ActualKeys.Count -ne $RequiredKeys.Count -or $null -ne (Compare-Object -ReferenceObject $RequiredKeys -DifferenceObject $ActualKeys -CaseSensitive)) { throw "$Stage durable state has an unexpected field set" }
    }
    foreach ($Field in @('schema_version', 'state', 'kernel_namespace', 'principal_sid', 'machine_name', 'interactive_session_id', 'interactive_logon_luid', 'interactive_session_binding_sha256', 'state_store_scope', 'canonical_repository_root', 'canonical_plan_path', 'root_hash', 'record_id', 'written_utc')) {
      if (-not $Record.Contains($Field)) { throw "$Stage durable state lacks $Field" }
    }
    if ((($Record['schema_version'] -isnot [int]) -and ($Record['schema_version'] -isnot [long])) -or [int]$Record['schema_version'] -ne $script:NativeCbmEvidenceControlSchemaVersion) { throw "$Stage durable state has an unsupported schema" }
    if ((& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'kernel_namespace' -Stage $Stage) -cne $Paths.kernel_namespace -or (& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'principal_sid' -Stage $Stage) -cne $Paths.principal_sid -or (& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'machine_name' -Stage $Stage) -cne $Paths.machine_name -or (($Record['interactive_session_id'] -isnot [int]) -and ($Record['interactive_session_id'] -isnot [long])) -or [int]$Record['interactive_session_id'] -ne $Paths.interactive_session_id -or (& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'interactive_logon_luid' -Stage $Stage) -cne $Paths.interactive_logon_luid -or (& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'interactive_session_binding_sha256' -Stage $Stage) -cne $Paths.interactive_session_binding_sha256 -or (& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'state_store_scope' -Stage $Stage) -cne $Paths.state_store_scope) { throw "$Stage FOREIGN_INTERACTIVE_SESSION_HARD_STOP" }
    if ((& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'canonical_repository_root' -Stage $Stage) -cne $CanonicalRepositoryRoot -or (& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'canonical_plan_path' -Stage $Stage) -cne $CanonicalPlanPath -or (& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'root_hash' -Stage $Stage) -cne $Paths.root_hash) { throw "$Stage durable state canonical identity is invalid" }
    if ((& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'record_id' -Stage $Stage) -cnotmatch '^[0-9a-f]{32}$' -or (& $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'written_utc' -Stage $Stage) -cnotmatch '^\d{4}-\d{2}-\d{2}T') { throw "$Stage durable state record metadata is invalid" }
    $State = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'state' -Stage $Stage
    if ($State -ceq 'cleared') {
      & $RequireExactKeys @('schema_version', 'state', 'kernel_namespace', 'principal_sid', 'machine_name', 'interactive_session_id', 'interactive_logon_luid', 'interactive_session_binding_sha256', 'state_store_scope', 'canonical_repository_root', 'canonical_plan_path', 'root_hash', 'draft_plan_sha256', 'clearance_evidence_epoch', 'clearance_session_id', 'record_id', 'written_utc')
      $PlanHash = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'draft_plan_sha256' -Stage $Stage
      $ClearanceEpoch = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'clearance_evidence_epoch' -Stage $Stage
      if ($PlanHash -cnotmatch '^[0-9A-F]{64}$' -or $ClearanceEpoch -cnotmatch '^[0-9a-f]{32}$' -or (($Record['clearance_session_id'] -isnot [int]) -and ($Record['clearance_session_id'] -isnot [long])) -or [int]$Record['clearance_session_id'] -ne $Paths.interactive_session_id) { throw "$Stage cleared durable state has invalid Local interactive-session authority fields" }
    }
    elseif ($State -ceq 'unconfirmed-tree-termination') {
      & $RequireExactKeys @('schema_version', 'state', 'kernel_namespace', 'principal_sid', 'machine_name', 'interactive_session_id', 'interactive_logon_luid', 'interactive_session_binding_sha256', 'state_store_scope', 'origin_session_id', 'containment_scope', 'canonical_repository_root', 'canonical_plan_path', 'root_hash', 'draft_plan_sha256', 'evidence_epoch', 'stage', 'job_name', 'bootstrap_root_pid', 'bootstrap_root_creation_filetime', 'process_count_state', 'job_active_processes', 'termination_error', 'confirmation_error', 'executed_artifact', 'wrapper_identity', 'record_id', 'written_utc')
      $PlanHash = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'draft_plan_sha256' -Stage $Stage
      $Epoch = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'evidence_epoch' -Stage $Stage
      $Scope = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'containment_scope' -Stage $Stage
      $JobName = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'job_name' -Stage $Stage
      $FailureStage = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRequiredString'] -Record $Record -Field 'stage' -Stage $Stage
      $ExpectedJobPattern = '^Local\\AgentsCommander-1283-cbm-[0-9A-F]{64}-[0-9A-F]{16}-' + [regex]::Escape($Paths.interactive_session_id.ToString([System.Globalization.CultureInfo]::InvariantCulture)) + '-' + $Paths.interactive_session_binding_sha256.Substring(0, 24) + '-[0-9a-f]{32}-[0-9a-f]{32}$'
      if ($PlanHash -cnotmatch '^[0-9A-F]{64}$' -or $Epoch -cnotmatch '^[0-9a-f]{32}$' -or $JobName -cnotmatch $ExpectedJobPattern -or $FailureStage.Length -lt 1 -or (($Record['origin_session_id'] -isnot [int]) -and ($Record['origin_session_id'] -isnot [long])) -or [int]$Record['origin_session_id'] -ne $Paths.interactive_session_id) { throw "$Stage unconfirmed durable state has invalid Local interactive-session authority fields" }
      if ((($Record['bootstrap_root_pid'] -isnot [int]) -and ($Record['bootstrap_root_pid'] -isnot [long])) -or [int64]$Record['bootstrap_root_pid'] -le 0 -or (($Record['bootstrap_root_creation_filetime'] -isnot [int]) -and ($Record['bootstrap_root_creation_filetime'] -isnot [long]))) { throw "$Stage unconfirmed durable state lacks exact root identity" }
      if ($Record['process_count_state'] -isnot [string] -or [string]$Record['process_count_state'] -cnotin @('known-zero', 'known-nonzero', 'unknown')) { throw "$Stage unconfirmed durable state has an illegal process-count state" }
      if ($null -ne $Record['job_active_processes']) {
        if (($Record['job_active_processes'] -isnot [int]) -and ($Record['job_active_processes'] -isnot [long])) { throw "$Stage unconfirmed durable state has invalid active-process count" }
        if ([int64]$Record['job_active_processes'] -lt 0) { throw "$Stage unconfirmed durable state has invalid active-process count" }
      }
      foreach ($Field in @('termination_error', 'confirmation_error')) { if ($null -ne $Record[$Field] -and $Record[$Field] -isnot [string]) { throw "$Stage unconfirmed durable state has invalid $Field" } }
      if ($Record['executed_artifact'] -isnot [System.Collections.IDictionary] -or $Record['wrapper_identity'] -isnot [System.Collections.IDictionary]) { throw "$Stage unconfirmed durable state lacks captured artifact identity" }
      if ($Scope -cnotin @('assigned-job-tree', 'unassigned-suspended-bootstrap-root')) { throw "$Stage unconfirmed durable state has an illegal containment scope" }
    }
    else { throw "$Stage durable state has an illegal state value" }
    & $script:NativeCbmPrivateControls['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-after-state-read-held-capability" | Out-Null
    [pscustomobject]@{ paths = $Paths; record = $Record; record_sha256 = (& $script:NativeCbmPrivateControls['Get-NativeCbmControlBytesSha256'] -Bytes $Bytes) }
  }
  function Write-NativeCbmControlBytes {
    param([string]$Path, [byte[]]$Bytes)
    $Stream = [System.IO.FileStream]::new($Path, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::None, 4096, [System.IO.FileOptions]::WriteThrough)
    try { $Stream.Write($Bytes, 0, $Bytes.Length); $Stream.Flush($true) } finally { $Stream.Dispose() }
  }
  function Write-NativeCbmControlState {
    param([System.Collections.IDictionary]$Record, [string]$CanonicalRepositoryRoot, [object]$RepositoryLease, [string]$Stage)
    & $script:NativeCbmPrivateControls['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-before-write-held-capability"
    $Paths = & $script:NativeCbmPrivateControls['Get-NativeCbmControlStatePaths'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot
    & $script:NativeCbmPrivateControls['Assert-NativeCbmLocalSessionStateStore'] -Paths $Paths -CreateIfMissing $true -Stage "$Stage-state-store-write" | Out-Null
    foreach ($Field in @('kernel_namespace', 'principal_sid', 'machine_name', 'interactive_logon_luid', 'interactive_session_binding_sha256', 'state_store_scope')) { if (-not $Record.Contains($Field) -or $Record[$Field] -isnot [string]) { throw "$Stage durable state record lacks Local interactive-session scope field $Field" } }
    if (-not $Record.Contains('interactive_session_id') -or (($Record['interactive_session_id'] -isnot [int]) -and ($Record['interactive_session_id'] -isnot [long])) -or $Record['kernel_namespace'] -cne $Paths.kernel_namespace -or $Record['principal_sid'] -cne $Paths.principal_sid -or $Record['machine_name'] -cne $Paths.machine_name -or [int]$Record['interactive_session_id'] -ne $Paths.interactive_session_id -or $Record['interactive_logon_luid'] -cne $Paths.interactive_logon_luid -or $Record['interactive_session_binding_sha256'] -cne $Paths.interactive_session_binding_sha256 -or $Record['state_store_scope'] -cne $Paths.state_store_scope) { throw "$Stage FOREIGN_INTERACTIVE_SESSION_HARD_STOP" }
    $Record['record_id'] = [guid]::NewGuid().ToString('N'); $Record['written_utc'] = [DateTime]::UtcNow.ToString('O')
    $Bytes = ([System.Text.UTF8Encoding]::new($false, $true)).GetBytes(($Record | ConvertTo-Json -Depth 32 -Compress))
    $History = Join-Path $Paths.directory "$($Paths.root_hash).$($Record['record_id']).history.json"
    & $script:NativeCbmPrivateControls['Write-NativeCbmControlBytes'] -Path $History -Bytes $Bytes
    $Temp = Join-Path $Paths.directory "$($Paths.root_hash).$($Record['record_id']).tmp"
    & $script:NativeCbmPrivateControls['Write-NativeCbmControlBytes'] -Path $Temp -Bytes $Bytes
    if ([System.IO.File]::Exists($Paths.head_path)) { [System.IO.File]::Replace($Temp, $Paths.head_path, (Join-Path $Paths.directory "$($Paths.root_hash).$($Record['record_id']).previous.json"), $true) } else { [System.IO.File]::Move($Temp, $Paths.head_path) }
    & $script:NativeCbmPrivateControls['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-after-write-held-capability" | Out-Null
    [pscustomobject]@{ state_path = $Paths.head_path; record = $Record; state_sha256 = (& $script:NativeCbmPrivateControls['Get-NativeCbmControlBytesSha256'] -Bytes $Bytes) }
  }
  function Get-NativeCbmControlActiveProcessCount {
    param([IntPtr]$JobHandle, [string]$Stage)
    $Accounting = [AgentsCommander.Review1283.JOBOBJECT_BASIC_ACCOUNTING_INFORMATION]::new(); [uint32]$Returned = 0
    $Size = [uint32][System.Runtime.InteropServices.Marshal]::SizeOf([type][AgentsCommander.Review1283.JOBOBJECT_BASIC_ACCOUNTING_INFORMATION])
    if (-not [AgentsCommander.Review1283.NativeCbmJobInterop]::QueryInformationJobObject($JobHandle, 1, [ref]$Accounting, $Size, [ref]$Returned)) { throw "$Stage QueryInformationJobObject failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
    [int64]$Accounting.ActiveProcesses
  }
  function Get-NativeCbmPhysicalPathComponent {
    param([string]$Path, [bool]$IsDirectory, [string]$Stage)
    if ([string]::IsNullOrWhiteSpace($Path) -or -not [System.IO.Path]::IsPathFullyQualified($Path)) { throw "$Stage physical path component is not fully qualified" }
    $Handle = [AgentsCommander.Review1283.NativeCbmJobInterop]::OpenPathForPhysicalVerification($Path, $IsDirectory)
    try {
      $Attributes = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetPathFileAttributes($Handle)
      if (($Attributes -band [AgentsCommander.Review1283.NativeCbmJobInterop]::FILE_ATTRIBUTE_REPARSE_POINT) -ne 0) { throw "$Stage physical path component is a reparse point" }
      $Canonical = & $script:NativeCbmPrivateControls['ConvertTo-NativeCbmControlCanonicalPath'] -Path $Path
      $Final = & $script:NativeCbmPrivateControls['ConvertTo-NativeCbmControlCanonicalPath'] -Path ([AgentsCommander.Review1283.NativeCbmJobInterop]::GetFinalPathByHandle($Handle))
      [pscustomobject]@{ canonical_path = $Canonical; final_path = $Final; identity = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetFileIdentity($Handle); file_attributes = $Attributes }
    }
    finally { [void][AgentsCommander.Review1283.NativeCbmJobInterop]::CloseHandle($Handle) }
  }
  function Assert-NativeCbmInstalledSkillPhysicalPath {
    param([string]$TargetPath, [bool]$TargetIsDirectory, [string]$Stage)
    $P = $script:NativeCbmPrivateControls
    $Root = & $P['Get-NativeCbmPhysicalPathComponent'] -Path $script:NativeCbmTrustedInstalledSkillsRootPath -IsDirectory $true -Stage "$Stage-trusted-root"
    $TargetCanonical = & $P['ConvertTo-NativeCbmControlCanonicalPath'] -Path $TargetPath
    $RootPrefix = $Root.canonical_path.TrimEnd([char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)) + [System.IO.Path]::DirectorySeparatorChar
    if (-not $TargetCanonical.StartsWith($RootPrefix, [System.StringComparison]::Ordinal)) { throw "$Stage target escapes the trusted installed-skills root" }
    $Relative = [System.IO.Path]::GetRelativePath($Root.canonical_path, $TargetCanonical)
    if ([string]::IsNullOrWhiteSpace($Relative) -or $Relative -eq '.' -or $Relative.StartsWith('..', [System.StringComparison]::Ordinal) -or [System.IO.Path]::IsPathFullyQualified($Relative)) { throw "$Stage target has no safe relative installed-skill path" }
    $Segments = @($Relative -split '[\\/]' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($Segments.Count -eq 0) { throw "$Stage target has no installed-skill components" }
    $PhysicalPrefix = $Root.final_path.TrimEnd([char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)) + [System.IO.Path]::DirectorySeparatorChar
    $Chain = [System.Collections.Generic.List[object]]::new(); [void]$Chain.Add($Root)
    $Current = $Root.canonical_path
    for ($Index = 0; $Index -lt $Segments.Count; $Index++) {
      $Current = Join-Path $Current $Segments[$Index]
      $Component = & $P['Get-NativeCbmPhysicalPathComponent'] -Path $Current -IsDirectory (($Index -lt ($Segments.Count - 1)) -or $TargetIsDirectory) -Stage "$Stage-component-$Index"
      if (-not $Component.final_path.StartsWith($PhysicalPrefix, [System.StringComparison]::Ordinal)) { throw "$Stage physical component escapes the trusted installed-skills root" }
      [void]$Chain.Add($Component)
    }
    $Target = $Chain[$Chain.Count - 1]
    if ($Target.canonical_path -cne $TargetCanonical) { throw "$Stage physical target identity differs from its canonical path" }
    [pscustomobject]@{ trusted_root = $Root; target = $Target; chain = $Chain }
  }
  function Get-NativeCbmWrapperBindingCapability {
    param([object]$WrapperBindingCapability, [string]$Stage)
    $Binding = $null
    if ($null -eq $WrapperBindingCapability -or -not $script:NativeCbmWrapperCapabilityRegistry.TryGetValue($WrapperBindingCapability, [ref]$Binding)) { throw "$Stage wrapper capability was not issued by this rebind module" }
    if ($null -eq $Binding -or -not [object]::ReferenceEquals($Binding.issuer, $script:NativeCbmWrapperCapabilityIssuer) -or $Binding.state -cne 'held' -or $Binding.original_loader_path -cne $script:NativeCbmOriginalLoaderSkillMarkdownPath -or $Binding.trusted_root_path -cne $script:NativeCbmTrustedInstalledSkillsRootPath) { throw "$Stage wrapper capability differs from its rebind-issued loader identity" }
    return $Binding
  }
  function Open-VerifiedCodebaseMemoryWrapperReadLease {
    param([object]$WrapperBindingCapability, [string]$Stage)
    $P = $script:NativeCbmPrivateControls
    $Binding = & $P['Get-NativeCbmWrapperBindingCapability'] -WrapperBindingCapability $WrapperBindingCapability -Stage "$Stage-capability"
    $LoaderPhysical = & $P['Assert-NativeCbmInstalledSkillPhysicalPath'] -TargetPath $Binding.loader_skill_markdown_path -TargetIsDirectory $false -Stage "$Stage-loader-physical"
    $WrapperPhysical = & $P['Assert-NativeCbmInstalledSkillPhysicalPath'] -TargetPath $Binding.wrapper_path -TargetIsDirectory $false -Stage "$Stage-wrapper-physical"
    if ($LoaderPhysical.trusted_root.final_path -cne $Binding.trusted_root_final_path -or $LoaderPhysical.trusted_root.identity.VolumeSerialNumber -ne $Binding.trusted_root_identity.VolumeSerialNumber -or $LoaderPhysical.trusted_root.identity.FileIndex -ne $Binding.trusted_root_identity.FileIndex -or $LoaderPhysical.target.identity.VolumeSerialNumber -ne $Binding.loader_skill_file_identity.VolumeSerialNumber -or $LoaderPhysical.target.identity.FileIndex -ne $Binding.loader_skill_file_identity.FileIndex -or $WrapperPhysical.target.final_path -cne $Binding.wrapper_final_path -or $WrapperPhysical.target.identity.VolumeSerialNumber -ne $Binding.wrapper_file_identity.VolumeSerialNumber -or $WrapperPhysical.target.identity.FileIndex -ne $Binding.wrapper_file_identity.FileIndex) { throw "$Stage wrapper capability physical chain differs from its rebind identity" }
    $Path = [string]$Binding.wrapper_path
    $Stream = [System.IO.FileStream]::new($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read, 4096, [System.IO.FileOptions]::SequentialScan)
    try {
      $Hash = & $script:NativeCbmPrivateControls['Get-NativeCbmSha256FromStream'] -Stream $Stream
      if ($Hash -cne [string]$Binding.wrapper_sha256) { throw "$Stage loader wrapper hash differs" }
      $Identity = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetFileIdentity($Stream.SafeFileHandle.DangerousGetHandle())
      if ($Identity.VolumeSerialNumber -ne $Binding.wrapper_file_identity.VolumeSerialNumber -or $Identity.FileIndex -ne $Binding.wrapper_file_identity.FileIndex) { throw "$Stage loader wrapper identity differs" }
      [pscustomobject]@{ capability = $WrapperBindingCapability; path = $Path; stream = $Stream; sha256 = $Hash; identity = $Identity }
    }
    catch { $Stream.Dispose(); throw }
  }
  function Close-VerifiedCodebaseMemoryWrapperReadLease {
    param([psobject]$WrapperReadLease, [string]$Stage)
    try {
      $P = $script:NativeCbmPrivateControls; $Binding = & $P['Get-NativeCbmWrapperBindingCapability'] -WrapperBindingCapability $WrapperReadLease.capability -Stage "$Stage-capability"
      $WrapperPhysical = & $P['Assert-NativeCbmInstalledSkillPhysicalPath'] -TargetPath $Binding.wrapper_path -TargetIsDirectory $false -Stage "$Stage-wrapper-physical"
      if ($WrapperPhysical.target.final_path -cne $Binding.wrapper_final_path -or $WrapperPhysical.target.identity.VolumeSerialNumber -ne $Binding.wrapper_file_identity.VolumeSerialNumber -or $WrapperPhysical.target.identity.FileIndex -ne $Binding.wrapper_file_identity.FileIndex) { throw "$Stage wrapper physical chain changed through the held lease" }
      $After = & $script:NativeCbmPrivateControls['Get-NativeCbmSha256FromStream'] -Stream $WrapperReadLease.stream
      $AfterIdentity = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetFileIdentity($WrapperReadLease.stream.SafeFileHandle.DangerousGetHandle())
      if ($After -cne $WrapperReadLease.sha256 -or $AfterIdentity.VolumeSerialNumber -ne $WrapperReadLease.identity.VolumeSerialNumber -or $AfterIdentity.FileIndex -ne $WrapperReadLease.identity.FileIndex) { throw "$Stage loader wrapper changed through its held handle" }
      [pscustomobject]@{ path = $WrapperReadLease.path; sha256 = $After; identity = $AfterIdentity }
    }
    finally { $WrapperReadLease.stream.Dispose() }
  }
  function New-NativeCbmControlJob {
    param([string]$JobName, [string]$Stage)
    $ExpectedJobPattern = '^Local\\AgentsCommander-1283-cbm-[0-9A-F]{64}-' + $script:NativeCbmPrincipalSidHash.Substring(0, 16) + '-' + [regex]::Escape($script:NativeCbmInteractiveSessionId.ToString([System.Globalization.CultureInfo]::InvariantCulture)) + '-' + $script:NativeCbmInteractiveSessionBindingSha256.Substring(0, 24) + '-[0-9a-f]{32}-[0-9a-f]{32}$'
    if ($script:NativeCbmKernelObjectNamespace -cne 'Local' -or $JobName -cnotmatch $ExpectedJobPattern) { throw "$Stage capture Job name is not in the exact Local interactive-session namespace" }
    $Sddl = "D:P(A;;GA;;;$($script:NativeCbmPrincipalSid))"
    [int]$CreateError = 0
    $Job = [AgentsCommander.Review1283.NativeCbmJobInterop]::CreateJobObjectWithSddl($JobName, $Sddl, [ref]$CreateError)
    if ($Job -eq [IntPtr]::Zero) { throw "$Stage Local CreateJobObject failed: $CreateError" }
    if ($CreateError -ne 0) {
      [void][AgentsCommander.Review1283.NativeCbmJobInterop]::CloseHandle($Job)
      if ($CreateError -eq [AgentsCommander.Review1283.NativeCbmJobInterop]::ERROR_ALREADY_EXISTS) { throw "$Stage Local capture Job name already exists" }
      throw "$Stage Local capture Job creation state is ambiguous: $CreateError"
    }
    try {
      $Probe = [AgentsCommander.Review1283.NativeCbmJobInterop]::OpenJobObject([AgentsCommander.Review1283.NativeCbmJobInterop]::JOB_OBJECT_QUERY, $false, $JobName)
      if ($Probe -eq [IntPtr]::Zero) { throw "$Stage cannot reopen the Local capture Job with query access: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
      [void][AgentsCommander.Review1283.NativeCbmJobInterop]::CloseHandle($Probe)
      $Limits = [AgentsCommander.Review1283.JOBOBJECT_EXTENDED_LIMIT_INFORMATION]::new()
      $Limits.BasicLimitInformation.LimitFlags = [AgentsCommander.Review1283.NativeCbmJobInterop]::JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
      $Size = [System.Runtime.InteropServices.Marshal]::SizeOf($Limits); $Pointer = [System.Runtime.InteropServices.Marshal]::AllocHGlobal($Size)
      try {
        [System.Runtime.InteropServices.Marshal]::StructureToPtr($Limits, $Pointer, $false)
        if (-not [AgentsCommander.Review1283.NativeCbmJobInterop]::SetInformationJobObject($Job, 9, $Pointer, [uint32]$Size)) { throw "$Stage SetInformationJobObject failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
      } finally { [System.Runtime.InteropServices.Marshal]::FreeHGlobal($Pointer) }
      $Job
    } catch { [void][AgentsCommander.Review1283.NativeCbmJobInterop]::CloseHandle($Job); throw }
  }
  function Stop-NativeCbmControlJobAndConfirm {
    param([IntPtr]$JobHandle, [int]$TerminationWaitMilliseconds, [string]$Stage)
    [Nullable[int64]]$Count = $null; $State = 'unknown'; $TerminationError = $null; $ConfirmationError = $null
    try { if (-not [AgentsCommander.Review1283.NativeCbmJobInterop]::TerminateJobObject($JobHandle, 1)) { $TerminationError = "TerminateJobObject failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" } } catch { $TerminationError = $_.Exception.Message }
    try { $Deadline = [DateTime]::UtcNow.AddMilliseconds($TerminationWaitMilliseconds) } catch { $Deadline = [DateTime]::UtcNow; $ConfirmationError = $_.Exception.Message }
    while ($null -eq $ConfirmationError -and [DateTime]::UtcNow -lt $Deadline) {
      try {
        $Count = & $script:NativeCbmPrivateControls['Get-NativeCbmControlActiveProcessCount'] -JobHandle $JobHandle -Stage "$Stage-query"
        $State = if ($Count -eq 0) { 'known-zero' } else { 'known-nonzero' }
        if ($null -eq $TerminationError -and $Count -eq 0) { return [pscustomobject]@{ tree_termination_confirmed = $true; job_active_processes = [int64]0; process_count_state = 'known-zero'; termination_error = $null; confirmation_error = $null } }
      } catch { $ConfirmationError = $_.Exception.Message; break }
      try { Start-Sleep -Milliseconds 25 } catch { $ConfirmationError = $_.Exception.Message; break }
    }
    [pscustomobject]@{ tree_termination_confirmed = $false; job_active_processes = $Count; process_count_state = $State; termination_error = $TerminationError; confirmation_error = $ConfirmationError }
  }
  function New-NativeCbmBootstrapReadLease {
    param([object]$RepositoryLease, [string]$CanonicalRepositoryRoot, [string]$Stage)
    & $script:NativeCbmPrivateControls['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-before-bootstrap"
    $Directory = Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)) 'AgentsCommander/review-state/1283/native-cbm/bootstrap'
    [System.IO.Directory]::CreateDirectory($Directory) | Out-Null
    $Path = Join-Path $Directory ("bootstrap-" + [guid]::NewGuid().ToString('N') + '.ps1')
    $Source = @'
param()
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$Input = [Console]::OpenStandardInput()
try {
  $Memory = [System.IO.MemoryStream]::new(); $Input.CopyTo($Memory)
  $Payload = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($Memory.ToArray()) | ConvertFrom-Json -AsHashtable -Depth 16
  foreach ($Field in @('token', 'artifact_path', 'artifact_sha256', 'artifact_byte_length', 'artifact_file_identity', 'operation_arguments')) { if (-not $Payload.Contains($Field)) { exit 97 } }
  if ($null -eq ('AgentsCommander.Review1283.NativeCbmBootstrapFileInterop' -as [type])) {
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
namespace AgentsCommander.Review1283 {
  [StructLayout(LayoutKind.Sequential)] public struct NativeCbmBootstrapFileTime { public uint Low; public uint High; }
  [StructLayout(LayoutKind.Sequential)] public struct NativeCbmBootstrapFileInfo {
    public uint Attributes; public NativeCbmBootstrapFileTime Creation; public NativeCbmBootstrapFileTime Access; public NativeCbmBootstrapFileTime Write;
    public uint VolumeSerialNumber; public uint SizeHigh; public uint SizeLow; public uint Links; public uint FileIndexHigh; public uint FileIndexLow;
  }
  public static class NativeCbmBootstrapFileInterop {
    [DllImport("kernel32.dll", SetLastError=true)] static extern bool GetFileInformationByHandle(IntPtr handle, out NativeCbmBootstrapFileInfo info);
    public static string GetFileIdentity(IntPtr handle) {
      NativeCbmBootstrapFileInfo info;
      if (!GetFileInformationByHandle(handle, out info)) throw new InvalidOperationException("GetFileInformationByHandle failed: " + Marshal.GetLastWin32Error());
      return info.VolumeSerialNumber.ToString() + ":" + (((ulong)info.FileIndexHigh << 32) | info.FileIndexLow).ToString();
    }
  }
}
"@
  }
  $Artifact = [System.IO.FileStream]::new([string]$Payload['artifact_path'], [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
  try {
    $Hasher = [System.Security.Cryptography.SHA256]::Create(); $Hash = [Convert]::ToHexString($Hasher.ComputeHash($Artifact)); $Hasher.Dispose()
    $ExpectedIdentity = ('{0}:{1}' -f [uint32]$Payload['artifact_file_identity']['VolumeSerialNumber'], [uint64]$Payload['artifact_file_identity']['FileIndex'])
    $ActualIdentity = [AgentsCommander.Review1283.NativeCbmBootstrapFileInterop]::GetFileIdentity($Artifact.SafeFileHandle.DangerousGetHandle())
    if ($Hash -cne [string]$Payload['artifact_sha256'] -or $Artifact.Length -ne [int64]$Payload['artifact_byte_length'] -or $ActualIdentity -cne $ExpectedIdentity) { exit 98 }
    & ([string]$Payload['artifact_path']) @([string[]]$Payload['operation_arguments'])
    exit $LASTEXITCODE
  } finally { $Artifact.Dispose() }
} finally { $Input.Dispose() }
'@
    $Bytes = ([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($Source)
    $Stream = [System.IO.FileStream]::new($Path, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::Read, 4096, [System.IO.FileOptions]::WriteThrough)
    try {
      $Stream.Write($Bytes, 0, $Bytes.Length); $Stream.Flush($true)
      $Hash = & $script:NativeCbmPrivateControls['Get-NativeCbmSha256FromStream'] -Stream $Stream
      $Identity = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetFileIdentity($Stream.SafeFileHandle.DangerousGetHandle())
      [pscustomobject]@{ path = $Path; stream = $Stream; sha256 = $Hash; byte_length = [int64]$Stream.Length; identity = $Identity }
    }
    catch {
      $Stream.Dispose()
      try { [System.IO.File]::Delete($Path) } catch { }
      throw
    }
  }
  function Remove-NativeCbmBootstrapReadLease {
    param([psobject]$BootstrapReadLease, [string]$Stage)
    $BootstrapReadLease.stream.Dispose(); [System.IO.File]::Delete([string]$BootstrapReadLease.path)
  }
  function Assert-NativeCbmBootstrapReadLease {
    param([psobject]$BootstrapReadLease, [string]$Stage)
    $Identity = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetFileIdentity($BootstrapReadLease.stream.SafeFileHandle.DangerousGetHandle())
    $Hash = & $script:NativeCbmPrivateControls['Get-NativeCbmSha256FromStream'] -Stream $BootstrapReadLease.stream
    if ($Hash -cne $BootstrapReadLease.sha256 -or [int64]$BootstrapReadLease.stream.Length -ne [int64]$BootstrapReadLease.byte_length -or $Identity.VolumeSerialNumber -ne $BootstrapReadLease.identity.VolumeSerialNumber -or $Identity.FileIndex -ne $BootstrapReadLease.identity.FileIndex) { throw "$Stage bootstrap artifact identity differs" }
    [pscustomobject]@{ path = $BootstrapReadLease.path; sha256 = $Hash; byte_length = $BootstrapReadLease.byte_length; identity = $Identity }
  }
  function New-NativeCbmVerifiedExecutionArtifact {
    param([psobject]$WrapperReadLease, [string]$CanonicalRepositoryRoot, [string]$EvidenceEpoch, [string]$Stage)
    $RootHash = & $script:NativeCbmPrivateControls['Get-NativeCbmControlRootHash'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot
    $Directory = Join-Path ([Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)) "AgentsCommander/review-state/1283/native-cbm/executables/$RootHash/$EvidenceEpoch"
    [System.IO.Directory]::CreateDirectory($Directory) | Out-Null
    $Path = Join-Path $Directory ("capture-" + [guid]::NewGuid().ToString('N') + "." + $WrapperReadLease.sha256 + '.ps1')
    $Stream = [System.IO.FileStream]::new($Path, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::Read, 4096, [System.IO.FileOptions]::WriteThrough)
    try {
      $WrapperReadLease.stream.Position = 0; $WrapperReadLease.stream.CopyTo($Stream); $Stream.Flush($true)
      $Hash = & $script:NativeCbmPrivateControls['Get-NativeCbmSha256FromStream'] -Stream $Stream
      if ($Hash -cne $WrapperReadLease.sha256) { throw "$Stage execution artifact bytes differ from verified wrapper lease" }
      [pscustomobject]@{ path = $Path; stream = $Stream; sha256 = $Hash; byte_length = [int64]$Stream.Length; identity = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetFileIdentity($Stream.SafeFileHandle.DangerousGetHandle()) }
    } catch { $Stream.Dispose(); throw }
  }
  function Assert-NativeCbmVerifiedExecutionArtifact {
    param([psobject]$Artifact, [string]$Stage)
    $Identity = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetFileIdentity($Artifact.stream.SafeFileHandle.DangerousGetHandle())
    $Hash = & $script:NativeCbmPrivateControls['Get-NativeCbmSha256FromStream'] -Stream $Artifact.stream
    if ($Hash -cne $Artifact.sha256 -or [int64]$Artifact.stream.Length -ne [int64]$Artifact.byte_length -or $Identity.VolumeSerialNumber -ne $Artifact.identity.VolumeSerialNumber -or $Identity.FileIndex -ne $Artifact.identity.FileIndex) { throw "$Stage execution artifact identity differs" }
    [pscustomobject]@{ path = $Artifact.path; sha256 = $Hash; byte_length = $Artifact.byte_length; identity = $Identity }
  }
  function Remove-NativeCbmVerifiedExecutionArtifact {
    param([psobject]$Artifact, [string]$Stage)
    $Artifact.stream.Dispose(); [System.IO.File]::Delete([string]$Artifact.path)
  }
  function Start-NativeCbmBootstrapSuspended {
    param([psobject]$BootstrapReadLease, [string]$CanonicalRepositoryRoot, [string]$Stage)
    & $script:NativeCbmPrivateControls['Assert-NativeCbmBootstrapReadLease'] -BootstrapReadLease $BootstrapReadLease -Stage "$Stage-before-launch" | Out-Null
    $Input = [System.IO.Pipes.AnonymousPipeServerStream]::new([System.IO.Pipes.PipeDirection]::Out, [System.IO.HandleInheritability]::Inheritable)
    $Output = [System.IO.Pipes.AnonymousPipeServerStream]::new([System.IO.Pipes.PipeDirection]::In, [System.IO.HandleInheritability]::Inheritable)
    $Error = [System.IO.Pipes.AnonymousPipeServerStream]::new([System.IO.Pipes.PipeDirection]::In, [System.IO.HandleInheritability]::Inheritable)
    $Process = [AgentsCommander.Review1283.PROCESS_INFORMATION]::new()
    try {
      $HostPath = [System.Diagnostics.Process]::GetCurrentProcess().MainModule.FileName
      if ([System.IO.Path]::GetFileName($HostPath) -inotmatch '^(powershell|pwsh)\.exe$') { throw "$Stage current host is not PowerShell" }
      $Startup = [AgentsCommander.Review1283.STARTUPINFOW]::new()
      $Startup.cb = [System.Runtime.InteropServices.Marshal]::SizeOf([type][AgentsCommander.Review1283.STARTUPINFOW])
      $Startup.dwFlags = [AgentsCommander.Review1283.NativeCbmJobInterop]::STARTF_USESTDHANDLES
      $Startup.hStdInput = [IntPtr]([int64]$Input.GetClientHandleAsString())
      $Startup.hStdOutput = [IntPtr]([int64]$Output.GetClientHandleAsString())
      $Startup.hStdError = [IntPtr]([int64]$Error.GetClientHandleAsString())
      $Arguments = @('-NoLogo', '-NoProfile', '-NonInteractive', '-File', [string]$BootstrapReadLease.path)
      $CommandLine = [AgentsCommander.Review1283.NativeCbmJobInterop]::BuildCommandLine($HostPath, $Arguments)
      if (-not [AgentsCommander.Review1283.NativeCbmJobInterop]::CreateProcessW($HostPath, $CommandLine, [IntPtr]::Zero, [IntPtr]::Zero, $true, ([AgentsCommander.Review1283.NativeCbmJobInterop]::CREATE_SUSPENDED -bor [AgentsCommander.Review1283.NativeCbmJobInterop]::CREATE_NO_WINDOW), [IntPtr]::Zero, $CanonicalRepositoryRoot, [ref]$Startup, [ref]$Process)) { throw "$Stage CreateProcessW failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
    } catch { $Input.Dispose(); $Output.Dispose(); $Error.Dispose(); throw }
    $PostCreateError = $null; $CreationFileTime = $null
    try {
      $Input.DisposeLocalCopyOfClientHandle(); $Output.DisposeLocalCopyOfClientHandle(); $Error.DisposeLocalCopyOfClientHandle()
      $CreationFileTime = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetProcessCreationFileTime($Process.hProcess)
    } catch { $PostCreateError = $_.Exception.Message }
    [pscustomobject]@{ process_handle = $Process.hProcess; thread_handle = $Process.hThread; process_id = $Process.dwProcessId; creation_filetime = $CreationFileTime; stdin = $Input; stdout = $Output; stderr = $Error; post_create_error = $PostCreateError }
  }
  function Stop-NativeCbmUnassignedBootstrapAndConfirm {
    param([psobject]$Started, [int]$TerminationWaitMilliseconds, [string]$Stage)
    $Error = $null; $ExitCode = $null
    try { if (-not [AgentsCommander.Review1283.NativeCbmJobInterop]::TerminateProcess($Started.process_handle, 1)) { $Error = "TerminateProcess failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" } } catch { $Error = $_.Exception.Message }
    try { $Wait = [AgentsCommander.Review1283.NativeCbmJobInterop]::WaitForSingleObject($Started.process_handle, [uint32]$TerminationWaitMilliseconds); if ($Wait -eq [AgentsCommander.Review1283.NativeCbmJobInterop]::WAIT_OBJECT_0 -and [AgentsCommander.Review1283.NativeCbmJobInterop]::GetExitCodeProcess($Started.process_handle, [ref]$ExitCode) -and $null -eq $Error) { return [pscustomobject]@{ root_termination_confirmed = $true; termination_error = $null; confirmation_error = $null; exit_code = $ExitCode; process_creation_filetime = $Started.creation_filetime } }; if ($Wait -ne [AgentsCommander.Review1283.NativeCbmJobInterop]::WAIT_OBJECT_0) { $Error = "direct root wait failed: $Wait" } } catch { $Error = $_.Exception.Message }
    [pscustomobject]@{ root_termination_confirmed = $false; termination_error = $Error; confirmation_error = $Error; exit_code = $ExitCode; process_creation_filetime = $Started.creation_filetime }
  }
  function Wait-NativeCbmCaptureCompletion {
    param([psobject]$Started, [System.Threading.Tasks.Task]$OutputTask, [System.Threading.Tasks.Task]$ErrorTask, [DateTime]$CaptureDeadlineUtc, [System.Threading.CancellationTokenSource]$Cancellation, [string]$Stage)
    while ([DateTime]::UtcNow -lt $CaptureDeadlineUtc) {
      if ($OutputTask.IsFaulted -or $ErrorTask.IsFaulted -or $OutputTask.IsCanceled -or $ErrorTask.IsCanceled) { $Cancellation.Cancel(); throw "$Stage native reader fault or cancellation" }
      $RootWait = [AgentsCommander.Review1283.NativeCbmJobInterop]::WaitForSingleObject($Started.process_handle, 0)
      if ($RootWait -eq [AgentsCommander.Review1283.NativeCbmJobInterop]::WAIT_FAILED) { $Cancellation.Cancel(); throw "$Stage root wait failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
      if ($RootWait -eq [AgentsCommander.Review1283.NativeCbmJobInterop]::WAIT_OBJECT_0 -and $OutputTask.Status -eq [System.Threading.Tasks.TaskStatus]::RanToCompletion -and $ErrorTask.Status -eq [System.Threading.Tasks.TaskStatus]::RanToCompletion) {
        [uint32]$ExitCode = 0
        if (-not [AgentsCommander.Review1283.NativeCbmJobInterop]::GetExitCodeProcess($Started.process_handle, [ref]$ExitCode)) { throw "$Stage GetExitCodeProcess failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
        return [pscustomobject]@{ exit_code = $ExitCode; standard_output_bytes = [byte[]]$OutputTask.Result; standard_error_bytes = [byte[]]$ErrorTask.Result; root_exit_observed = $true; readers_closed = $true }
      }
      Start-Sleep -Milliseconds 10
    }
    $Cancellation.Cancel(); throw "$Stage complete native capture deadline expired"
  }
  function Write-NativeCbmCapturePayload {
    param([psobject]$Started, [byte[]]$PayloadBytes, [int]$MaximumPayloadBytes, [DateTime]$CaptureDeadlineUtc, [System.Threading.CancellationTokenSource]$Cancellation, [string]$Stage)
    if ($null -eq $PayloadBytes -or $MaximumPayloadBytes -lt 1 -or $PayloadBytes.Length -gt $MaximumPayloadBytes) { throw "$Stage stdin payload exceeds its bounded contract" }
    if ([DateTime]::UtcNow -ge $CaptureDeadlineUtc) { $Cancellation.Cancel(); throw "$Stage deadline expired before bounded stdin write" }
    $WriteTask = [AgentsCommander.Review1283.NativeCbmJobInterop]::WriteBoundedAsync($Started.stdin, $PayloadBytes, $MaximumPayloadBytes, $Cancellation.Token)
    while (-not $WriteTask.IsCompleted) {
      if ([DateTime]::UtcNow -ge $CaptureDeadlineUtc) { $Cancellation.Cancel(); throw "$Stage deadline expired during bounded stdin write" }
      if ($WriteTask.IsFaulted -or $WriteTask.IsCanceled) { $Cancellation.Cancel(); throw "$Stage bounded stdin write faulted or cancelled" }
      Start-Sleep -Milliseconds 10
    }
    if ($WriteTask.IsFaulted -or $WriteTask.IsCanceled -or $WriteTask.Status -ne [System.Threading.Tasks.TaskStatus]::RanToCompletion) { $Cancellation.Cancel(); throw "$Stage bounded stdin write did not complete" }
    if ([DateTime]::UtcNow -ge $CaptureDeadlineUtc) { $Cancellation.Cancel(); throw "$Stage deadline expired after bounded stdin write" }
    $Started.stdin.Dispose()
    [pscustomobject]@{ payload_byte_length = $PayloadBytes.Length; payload_write_completed = $true }
  }
  function Persist-NativeCbmUnconfirmedTerminationAndRetain {
    param([Exception]$CaptureFailure, [psobject]$Termination, [string]$ContainmentScope, [IntPtr]$JobHandle, [psobject]$Started, [psobject]$Artifact, [psobject]$BootstrapReadLease, [psobject]$WrapperReadLease, [string]$JobName, [string]$CanonicalRepositoryRoot, [string]$CanonicalPlanPath, [string]$ExpectedPlanSha256, [string]$EvidenceEpoch, [object]$RepositoryLease, [string]$Stage)
    $ExpectedJobPattern = '^Local\\AgentsCommander-1283-cbm-[0-9A-F]{64}-' + $script:NativeCbmPrincipalSidHash.Substring(0, 16) + '-' + [regex]::Escape($script:NativeCbmInteractiveSessionId.ToString([System.Globalization.CultureInfo]::InvariantCulture)) + '-' + $script:NativeCbmInteractiveSessionBindingSha256.Substring(0, 24) + '-[0-9a-f]{32}-[0-9a-f]{32}$'
    if ($JobName -cnotmatch $ExpectedJobPattern) { throw "$Stage cannot persist a foreign or non-Local capture Job" }
    $Record = [ordered]@{ schema_version = $script:NativeCbmEvidenceControlSchemaVersion; state = 'unconfirmed-tree-termination'; kernel_namespace = $script:NativeCbmKernelObjectNamespace; principal_sid = $script:NativeCbmPrincipalSid; machine_name = $script:NativeCbmMachineName; interactive_session_id = $script:NativeCbmInteractiveSessionId; interactive_logon_luid = $script:NativeCbmInteractiveLogonLuid; interactive_session_binding_sha256 = $script:NativeCbmInteractiveSessionBindingSha256; state_store_scope = $script:NativeCbmStateStoreScope; origin_session_id = $script:NativeCbmInteractiveSessionId; containment_scope = $ContainmentScope; canonical_repository_root = $CanonicalRepositoryRoot; canonical_plan_path = $CanonicalPlanPath; root_hash = (& $script:NativeCbmPrivateControls['Get-NativeCbmControlRootHash'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot); draft_plan_sha256 = $ExpectedPlanSha256; evidence_epoch = $EvidenceEpoch; stage = $Stage; job_name = $JobName; bootstrap_root_pid = if ($null -eq $Started) { $null } else { $Started.process_id }; bootstrap_root_creation_filetime = if ($null -eq $Started) { $null } else { $Started.creation_filetime }; process_count_state = if ($null -eq $Termination.PSObject.Properties['process_count_state']) { 'unknown' } else { $Termination.process_count_state }; job_active_processes = if ($null -eq $Termination.PSObject.Properties['job_active_processes']) { $null } else { $Termination.job_active_processes }; termination_error = $Termination.termination_error; confirmation_error = $Termination.confirmation_error; executed_artifact = $Artifact; wrapper_identity = $WrapperReadLease }
    try { [void](& $script:NativeCbmPrivateControls['Write-NativeCbmControlState'] -Record $Record -CanonicalRepositoryRoot $CanonicalRepositoryRoot -RepositoryLease $RepositoryLease -Stage "$Stage-hard-stop-write") }
    catch { $script:NativeCbmUnconfirmedJobs.Add([pscustomobject]@{ job_handle = $JobHandle; started = $Started; artifact = $Artifact; bootstrap = $BootstrapReadLease; wrapper = $WrapperReadLease; persistence_error = $_.Exception.Message }); throw [AggregateException]::new("$Stage durable hard-stop persistence failed", [Exception[]]@($CaptureFailure, $_.Exception)) }
    $script:NativeCbmUnconfirmedJobs.Add([pscustomobject]@{ job_handle = $JobHandle; started = $Started; artifact = $Artifact; bootstrap = $BootstrapReadLease; wrapper = $WrapperReadLease; persistence_error = $null })
  }
  function Resolve-InstalledCodebaseMemoryWrapperBinding {
    param([string]$Stage)
    $P = $script:NativeCbmPrivateControls
    if ($null -ne $script:NativeCbmRebindWrapperCapability) { throw "$Stage wrapper capability was already issued for this rebind module" }
    $LoaderFullPath = [System.IO.Path]::GetFullPath($script:NativeCbmOriginalLoaderSkillMarkdownPath)
    if ([System.IO.Path]::GetFileName($LoaderFullPath) -cne 'SKILL.md') { throw "$Stage loader path leaf is not exactly SKILL.md" }
    $LoaderPhysical = & $P['Assert-NativeCbmInstalledSkillPhysicalPath'] -TargetPath $LoaderFullPath -TargetIsDirectory $false -Stage "$Stage-loader-skill"
    $Skill = $LoaderPhysical.target.canonical_path
    $Directory = & $P['ConvertTo-NativeCbmControlCanonicalPath'] -Path (Split-Path -Parent $Skill)
    $SkillPrefix = $Directory.TrimEnd([char[]]@([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar)) + [System.IO.Path]::DirectorySeparatorChar
    $SkillDirectoryPhysical = & $P['Assert-NativeCbmInstalledSkillPhysicalPath'] -TargetPath $Directory -TargetIsDirectory $true -Stage "$Stage-skill-directory"
    $ScriptsDirectory = & $P['ConvertTo-NativeCbmControlCanonicalPath'] -Path (Join-Path $Directory 'scripts')
    if (-not $ScriptsDirectory.StartsWith($SkillPrefix, [System.StringComparison]::Ordinal) -or [System.IO.Path]::GetRelativePath($Directory, $ScriptsDirectory) -cne 'SCRIPTS') { throw "$Stage scripts directory escapes the loader skill directory" }
    $ScriptsPhysical = & $P['Assert-NativeCbmInstalledSkillPhysicalPath'] -TargetPath $ScriptsDirectory -TargetIsDirectory $true -Stage "$Stage-scripts-directory"
    $ExpectedWrapper = & $P['ConvertTo-NativeCbmControlCanonicalPath'] -Path (Join-Path $ScriptsDirectory 'cbm.ps1')
    if (-not $ExpectedWrapper.StartsWith($SkillPrefix, [System.StringComparison]::Ordinal) -or [System.IO.Path]::GetRelativePath($Directory, $ExpectedWrapper) -cne 'SCRIPTS\CBM.PS1' -or [System.IO.Path]::GetFileName($ExpectedWrapper) -cne 'CBM.PS1') { throw "$Stage expected wrapper leaf or skill-directory prefix is invalid" }
    $WrapperPhysical = & $P['Assert-NativeCbmInstalledSkillPhysicalPath'] -TargetPath $ExpectedWrapper -TargetIsDirectory $false -Stage "$Stage-wrapper-leaf"
    $Wrapper = $WrapperPhysical.target.canonical_path
    if ($Wrapper -cne $ExpectedWrapper -or -not $Wrapper.StartsWith($SkillPrefix, [System.StringComparison]::Ordinal) -or [System.IO.Path]::GetRelativePath($Directory, $Wrapper) -cne 'SCRIPTS\CBM.PS1' -or $SkillDirectoryPhysical.target.final_path -notlike ($LoaderPhysical.trusted_root.final_path + '*') -or $ScriptsPhysical.target.final_path -notlike ($LoaderPhysical.trusted_root.final_path + '*')) { throw "$Stage resolved wrapper or ancestor escapes the trusted physical installed-skill root" }
    $Stream = [System.IO.FileStream]::new($Wrapper, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    try {
      $Identity = [AgentsCommander.Review1283.NativeCbmJobInterop]::GetFileIdentity($Stream.SafeFileHandle.DangerousGetHandle())
      if ($Identity.VolumeSerialNumber -ne $WrapperPhysical.target.identity.VolumeSerialNumber -or $Identity.FileIndex -ne $WrapperPhysical.target.identity.FileIndex) { throw "$Stage wrapper stream identity differs from the physical chain" }
      $Capability = [object]::new()
      $Binding = [pscustomobject]@{ issuer = $script:NativeCbmWrapperCapabilityIssuer; state = 'held'; original_loader_path = $script:NativeCbmOriginalLoaderSkillMarkdownPath; trusted_root_path = $script:NativeCbmTrustedInstalledSkillsRootPath; trusted_root_final_path = $LoaderPhysical.trusted_root.final_path; trusted_root_identity = $LoaderPhysical.trusted_root.identity; loader_skill_markdown_path = $Skill; loader_skill_file_identity = $LoaderPhysical.target.identity; skill_dir = $Directory; scripts_dir = $ScriptsDirectory; wrapper_path = $Wrapper; wrapper_final_path = $WrapperPhysical.target.final_path; wrapper_sha256 = (& $P['Get-NativeCbmSha256FromStream'] -Stream $Stream); wrapper_file_identity = $Identity }
      $script:NativeCbmWrapperCapabilityRegistry.Add($Capability, $Binding)
      $script:NativeCbmRebindWrapperCapability = $Capability
      return $Capability
    }
    finally { $Stream.Dispose() }
  }
  function Confirm-NativeCbmPersistentHardStopCleared {
    param([string]$CanonicalRepositoryRoot, [string]$CanonicalPlanPath, [string]$ExpectedPlanSha256, [string]$EvidenceEpoch, [object]$RepositoryLease, [string]$Stage)
    $P = $script:NativeCbmPrivateControls; & $P['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-lease"
    if ($ExpectedPlanSha256 -cnotmatch '^[0-9A-F]{64}$' -or $EvidenceEpoch -cnotmatch '^[0-9a-f]{32}$') { throw "$Stage clearance authority is malformed" }
    $Prior = & $P['Read-NativeCbmControlState'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot -CanonicalPlanPath $CanonicalPlanPath -RepositoryLease $RepositoryLease -Stage $Stage
    if ($null -ne $Prior -and $Prior.record['state'] -ceq 'unconfirmed-tree-termination') {
      if ($Prior.record['containment_scope'] -ceq 'assigned-job-tree') {
        $PriorJobName = [string]$Prior.record['job_name']
        $ExpectedJobPattern = '^Local\\AgentsCommander-1283-cbm-[0-9A-F]{64}-' + $script:NativeCbmPrincipalSidHash.Substring(0, 16) + '-' + [regex]::Escape($script:NativeCbmInteractiveSessionId.ToString([System.Globalization.CultureInfo]::InvariantCulture)) + '-' + $script:NativeCbmInteractiveSessionBindingSha256.Substring(0, 24) + '-[0-9a-f]{32}-[0-9a-f]{32}$'
        if ($Prior.record['kernel_namespace'] -cne 'Local' -or $Prior.record['principal_sid'] -cne $script:NativeCbmPrincipalSid -or $Prior.record['machine_name'] -cne $script:NativeCbmMachineName -or $Prior.record['interactive_session_id'] -ne $script:NativeCbmInteractiveSessionId -or $Prior.record['interactive_logon_luid'] -cne $script:NativeCbmInteractiveLogonLuid -or $Prior.record['interactive_session_binding_sha256'] -cne $script:NativeCbmInteractiveSessionBindingSha256 -or $PriorJobName -cnotmatch $ExpectedJobPattern) { throw "$Stage FOREIGN_INTERACTIVE_SESSION_HARD_STOP" }
        $Job = [AgentsCommander.Review1283.NativeCbmJobInterop]::OpenJobObject([AgentsCommander.Review1283.NativeCbmJobInterop]::JOB_OBJECT_QUERY, $false, $PriorJobName)
        if ($Job -ne [IntPtr]::Zero) { try { if ((& $P['Get-NativeCbmControlActiveProcessCount'] -JobHandle $Job -Stage "$Stage-prior-job") -ne 0) { throw "$Stage prior Job remains active" } } finally { [void][AgentsCommander.Review1283.NativeCbmJobInterop]::CloseHandle($Job) } }
        else {
          $OpenError = [System.Runtime.InteropServices.Marshal]::GetLastWin32Error()
          if ($OpenError -eq [AgentsCommander.Review1283.NativeCbmJobInterop]::ERROR_ACCESS_DENIED) { throw "$Stage lacks access to the prior Local Job; hard-stop absence is ambiguous" }
          if ($OpenError -ne [AgentsCommander.Review1283.NativeCbmJobInterop]::ERROR_FILE_NOT_FOUND) { throw "$Stage cannot inspect prior Local Job" }
          # The record has already matched this held Local SID/session binding. KILL_ON_JOB_CLOSE means disappearance of this exact Local Job after its owned handle closes cannot leave a member of that capture running; this exact-scope absence is the only permitted disappeared-Job proof.
        }
      } elseif ($Prior.record['containment_scope'] -ceq 'unassigned-suspended-bootstrap-root') {
        if ($null -eq $Prior.record['bootstrap_root_creation_filetime']) { throw "$Stage unassigned root hard stop lacks creation-time identity" }
        $Process = [AgentsCommander.Review1283.NativeCbmJobInterop]::OpenProcess(([AgentsCommander.Review1283.NativeCbmJobInterop]::PROCESS_QUERY_LIMITED_INFORMATION -bor [AgentsCommander.Review1283.NativeCbmJobInterop]::SYNCHRONIZE), $false, [uint32]$Prior.record['bootstrap_root_pid'])
        if ($Process -ne [IntPtr]::Zero) { try { if ([AgentsCommander.Review1283.NativeCbmJobInterop]::GetProcessCreationFileTime($Process) -eq [int64]$Prior.record['bootstrap_root_creation_filetime']) { throw "$Stage unassigned root identity remains live" } } finally { [void][AgentsCommander.Review1283.NativeCbmJobInterop]::CloseHandle($Process) } }
        elseif ([System.Runtime.InteropServices.Marshal]::GetLastWin32Error() -ne [AgentsCommander.Review1283.NativeCbmJobInterop]::ERROR_INVALID_PARAMETER) { throw "$Stage cannot prove unassigned root is gone" }
      } else { throw "$Stage has unknown hard-stop containment scope" }
    }
    elseif ($null -ne $Prior -and $Prior.record['state'] -cne 'cleared') { throw "$Stage durable hard-stop state is not clearable" }
    $Record = [ordered]@{ schema_version = $script:NativeCbmEvidenceControlSchemaVersion; state = 'cleared'; kernel_namespace = $script:NativeCbmKernelObjectNamespace; principal_sid = $script:NativeCbmPrincipalSid; machine_name = $script:NativeCbmMachineName; interactive_session_id = $script:NativeCbmInteractiveSessionId; interactive_logon_luid = $script:NativeCbmInteractiveLogonLuid; interactive_session_binding_sha256 = $script:NativeCbmInteractiveSessionBindingSha256; state_store_scope = $script:NativeCbmStateStoreScope; canonical_repository_root = $CanonicalRepositoryRoot; canonical_plan_path = $CanonicalPlanPath; root_hash = (& $P['Get-NativeCbmControlRootHash'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot); draft_plan_sha256 = $ExpectedPlanSha256; clearance_evidence_epoch = $EvidenceEpoch; clearance_session_id = $script:NativeCbmInteractiveSessionId }
    $Clearance = & $P['Write-NativeCbmControlState'] -Record $Record -CanonicalRepositoryRoot $CanonicalRepositoryRoot -RepositoryLease $RepositoryLease -Stage "$Stage-clearance-write"
    & $P['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-after-clearance-write" | Out-Null
    [pscustomobject]@{ clearance = $Clearance; prior_state = if ($null -eq $Prior) { 'none' } else { $Prior.record['state'] }; clearance_state = 'cleared' }
  }
  function Assert-NativeCbmEvidenceMayProceed {
    param([string]$CanonicalRepositoryRoot, [string]$CanonicalPlanPath, [string]$ExpectedPlanSha256, [string]$EvidenceEpoch, [object]$RepositoryLease, [string]$Stage)
    $P = $script:NativeCbmPrivateControls; & $P['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-lease"
    $State = & $P['Read-NativeCbmControlState'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot -CanonicalPlanPath $CanonicalPlanPath -RepositoryLease $RepositoryLease -Stage $Stage
    if ($null -eq $State -or $State.record['state'] -cne 'cleared' -or $State.record['draft_plan_sha256'] -cne $ExpectedPlanSha256 -or $State.record['clearance_evidence_epoch'] -cne $EvidenceEpoch) { throw "$Stage has no matching native hard-stop clearance" }
  }
  function Invoke-ContainedNativeCbmCapture {
    param([string]$Stage, [ValidateSet('gate', 'text', 'name')] [string]$Operation, [string[]]$OperationArguments, [string]$CanonicalRepositoryRoot, [string]$CanonicalPlanPath, [string]$ExpectedPlanSha256, [object]$WrapperBindingCapability, [string]$EvidenceEpoch, [object]$RepositoryLease, [TimeSpan]$Timeout, [int]$MaximumStandardInputBytes, [int]$MaximumStandardOutputBytes, [int]$MaximumStandardErrorBytes, [int]$TerminationWaitMilliseconds)
    if ($Timeout -le [TimeSpan]::Zero -or $MaximumStandardInputBytes -lt 1 -or $MaximumStandardOutputBytes -lt 1 -or $MaximumStandardErrorBytes -lt 1 -or $TerminationWaitMilliseconds -le 0) { throw "$Stage invalid native capture limits" }
    $P = $script:NativeCbmPrivateControls; $Public = $script:NativeCbmPublicControls
    & $Public['Assert-NativeCbmEvidenceMayProceed'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot -CanonicalPlanPath $CanonicalPlanPath -ExpectedPlanSha256 $ExpectedPlanSha256 -EvidenceEpoch $EvidenceEpoch -RepositoryLease $RepositoryLease -Stage "$Stage-before-capture"
    $Deadline = [DateTime]::UtcNow.Add($Timeout); $Cancellation = [System.Threading.CancellationTokenSource]::new(); $Job = [IntPtr]::Zero; $Started = $null; $Artifact = $null; $Bootstrap = $null; $Wrapper = $null; $AssignmentVerified = $false; $TreeConfirmed = $true
    $RootHash = & $P['Get-NativeCbmControlRootHash'] -CanonicalRepositoryRoot $CanonicalRepositoryRoot; $JobName = "Local\AgentsCommander-1283-cbm-$RootHash-$($script:NativeCbmPrincipalSidHash.Substring(0, 16))-$($script:NativeCbmInteractiveSessionId)-$($script:NativeCbmInteractiveSessionBindingSha256.Substring(0, 24))-$EvidenceEpoch-$([guid]::NewGuid().ToString('N'))"
    try {
      $Wrapper = & $P['Open-VerifiedCodebaseMemoryWrapperReadLease'] -WrapperBindingCapability $WrapperBindingCapability -Stage "$Stage-wrapper-open"
      if ([DateTime]::UtcNow -ge $Deadline) { throw "$Stage deadline expired before artifact creation" }
      $Artifact = & $P['New-NativeCbmVerifiedExecutionArtifact'] -WrapperReadLease $Wrapper -CanonicalRepositoryRoot $CanonicalRepositoryRoot -EvidenceEpoch $EvidenceEpoch -Stage "$Stage-artifact-create"
      $Bootstrap = & $P['New-NativeCbmBootstrapReadLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-bootstrap-create"
      $Job = & $P['New-NativeCbmControlJob'] -JobName $JobName -Stage "$Stage-job-create"
      $Started = & $P['Start-NativeCbmBootstrapSuspended'] -BootstrapReadLease $Bootstrap -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-suspended-start"; $TreeConfirmed = $false
      if ($null -ne $Started.post_create_error) { throw "$Stage suspended bootstrap post-create failure: $($Started.post_create_error)" }
      if (-not [AgentsCommander.Review1283.NativeCbmJobInterop]::AssignProcessToJobObject($Job, $Started.process_handle)) { throw "$Stage AssignProcessToJobObject failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
      [bool]$InJob = $false; if (-not [AgentsCommander.Review1283.NativeCbmJobInterop]::IsProcessInJob($Started.process_handle, $Job, [ref]$InJob) -or -not $InJob) { throw "$Stage Job assignment verification failed" }
      $AssignmentVerified = $true
      $OutputTask = [AgentsCommander.Review1283.NativeCbmJobInterop]::ReadBoundedAsync($Started.stdout, $MaximumStandardOutputBytes, $Cancellation.Token)
      $ErrorTask = [AgentsCommander.Review1283.NativeCbmJobInterop]::ReadBoundedAsync($Started.stderr, $MaximumStandardErrorBytes, $Cancellation.Token)
      $Token = [byte[]]::new(32); [System.Security.Cryptography.RandomNumberGenerator]::Fill($Token)
      $Payload = [ordered]@{ token = [Convert]::ToBase64String($Token); artifact_path = $Artifact.path; artifact_sha256 = $Artifact.sha256; artifact_byte_length = $Artifact.byte_length; artifact_file_identity = $Artifact.identity; operation_arguments = $OperationArguments }
      $PayloadBytes = ([System.Text.UTF8Encoding]::new($false, $true)).GetBytes(($Payload | ConvertTo-Json -Depth 16 -Compress))
      if ([DateTime]::UtcNow -ge $Deadline) { throw "$Stage deadline expired before post-assignment resume" }
      if ([AgentsCommander.Review1283.NativeCbmJobInterop]::ResumeThread($Started.thread_handle) -eq [uint32]::MaxValue) { throw "$Stage ResumeThread failed: $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error())" }
      $PayloadWrite = & $P['Write-NativeCbmCapturePayload'] -Started $Started -PayloadBytes $PayloadBytes -MaximumPayloadBytes $MaximumStandardInputBytes -CaptureDeadlineUtc $Deadline -Cancellation $Cancellation -Stage "$Stage-payload-write"
      $Completed = & $P['Wait-NativeCbmCaptureCompletion'] -Started $Started -OutputTask $OutputTask -ErrorTask $ErrorTask -CaptureDeadlineUtc $Deadline -Cancellation $Cancellation -Stage $Stage
      $ArtifactIdentity = & $P['Assert-NativeCbmVerifiedExecutionArtifact'] -Artifact $Artifact -Stage "$Stage-artifact-post-capture"
      $BootstrapIdentity = & $P['Assert-NativeCbmBootstrapReadLease'] -BootstrapReadLease $Bootstrap -Stage "$Stage-bootstrap-post-capture"
      $Active = & $P['Get-NativeCbmControlActiveProcessCount'] -JobHandle $Job -Stage "$Stage-success-job-query"; if ($Active -ne 0) { throw "$Stage child exited with active Job descendants" }
      $WrapperIdentity = & $P['Close-VerifiedCodebaseMemoryWrapperReadLease'] -WrapperReadLease $Wrapper -Stage "$Stage-wrapper-post-capture"; $Wrapper = $null; $TreeConfirmed = $true
      [pscustomobject]@{ exit_code = $Completed.exit_code; standard_output = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($Completed.standard_output_bytes); standard_error = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($Completed.standard_error_bytes); standard_output_bytes = $Completed.standard_output_bytes.Length; standard_error_bytes = $Completed.standard_error_bytes.Length; standard_input_bytes = $PayloadWrite.payload_byte_length; payload_write_completed = $PayloadWrite.payload_write_completed; tree_termination_confirmed = $true; root_exit_observed = $true; readers_closed = $true; assignment_state = 'verified-assigned'; job_name = $JobName; job_active_processes = 0; evidence_epoch = $EvidenceEpoch; wrapper_identity = $WrapperIdentity; bootstrap_identity = $BootstrapIdentity; executed_artifact = $ArtifactIdentity }
    } catch {
      $Failure = $_.Exception; $Cancellation.Cancel()
      if ($null -ne $Started -and -not $AssignmentVerified) {
        try { $Termination = & $P['Stop-NativeCbmUnassignedBootstrapAndConfirm'] -Started $Started -TerminationWaitMilliseconds $TerminationWaitMilliseconds -Stage "$Stage-unassigned-stop" } catch { $Termination = [pscustomobject]@{ root_termination_confirmed = $false; termination_error = $_.Exception.Message; confirmation_error = $_.Exception.Message; process_count_state = 'unknown'; job_active_processes = $null } }
        $TreeConfirmed = ($Termination.root_termination_confirmed -eq $true); $Scope = 'unassigned-suspended-bootstrap-root'
      } elseif ($AssignmentVerified) {
        try { $Termination = & $P['Stop-NativeCbmControlJobAndConfirm'] -JobHandle $Job -TerminationWaitMilliseconds $TerminationWaitMilliseconds -Stage "$Stage-job-stop" } catch { $Termination = [pscustomobject]@{ tree_termination_confirmed = $false; termination_error = $_.Exception.Message; confirmation_error = $_.Exception.Message; process_count_state = 'unknown'; job_active_processes = $null } }
        $TreeConfirmed = ($Termination.tree_termination_confirmed -eq $true); $Scope = 'assigned-job-tree'
      } else { $Termination = [pscustomobject]@{ tree_termination_confirmed = $true; process_count_state = 'known-zero'; job_active_processes = 0; termination_error = $null; confirmation_error = $null }; $Scope = 'no-root-started' }
      if (-not $TreeConfirmed) { & $P['Persist-NativeCbmUnconfirmedTerminationAndRetain'] -CaptureFailure $Failure -Termination $Termination -ContainmentScope $Scope -JobHandle $Job -Started $Started -Artifact $Artifact -BootstrapReadLease $Bootstrap -WrapperReadLease $Wrapper -JobName $JobName -CanonicalRepositoryRoot $CanonicalRepositoryRoot -CanonicalPlanPath $CanonicalPlanPath -ExpectedPlanSha256 $ExpectedPlanSha256 -EvidenceEpoch $EvidenceEpoch -RepositoryLease $RepositoryLease -Stage $Stage }
      throw $Failure
    } finally {
      $Cancellation.Dispose()
      if ($TreeConfirmed) {
        if ($null -ne $Wrapper) { try { [void](& $P['Close-VerifiedCodebaseMemoryWrapperReadLease'] -WrapperReadLease $Wrapper -Stage "$Stage-wrapper-finally") } catch {} }
        if ($null -ne $Artifact) { try { & $P['Remove-NativeCbmVerifiedExecutionArtifact'] -Artifact $Artifact -Stage "$Stage-artifact-cleanup" } catch {} }
        if ($null -ne $Bootstrap) { try { & $P['Remove-NativeCbmBootstrapReadLease'] -BootstrapReadLease $Bootstrap -Stage "$Stage-bootstrap-cleanup" } catch {} }
        if ($null -ne $Started) { $Started.stdin.Dispose(); $Started.stdout.Dispose(); $Started.stderr.Dispose(); [void][AgentsCommander.Review1283.NativeCbmJobInterop]::CloseHandle($Started.thread_handle); [void][AgentsCommander.Review1283.NativeCbmJobInterop]::CloseHandle($Started.process_handle) }
        if ($Job -ne [IntPtr]::Zero) { [void][AgentsCommander.Review1283.NativeCbmJobInterop]::CloseHandle($Job) }
      }
    }
  }
  function Assert-NativeCbmEvidenceControlModule {
    param([psobject]$ControlModule, [object]$RepositoryLease, [string]$CanonicalRepositoryRoot, [object]$WrapperBindingCapability, [string]$Stage)
    & $script:NativeCbmPrivateControls['Assert-NativeCbmControlLease'] -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-lease"
    if ($ControlModule.module_name -cne $script:NativeCbmControlModuleName -or $ControlModule.module_guid -cne $script:NativeCbmControlModuleGuid -or $null -eq $ControlModule.public_commands) { throw "$Stage module identity or descriptors differ" }
    foreach ($Name in $script:NativeCbmPublicControlNames) {
      $Descriptor = $ControlModule.public_commands[$Name]
      if ($null -eq $Descriptor -or $Descriptor.Module.Name -cne $script:NativeCbmControlModuleName -or $Descriptor.Module.Guid.ToString('D') -cne $script:NativeCbmControlModuleGuid) { throw "$Stage public descriptor $Name is not this module" }
    }
    $WrapperCapabilityState = 'not-requested'; $WrapperCapabilityOrigin = 'none'
    if ($null -ne $WrapperBindingCapability) {
      [void](& $script:NativeCbmPrivateControls['Get-NativeCbmWrapperBindingCapability'] -WrapperBindingCapability $WrapperBindingCapability -Stage "$Stage-wrapper-capability")
      $WrapperCapabilityState = 'rebind-issued-exact-object'; $WrapperCapabilityOrigin = 'original-loader-identity'
    }
    [pscustomobject]@{ module_name = $script:NativeCbmControlModuleName; module_guid = $script:NativeCbmControlModuleGuid; private_command_count = $script:NativeCbmPrivateControls.Count; control_schema_version = $script:NativeCbmEvidenceControlSchemaVersion; wrapper_capability_state = $WrapperCapabilityState; wrapper_capability_origin = $WrapperCapabilityOrigin }
  }

  $script:NativeCbmPublicControlNames = @('Resolve-InstalledCodebaseMemoryWrapperBinding', 'Confirm-NativeCbmPersistentHardStopCleared', 'Assert-NativeCbmEvidenceMayProceed', 'Invoke-ContainedNativeCbmCapture', 'Assert-NativeCbmEvidenceControlModule')
  $script:NativeCbmPrivateControlNames = @('ConvertTo-NativeCbmControlCanonicalPath', 'Get-NativeCbmControlRootHash', 'Get-NativeCbmControlBytesSha256', 'Get-NativeCbmSha256FromStream', 'Assert-NativeCbmControlLease', 'Get-NativeCbmControlStatePaths', 'Assert-NativeCbmLocalSessionStateStore', 'Get-NativeCbmControlRequiredString', 'Read-NativeCbmControlState', 'Write-NativeCbmControlBytes', 'Write-NativeCbmControlState', 'Get-NativeCbmControlActiveProcessCount', 'Get-NativeCbmPhysicalPathComponent', 'Assert-NativeCbmInstalledSkillPhysicalPath', 'Get-NativeCbmWrapperBindingCapability', 'Open-VerifiedCodebaseMemoryWrapperReadLease', 'Close-VerifiedCodebaseMemoryWrapperReadLease', 'New-NativeCbmControlJob', 'Stop-NativeCbmControlJobAndConfirm', 'New-NativeCbmBootstrapReadLease', 'Remove-NativeCbmBootstrapReadLease', 'Assert-NativeCbmBootstrapReadLease', 'New-NativeCbmVerifiedExecutionArtifact', 'Assert-NativeCbmVerifiedExecutionArtifact', 'Remove-NativeCbmVerifiedExecutionArtifact', 'Start-NativeCbmBootstrapSuspended', 'Stop-NativeCbmUnassignedBootstrapAndConfirm', 'Wait-NativeCbmCaptureCompletion', 'Write-NativeCbmCapturePayload', 'Persist-NativeCbmUnconfirmedTerminationAndRetain')
  $script:NativeCbmPrivateControls = [ordered]@{}; $script:NativeCbmPublicControls = [ordered]@{}
  foreach ($Name in $script:NativeCbmPrivateControlNames) {
    $Command = $ExecutionContext.SessionState.InvokeCommand.GetCommand($Name, [System.Management.Automation.CommandTypes]::Function)
    if ($null -eq $Command -or $Command.Module.Name -cne $script:NativeCbmControlModuleName -or $Command.Module.Guid.ToString('D') -cne $script:NativeCbmControlModuleGuid) { throw "private control $Name did not resolve to this module" }
    $script:NativeCbmPrivateControls[$Name] = $Command
  }
  foreach ($Name in $script:NativeCbmPublicControlNames) {
    $Command = $ExecutionContext.SessionState.InvokeCommand.GetCommand($Name, [System.Management.Automation.CommandTypes]::Function)
    if ($null -eq $Command -or $Command.Module.Name -cne $script:NativeCbmControlModuleName -or $Command.Module.Guid.ToString('D') -cne $script:NativeCbmControlModuleGuid) { throw "public control $Name did not resolve to this module" }
    $script:NativeCbmPublicControls[$Name] = $Command
  }
  Microsoft.PowerShell.Core\Export-ModuleMember -Function $script:NativeCbmPublicControlNames
}

$NativeCbmPublicControlNames = @('Resolve-InstalledCodebaseMemoryWrapperBinding', 'Confirm-NativeCbmPersistentHardStopCleared', 'Assert-NativeCbmEvidenceMayProceed', 'Invoke-ContainedNativeCbmCapture', 'Assert-NativeCbmEvidenceControlModule')
$NativeCbmPrivateControlNames = @('ConvertTo-NativeCbmControlCanonicalPath', 'Get-NativeCbmControlRootHash', 'Get-NativeCbmControlBytesSha256', 'Get-NativeCbmSha256FromStream', 'Assert-NativeCbmControlLease', 'Get-NativeCbmControlStatePaths', 'Assert-NativeCbmLocalSessionStateStore', 'Get-NativeCbmControlRequiredString', 'Read-NativeCbmControlState', 'Write-NativeCbmControlBytes', 'Write-NativeCbmControlState', 'Get-NativeCbmControlActiveProcessCount', 'Get-NativeCbmPhysicalPathComponent', 'Assert-NativeCbmInstalledSkillPhysicalPath', 'Get-NativeCbmWrapperBindingCapability', 'Open-VerifiedCodebaseMemoryWrapperReadLease', 'Close-VerifiedCodebaseMemoryWrapperReadLease', 'New-NativeCbmControlJob', 'Stop-NativeCbmControlJobAndConfirm', 'New-NativeCbmBootstrapReadLease', 'Remove-NativeCbmBootstrapReadLease', 'Assert-NativeCbmBootstrapReadLease', 'New-NativeCbmVerifiedExecutionArtifact', 'Assert-NativeCbmVerifiedExecutionArtifact', 'Remove-NativeCbmVerifiedExecutionArtifact', 'Start-NativeCbmBootstrapSuspended', 'Stop-NativeCbmUnassignedBootstrapAndConfirm', 'Wait-NativeCbmCaptureCompletion', 'Write-NativeCbmCapturePayload', 'Persist-NativeCbmUnconfirmedTerminationAndRetain')
function Assert-NativeCbmEvidenceControlSourceContract {
  param([scriptblock]$Source, [string]$Stage)
  $Text = $Source.ToString(); $Tokens = $null; $Errors = $null
  $Ast = [System.Management.Automation.Language.Parser]::ParseInput($Text, [ref]$Tokens, [ref]$Errors)
  if (@($Errors).Count -ne 0) { throw "$Stage active NativeCbm source has parser errors" }
  $Expected = @($NativeCbmPublicControlNames + $NativeCbmPrivateControlNames | Sort-Object)
  $Defined = @($Ast.FindAll({ param($Node) $Node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $true) | ForEach-Object { $_.Name } | Sort-Object)
  if ($Defined.Count -ne $Expected.Count -or $null -ne (Compare-Object -ReferenceObject $Expected -DifferenceObject $Defined)) { throw "$Stage active NativeCbm source function manifest differs" }
  foreach ($Legacy in @('ProcessStartInfo', 'Process.Start(', 'CancellationToken.None', 'GetAwaiter().GetResult()', "Payload['wrapper_path']", '$Started.stdin.Write(', 'WaitOne([TimeSpan]::Zero)', '$RepositoryLease.mutex', 'IsPathRooted', 'CreateJobObject([IntPtr]::Zero, $JobName)', '-ExpectedBinding $WrapperBinding', '$script:NativeCbmPrivateControls[''Write-NativeCbmControlBytes''] -Path $Path -Bytes $Bytes')) { if ($Text.Contains($Legacy)) { throw "$Stage active NativeCbm source contains unsafe legacy token $Legacy" } }
  $BarePrivate = @($Ast.FindAll({ param($Node) $Node -is [System.Management.Automation.Language.CommandAst] -and $NativeCbmPrivateControlNames -contains $Node.GetCommandName() }, $true))
  if ($BarePrivate.Count -ne 0) { throw "$Stage active NativeCbm source has bare private-control invocation" }
  $Resolver = @($Ast.FindAll({ param($Node) $Node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $Node.Name -ceq 'Resolve-InstalledCodebaseMemoryWrapperBinding' }, $true))
  if ($Resolver.Count -ne 1) { throw "$Stage active NativeCbm source has no unique wrapper-capability resolver" }
  $ResolverParameters = @($Resolver[0].Body.ParamBlock.Parameters | ForEach-Object { $_.Name.VariablePath.UserPath })
  if ($ResolverParameters.Count -ne 1 -or $ResolverParameters[0] -cne 'Stage') { throw "$Stage wrapper-capability resolver accepts an alternate path or root" }
  $PhysicalComponent = @($Ast.FindAll({ param($Node) $Node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $Node.Name -ceq 'Get-NativeCbmPhysicalPathComponent' }, $true))
  $PhysicalWalker = @($Ast.FindAll({ param($Node) $Node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $Node.Name -ceq 'Assert-NativeCbmInstalledSkillPhysicalPath' }, $true))
  if ($PhysicalComponent.Count -ne 1 -or $PhysicalWalker.Count -ne 1 -or -not $PhysicalComponent[0].Extent.Text.Contains('FILE_ATTRIBUTE_REPARSE_POINT') -or -not $PhysicalComponent[0].Extent.Text.Contains('GetFinalPathByHandle') -or -not $Text.Contains('GetFinalPathNameByHandleW') -or -not $PhysicalWalker[0].Extent.Text.Contains('$Chain.Add') -or -not $PhysicalWalker[0].Extent.Text.Contains('$Stage-component-$Index')) { throw "$Stage active NativeCbm source lacks the full physical reparse-chain proof" }
  $LocalJobCreator = @($Ast.FindAll({ param($Node) $Node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $Node.Name -ceq 'New-NativeCbmControlJob' }, $true))
  if ($LocalJobCreator.Count -ne 1 -or -not $LocalJobCreator[0].Extent.Text.Contains('CreateJobObjectWithSddl') -or -not $LocalJobCreator[0].Extent.Text.Contains('Local\\AgentsCommander-1283-cbm-')) { throw "$Stage active NativeCbm source lacks the Local ACL-safe capture-Job proof" }
  $LocalStateReader = @($Ast.FindAll({ param($Node) $Node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $Node.Name -ceq 'Read-NativeCbmControlState' }, $true))
  $LocalClearer = @($Ast.FindAll({ param($Node) $Node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $Node.Name -ceq 'Confirm-NativeCbmPersistentHardStopCleared' }, $true))
  if ($LocalStateReader.Count -ne 1 -or $LocalClearer.Count -ne 1) { throw "$Stage active NativeCbm source lacks unique local-state reader or clearer" }
  $LocalStateReaderText = $LocalStateReader[0].Extent.Text; $LocalClearerText = $LocalClearer[0].Extent.Text
  foreach ($RequiredLocalScopeField in @('kernel_namespace', 'principal_sid', 'machine_name', 'interactive_session_id', 'interactive_logon_luid', 'interactive_session_binding_sha256', 'state_store_scope', 'FOREIGN_INTERACTIVE_SESSION_HARD_STOP')) {
    if (-not $LocalStateReaderText.Contains($RequiredLocalScopeField)) { throw "$Stage local-state reader lacks $RequiredLocalScopeField" }
  }
  $ForeignGuardOffset = $LocalClearerText.IndexOf('FOREIGN_INTERACTIVE_SESSION_HARD_STOP'); $JobOpenOffset = $LocalClearerText.IndexOf('OpenJobObject'); $ClearanceWriteOffset = $LocalClearerText.IndexOf('Write-NativeCbmControlState')
  if ($ForeignGuardOffset -lt 0 -or ($JobOpenOffset -ge 0 -and $ForeignGuardOffset -gt $JobOpenOffset) -or ($ClearanceWriteOffset -ge 0 -and $ForeignGuardOffset -gt $ClearanceWriteOffset)) { throw "$Stage local-scope guard does not precede Job control and clearance" }
  $NativeLeaseAssertion = @($Ast.FindAll({ param($Node) $Node -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $Node.Name -ceq 'Assert-NativeCbmControlLease' }, $true))
  if ($NativeLeaseAssertion.Count -ne 1 -or -not $NativeLeaseAssertion[0].Extent.Text.Contains('$script:NativeCbmGenuineOuterAssertionDescriptor') -or -not $Text.Contains('GenuineRepositoryLeaseAssertDescriptor') -or -not $Text.Contains('GenuineRepositoryLeaseAssertScriptBlock') -or -not $Text.Contains('GenuineRepositoryLeaseSessionIdDescriptor') -or -not $Text.Contains('GenuineRepositoryLeaseSessionIdScriptBlock') -or -not $Text.Contains('ReferenceEquals') -or -not $Text.Contains('FAIL_OUTER_DESCRIPTOR_IDENTITY_REPLACED')) { throw "$Stage active NativeCbm source lacks immutable genuine outer-descriptor identity proof" }
  foreach ($Forbidden in @('Glo' + 'bal\\AgentsCommander-1283-cbm-', 'global' + '-current-user', 'UNAVAILABLE_' + 'MULTI_SESSION_HOST', 'Remote' + ' Desktop')) { if ($Text.Contains($Forbidden)) { throw "$Stage active NativeCbm source contains superseded nonlocal-scope token $Forbidden" } }
  foreach ($Required in @('CreateProcessW', 'ResumeThread', 'TerminateProcess', 'WaitForSingleObject', 'GetExitCodeProcess', 'GetFileInformationByHandle', 'GetFileIdentity', 'OpenProcess', 'GetProcessTimes', 'WriteBoundedAsync', 'Start-NativeCbmBootstrapSuspended', 'Assert-NativeCbmBootstrapReadLease', 'Wait-NativeCbmCaptureCompletion', 'Write-NativeCbmCapturePayload', 'New-NativeCbmVerifiedExecutionArtifact', 'Remove-NativeCbmVerifiedExecutionArtifact', 'Persist-NativeCbmUnconfirmedTerminationAndRetain', 'MaximumStandardInputBytes', 'SKILL.md', 'IsPathFullyQualified', 'drive-relative', 'root-relative', 'FILE_FLAG_OPEN_REPARSE_POINT', 'GetFinalPathNameByHandleW', 'NativeCbmWrapperCapabilityRegistry', 'WrapperBindingCapability', 'TrustedInstalledSkillsRootPath', 'RepositoryLeaseSessionInterop', 'RepositoryLeaseSessionIdDescriptor', 'Get-RepositoryLeaseCurrentInteractiveSessionId', 'Local\\AgentsCommander-1283-cbm-', 'CreateJobObjectWithSddl', 'SetLastError(0)', 'local-current-user-interactive-session', 'interactive_logon_luid', 'interactive_session_binding_sha256', 'FOREIGN_INTERACTIVE_SESSION_HARD_STOP', 'ERROR_ACCESS_DENIED', 'wrapper_file_identity', 'NativeCbmGenuineOuterAssertionDescriptor', 'NativeCbmGenuineOuterSessionIdDescriptor', 'outer-held-capability', 'unconfirmed-tree-termination', 'NativeCbmPublicControls')) { if (-not $Text.Contains($Required)) { throw "$Stage active NativeCbm source lacks $Required" } }
  [pscustomobject]@{ source_sha256 = [Convert]::ToHexString([System.Security.Cryptography.SHA256]::HashData(([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($Text))); defined_functions = $Defined; private_manifest_count = $NativeCbmPrivateControlNames.Count; wrapper_capability_route = 'exact-object-registry-original-loader-only'; physical_reparse_chain = 'root-through-skill-scripts-wrapper'; kernel_scope = 'Local-current-user-interactive-session'; parser_errors = 0; bare_private_calls = 0 }
}
function Import-NativeCbmEvidenceControlModule {
  param([object]$RepositoryLease, [string]$CanonicalRepositoryRoot, [string]$LoaderProvidedSkillMarkdownPath, [string]$TrustedInstalledSkillsRootPath, [string]$Stage)
  $LeaseRecord = & $NativeCbmGenuineOuterAssertionDescriptor -Lease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-before-import"
  if ($LeaseRecord.kernel_namespace -cne 'Local' -or $LeaseRecord.principal_sid -cnotmatch '^S-\d-(?:\d+-)+\d+$' -or $LeaseRecord.machine_name -cne $script:RepositoryLeaseMachineName -or $LeaseRecord.interactive_session_id -ne $script:RepositoryLeaseInteractiveSessionId -or $LeaseRecord.interactive_logon_luid -cne $script:RepositoryLeaseInteractiveLogonLuid -or $LeaseRecord.interactive_session_binding_sha256 -cne $script:RepositoryLeaseInteractiveSessionBindingSha256 -or $LeaseRecord.state_store_scope -cne 'local-current-user-interactive-session') { throw "$Stage held repository lease does not provide the Local current-user interactive-session scope" }
  $CurrentProcessSessionId = & $NativeCbmGenuineOuterSessionIdDescriptor -Stage "$Stage-current-process-session"
  if ($CurrentProcessSessionId -isnot [int] -or $CurrentProcessSessionId -le 0 -or $CurrentProcessSessionId -ne $LeaseRecord.interactive_session_id) { throw "$Stage held repository lease does not match the current-process interactive session" }
  foreach ($Name in $NativeCbmPublicControlNames) {
    if (@(Microsoft.PowerShell.Core\Get-Command -Name $Name -All -ErrorAction SilentlyContinue).Count -ne 0) { throw "$Stage rejects pre-import ambient command collision for $Name" }
  }
  $SourceContract = & $NativeCbmEvidenceControlSourceContractDescriptor -Source $NativeCbmEvidenceControlModuleSource -Stage "$Stage-source-contract"
  $Module = Microsoft.PowerShell.Core\New-Module -Name ("AgentsCommander1283.NativeCbmEvidence.$($RepositoryLease.lease_id)") -ScriptBlock $NativeCbmEvidenceControlModuleSource -ArgumentList @($NativeCbmGenuineOuterAssertionDescriptor, $NativeCbmGenuineOuterSessionIdDescriptor, $NativeCbmGenuineOuterAssertionDescriptor, $NativeCbmGenuineOuterAssertionScriptBlock, $NativeCbmGenuineOuterSessionIdDescriptor, $NativeCbmGenuineOuterSessionIdScriptBlock, $LoaderProvidedSkillMarkdownPath, $TrustedInstalledSkillsRootPath, $LeaseRecord.kernel_namespace, $LeaseRecord.principal_sid, $LeaseRecord.machine_name, $LeaseRecord.interactive_session_id, $LeaseRecord.interactive_logon_luid, $LeaseRecord.interactive_session_binding_sha256, $LeaseRecord.state_store_scope)
  Microsoft.PowerShell.Core\Import-Module -ModuleInfo $Module -Global -ErrorAction Stop
  $Descriptors = [ordered]@{}
  foreach ($Name in $NativeCbmPublicControlNames) {
    $Descriptor = $Module.ExportedFunctions[$Name]
    if ($null -eq $Descriptor -or $Descriptor.Module.Name -cne $Module.Name -or $Descriptor.Module.Guid.ToString('D') -cne $Module.Guid.ToString('D')) { throw "$Stage exported descriptor $Name differs from imported module" }
    $Descriptors[$Name] = $Descriptor
  }
  $PrivateDescriptors = @(& $Module { $script:NativeCbmPrivateControls.GetEnumerator() | ForEach-Object { [pscustomobject]@{ name = $_.Key; module_name = $_.Value.Module.Name; module_guid = $_.Value.Module.Guid.ToString('D') } } })
  if ($PrivateDescriptors.Count -ne $NativeCbmPrivateControlNames.Count) { throw "$Stage private descriptor count differs" }
  $ControlModule = [pscustomobject]@{ module_name = $Module.Name; module_guid = $Module.Guid.ToString('D'); public_commands = $Descriptors; private_commands = $PrivateDescriptors; source_contract = $SourceContract }
  & $Descriptors['Assert-NativeCbmEvidenceControlModule'] -ControlModule $ControlModule -RepositoryLease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-after-import" | Out-Null
  $ControlModule
}
# This private construction scope runs in the lexical scope that defines the
# four genuine outer functions, before the importer descriptor is made
# reachable. It captures each exact FunctionInfo plus the same ScriptBlock
# object. No importer, module, or durable-write route resolves one of these
# names from mutable ambient session state after this point.
$NativeCbmGenuineOuterDescriptorSet = & {
  $Captured = [ordered]@{}
  foreach ($Name in @('Assert-RepositoryMutationLease', 'Get-RepositoryLeaseCurrentInteractiveSessionId', 'Assert-NativeCbmEvidenceControlSourceContract', 'Import-NativeCbmEvidenceControlModule')) {
    $Descriptor = $ExecutionContext.SessionState.InvokeCommand.GetCommand($Name, [System.Management.Automation.CommandTypes]::Function)
    if ($null -eq $Descriptor -or $Descriptor.Name -cne $Name -or $Descriptor.CommandType -ne [System.Management.Automation.CommandTypes]::Function -or $null -eq $Descriptor.ScriptBlock) { throw "native control construction scope cannot capture genuine outer descriptor $Name" }
    $Captured[$Name] = [pscustomobject][ordered]@{ descriptor = $Descriptor; script_block = $Descriptor.ScriptBlock }
  }
  [pscustomobject][ordered]@{ assertion = $Captured['Assert-RepositoryMutationLease']; session_id = $Captured['Get-RepositoryLeaseCurrentInteractiveSessionId']; source_contract = $Captured['Assert-NativeCbmEvidenceControlSourceContract']; importer = $Captured['Import-NativeCbmEvidenceControlModule'] }
}
$NativeCbmGenuineOuterAssertionDescriptor = $NativeCbmGenuineOuterDescriptorSet.assertion.descriptor
$NativeCbmGenuineOuterAssertionScriptBlock = $NativeCbmGenuineOuterDescriptorSet.assertion.script_block
$NativeCbmGenuineOuterSessionIdDescriptor = $NativeCbmGenuineOuterDescriptorSet.session_id.descriptor
$NativeCbmGenuineOuterSessionIdScriptBlock = $NativeCbmGenuineOuterDescriptorSet.session_id.script_block
$NativeCbmEvidenceControlSourceContractDescriptor = $NativeCbmGenuineOuterDescriptorSet.source_contract.descriptor
$NativeCbmEvidenceControlImporterDescriptor = $NativeCbmGenuineOuterDescriptorSet.importer.descriptor
$ImporterText = $NativeCbmEvidenceControlImporterDescriptor.ScriptBlock.ToString(); $ImporterTokens = $null; $ImporterErrors = $null
[void][System.Management.Automation.Language.Parser]::ParseInput($ImporterText, [ref]$ImporterTokens, [ref]$ImporterErrors)
if (@($ImporterErrors).Count -ne 0 -or -not $ImporterText.Contains('Microsoft.PowerShell.Core\New-Module') -or -not $ImporterText.Contains('-ScriptBlock $NativeCbmEvidenceControlModuleSource') -or -not $ImporterText.Contains('$NativeCbmGenuineOuterAssertionDescriptor') -or -not $ImporterText.Contains('$NativeCbmGenuineOuterAssertionScriptBlock') -or -not $ImporterText.Contains('$NativeCbmGenuineOuterSessionIdDescriptor') -or -not $ImporterText.Contains('$NativeCbmGenuineOuterSessionIdScriptBlock')) { throw 'native control importer descriptor is not the expected immutable outer-descriptor importer' }

# ---------------------------------------------------------------------------
# Step 8: LocalProofFixtureStateAdapter, fixture hard stop, owner identity, and
# coordinator-only cleanup (Sections 22.1.0.a-b).
#
# The fixture adapter is an external durable-state seam: it reads and writes ONLY
# ProofRunRoot\fixture-state\<SID-hash>\<session-id>-<binding-prefix>\<root-hash>.json
# and implements schema-4 exact-key, current-scope, Job-name, live-Job block, and
# cleared-transition rules without touching production durable state.
# ---------------------------------------------------------------------------

function Get-ProtectedCurrentUserAcl {
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

function Assert-FixtureScopeMatchesLease {
    param(
        [Parameter(Mandatory)] [string]$Stage,
        [Parameter(Mandatory)] $LeaseRecord
    )

    if ($LeaseRecord.kernel_namespace -cne $script:RepositoryLeaseKernelObjectNamespace -or
        $LeaseRecord.principal_sid -cne $script:RepositoryLeasePrincipalSid -or
        $LeaseRecord.machine_name -cne $script:RepositoryLeaseMachineName -or
        $LeaseRecord.interactive_session_id -ne $script:RepositoryLeaseInteractiveSessionId -or
        $LeaseRecord.interactive_logon_luid -cne $script:RepositoryLeaseInteractiveLogonLuid -or
        $LeaseRecord.interactive_session_binding_sha256 -cne $script:RepositoryLeaseInteractiveSessionBindingSha256 -or
        $LeaseRecord.state_store_scope -cne $script:RepositoryLeaseStateStoreScope) {
        throw "$Stage FOREIGN_INTERACTIVE_SESSION_HARD_STOP: fixture scope differs from the held Local lease scope"
    }
    $CurrentProcessSessionId = Get-RepositoryLeaseCurrentInteractiveSessionId -Stage "$Stage-current-process-session"
    if ($CurrentProcessSessionId -ne $script:RepositoryLeaseInteractiveSessionId) {
        throw "$Stage FOREIGN_INTERACTIVE_SESSION_HARD_STOP: current-process session differs from the fixture scope"
    }
}

function Get-LocalProofFixtureStatePath {
    param(
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$RootHash,
        [Parameter(Mandatory)] [string]$Stage
    )

    $BindingPrefix = $script:RepositoryLeaseInteractiveSessionBindingSha256.Substring(0, 24)
    $SessionSegment = "$($script:RepositoryLeaseInteractiveSessionId)-$BindingPrefix"
    $FixtureStateDir = Join-Path $ProofRunRoot (Join-Path 'fixture-state' (Join-Path $script:RepositoryLeasePrincipalSidHash $SessionSegment))
    $FixtureStatePath = Join-Path $FixtureStateDir "$RootHash.json"
    $ExpectedRelative = "fixture-state\$($script:RepositoryLeasePrincipalSidHash)\$SessionSegment\$RootHash.json"
    $ActualRelative = [System.IO.Path]::GetRelativePath($ProofRunRoot, $FixtureStatePath)
    if ($ActualRelative -cne $ExpectedRelative) {
        throw "$Stage fixture-state path is not the exact canonical fixture-state location"
    }
    return $FixtureStatePath
}

function New-LocalProofFixtureStateAdapter {
    param(
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$CanonicalRepositoryRoot,
        [Parameter(Mandatory)] [string]$ContractSha256,
        [Parameter(Mandatory)] $RepositoryLease,
        [Parameter(Mandatory)] [string]$Stage
    )

    $LeaseRecord = Assert-RepositoryMutationLease -Lease $RepositoryLease -CanonicalRepositoryRoot $CanonicalRepositoryRoot -Stage "$Stage-fixture-adapter-lease"
    Assert-FixtureScopeMatchesLease -Stage "$Stage-fixture-adapter-scope" -LeaseRecord $LeaseRecord
    if ($ContractSha256 -cnotmatch '^[0-9A-F]{64}$') {
        throw "$Stage fixture adapter received a malformed contract SHA-256"
    }
    $CanonicalRunRoot = ConvertTo-CanonicalAbsolutePath -Path $ProofRunRoot
    $RootHash = Get-ByteSha256 -Bytes (([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($CanonicalRunRoot))
    $FixtureStatePath = Get-LocalProofFixtureStatePath -ProofRunRoot $CanonicalRunRoot -RootHash $RootHash -Stage $Stage
    $FixtureStateDir = [System.IO.Path]::GetDirectoryName($FixtureStatePath)
    if (-not [System.IO.Directory]::Exists($FixtureStateDir)) {
        [System.IO.Directory]::CreateDirectory($FixtureStateDir) | Out-Null
        $Acl = Get-ProtectedCurrentUserAcl -Stage "$Stage-fixture-state-acl"
        Set-Acl -LiteralPath $FixtureStateDir -AclObject $Acl
    }

    $Adapter = [pscustomobject][ordered]@{
        fixture_state_path = $FixtureStatePath
        root_hash = $RootHash
        contract_sha256 = $ContractSha256
        lease_id = $LeaseRecord.lease_id
        scope = 'local-current-user-interactive-session'
    }
    $Adapter | Add-Member -MemberType ScriptMethod -Name 'ReadState' -Value {
        if (-not [System.IO.File]::Exists($this.fixture_state_path)) { return $null }
        $Bytes = [System.IO.File]::ReadAllBytes($this.fixture_state_path)
        $Json = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($Bytes)
        $Record = $Json | ConvertFrom-Json -Depth 8
        if ($Record.contract_sha256 -cne $this.contract_sha256 -or $Record.state_store_scope -cne $this.scope) {
            throw "$($this.contract_sha256.Substring(0, 8)) FOREIGN_INTERACTIVE_SESSION_HARD_STOP: fixture state record scope or contract differs"
        }
        return $Record
    }
    $Adapter | Add-Member -MemberType ScriptMethod -Name 'WriteState' -Value {
        param([Parameter(Mandatory)] $Record)
        $Json = $Record | ConvertTo-Json -Depth 8 -Compress
        $Bytes = ([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($Json)
        $Directory = [System.IO.Path]::GetDirectoryName($this.fixture_state_path)
        if (-not [System.IO.Directory]::Exists($Directory)) {
            [System.IO.Directory]::CreateDirectory($Directory) | Out-Null
            $Acl = Get-ProtectedCurrentUserAcl -Stage 'fixture-state-write-acl'
            Set-Acl -LiteralPath $Directory -AclObject $Acl
        }
        $Stream = [System.IO.File]::Open($this.fixture_state_path,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::Read)
        try {
            $Stream.Write($Bytes, 0, $Bytes.Length)
            $Stream.Flush($true)
        }
        finally {
            $Stream.Dispose()
        }
    }
    return $Adapter
}

function Write-LocalProofFixtureHardStop {
    param(
        [Parameter(Mandatory)] $Adapter,
        [Parameter(Mandatory)] [string]$ContractSha256,
        [Parameter(Mandatory)] [string]$JobName,
        [Parameter(Mandatory)] [string]$ChildPid,
        [Parameter(Mandatory)] [string]$Stage
    )

    $Record = [pscustomobject][ordered]@{
        state = 'unconfirmed-tree-termination'
        contract_sha256 = $ContractSha256
        root_hash = $Adapter.root_hash
        kernel_namespace = $script:RepositoryLeaseKernelObjectNamespace
        principal_sid = $script:RepositoryLeasePrincipalSid
        machine_name = $script:RepositoryLeaseMachineName
        interactive_session_id = $script:RepositoryLeaseInteractiveSessionId
        interactive_logon_luid = $script:RepositoryLeaseInteractiveLogonLuid
        interactive_session_binding_sha256 = $script:RepositoryLeaseInteractiveSessionBindingSha256
        state_store_scope = $script:RepositoryLeaseStateStoreScope
        job_name = $JobName
        child_pid = $ChildPid
        written_utc = [DateTime]::UtcNow.ToString('O')
    }
    $Adapter.WriteState($Record)
    return $Record
}

function Confirm-LocalProofFixtureHardStopCleared {
    param(
        [Parameter(Mandatory)] $Adapter,
        [Parameter(Mandatory)] [string]$ContractSha256,
        [Parameter(Mandatory)] [string]$Stage
    )

    $Record = $Adapter.ReadState()
    if ($null -eq $Record) {
        throw "$Stage no fixture hard-stop record exists"
    }
    if ($Record.state -cne 'unconfirmed-tree-termination') {
        throw "$Stage fixture record is not an unconfirmed-tree-termination record"
    }
    $JobName = [string]$Record.job_name
    if ([string]::IsNullOrWhiteSpace($JobName) -or $JobName -cnotmatch '^Local\\AgentsCommander-1283-cbm-[0-9A-F-]+$') {
        throw "$Stage fixture Job name is not the exact Local proof-scope Job grammar"
    }
    $LiveCount = Get-LocalProofFixtureJobActiveCount -JobName $JobName -Stage "$Stage-live-job-query"
    if ($LiveCount -gt 0) {
        return [pscustomobject][ordered]@{
            state = 'live-job-blocked'
            job_name = $JobName
            active_process_count = $LiveCount
            contract_sha256 = $ContractSha256
            clearance = 'blocked-live-job'
        }
    }
    return [pscustomobject][ordered]@{
        state = 'cleared'
        job_name = $JobName
        contract_sha256 = $ContractSha256
        kernel_namespace = $script:RepositoryLeaseKernelObjectNamespace
        principal_sid = $script:RepositoryLeasePrincipalSid
        machine_name = $script:RepositoryLeaseMachineName
        interactive_session_id = $script:RepositoryLeaseInteractiveSessionId
        interactive_logon_luid = $script:RepositoryLeaseInteractiveLogonLuid
        interactive_session_binding_sha256 = $script:RepositoryLeaseInteractiveSessionBindingSha256
        state_store_scope = $script:RepositoryLeaseStateStoreScope
        clearance_session_id = $script:RepositoryLeaseInteractiveSessionId
        cleared_utc = [DateTime]::UtcNow.ToString('O')
    }
}

function Get-ProcessCreationFileTime {
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
      IntPtr h = OpenProcess(0x0400 | 0x0010, false, pid); // QUERY_LIMITED_INFORMATION | SYNCHRONIZE
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
    $Creation = $Type::GetCreationFileTime($ProcessId)
    if ($Creation -cnotmatch '^[0-9A-F]{16}$') { throw "$Stage cannot read the process creation FILETIME" }
    return $Creation
}

function Assert-LocalProofCoordinatorOwnerIdentity {
    param(
        [Parameter(Mandatory)] [string]$ProofOwnerPath,
        [Parameter(Mandatory)] [string]$Stage
    )

    if (-not [System.IO.File]::Exists($ProofOwnerPath)) {
        throw "$Stage FAIL_NOT_COORDINATOR_OWNER: owner record is absent"
    }
    $Bytes = [System.IO.File]::ReadAllBytes($ProofOwnerPath)
    $OwnerJson = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($Bytes)
    $OwnerRecord = $OwnerJson | ConvertFrom-Json -Depth 8
    $OriginalPid = [int]$OwnerRecord.coordinator_pid
    $OriginalCreation = [string]$OwnerRecord.coordinator_process_creation_filetime
    if ($OriginalPid -le 0 -or [string]::IsNullOrWhiteSpace($OriginalCreation) -or $OriginalCreation -cnotmatch '^[0-9A-F]{16}$') {
        throw "$Stage FAIL_NOT_COORDINATOR_OWNER: owner record has no valid PID/creation identity"
    }
    $CurrentPid = $PID
    $CurrentCreation = Get-ProcessCreationFileTime -ProcessId $CurrentPid -Stage "$Stage-current-caller-creation"
    if ($CurrentPid -ne $OriginalPid -or $CurrentCreation -cne $OriginalCreation) {
        throw "$Stage FAIL_NOT_COORDINATOR_OWNER: caller identity does not match the original coordinator record"
    }
    return [pscustomobject][ordered]@{
        owner_name = [string]$OwnerRecord.owner_name
        coordinator_pid = $OriginalPid
        coordinator_process_creation_filetime = $OriginalCreation
    }
}

function Load-ProofJobInterop {
    param([Parameter(Mandatory)] [string]$Stage)

    if ('AgentsCommander.Review1283.ProofJobInterop' -as [type]) { return }
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace AgentsCommander.Review1283 {
  public static class ProofJobInterop {
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern IntPtr CreateJobObjectW(ref SecurityAttributes attrs, string name);
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern IntPtr OpenJobObject(uint access, bool inherit, string name);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool QueryInformationJobObject(IntPtr h, int infoClass, IntPtr info, uint len, out uint retLen);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern bool TerminateJobObject(IntPtr h, uint exitCode);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [StructLayout(LayoutKind.Sequential)] public struct SecurityAttributes { public int nLength; public IntPtr lpSecurityDescriptor; public bool bInheritHandle; }
    [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern bool ConvertStringSecurityDescriptorToSecurityDescriptor(string sddl, uint revision, out IntPtr sd, out uint size);
    [DllImport("kernel32.dll")] public static extern bool LocalFree(IntPtr h);
    private static IntPtr BuildProtectedDescriptor(string currentSid) {
      // One full-control rule for the current user only, DACL protected; this is the
      // exact SDDL-protected fixture-Job identity required by Section 22.1.0.b.
      string sddl = "D:(A;;GA;;;" + currentSid + ")";
      IntPtr sd; uint size;
      if (!ConvertStringSecurityDescriptorToSecurityDescriptor(sddl, 1, out sd, out size))
        throw new InvalidOperationException("ConvertStringSecurityDescriptorToSecurityDescriptor failed: " + Marshal.GetLastWin32Error());
      return sd;
    }
    public static IntPtr CreateJob(string jobName, string currentSid) {
      IntPtr sd = BuildProtectedDescriptor(currentSid);
      try {
        SecurityAttributes sa = new SecurityAttributes();
        sa.nLength = Marshal.SizeOf<SecurityAttributes>();
        sa.lpSecurityDescriptor = sd;
        sa.bInheritHandle = false;
        IntPtr h = CreateJobObjectW(ref sa, jobName);
        if (h == IntPtr.Zero) throw new InvalidOperationException("CreateJobObjectW failed: " + Marshal.GetLastWin32Error());
        return h;
      } finally { LocalFree(sd); }
    }
    public static bool AssignProcess(IntPtr job, int pid) {
      IntPtr p = OpenProcess(0x0001 | 0x0002 | 0x0400, false, pid);
      if (p == IntPtr.Zero) throw new InvalidOperationException("OpenProcess for assignment failed: " + Marshal.GetLastWin32Error());
      try { return AssignProcessToJobObject(job, p); } finally { CloseHandle(p); }
    }
    [StructLayout(LayoutKind.Sequential)] public struct StartupInfo { public int cb; public string lpReserved; public string lpDesktop; public string lpTitle; public int dwX; public int dwY; public int dwXSize; public int dwYSize; public int dwXCountChars; public int dwYCountChars; public int dwFillAttribute; public int dwFlags; public short wShowWindow; public short cbReserved2; public IntPtr lpReserved2; public IntPtr hStdInput; public IntPtr hStdOutput; public IntPtr hStdError; }
    [StructLayout(LayoutKind.Sequential)] public struct ProcessInformation { public IntPtr hProcess; public IntPtr hThread; public int dwProcessId; public int dwThreadId; }
    [DllImport("kernel32.dll", CharSet=CharSet.Unicode, SetLastError=true)] public static extern bool CreateProcessW(string app, string cmdline, IntPtr attrs, IntPtr threadAttrs, bool inherit, uint flags, IntPtr env, string cwd, ref StartupInfo si, out ProcessInformation pi);
    [DllImport("kernel32.dll", SetLastError=true)] public static extern uint ResumeThread(IntPtr thread);
    private static IntPtr _suspendedProcess = IntPtr.Zero;
    private static IntPtr _suspendedThread = IntPtr.Zero;
    private static int _suspendedPid = 0;
    public static int StartSuspendedChild(string commandLine) {
      StartupInfo si = new StartupInfo(); si.cb = Marshal.SizeOf<StartupInfo>(); si.dwFlags = 0x00000001;
      ProcessInformation pi;
      if (!CreateProcessW(null, commandLine, IntPtr.Zero, IntPtr.Zero, false, 0x08000004, IntPtr.Zero, null, ref si, out pi))
        throw new InvalidOperationException("CreateProcessW failed: " + Marshal.GetLastWin32Error());
      _suspendedProcess = pi.hProcess; _suspendedThread = pi.hThread; _suspendedPid = pi.dwProcessId;
      return pi.dwProcessId;
    }
    public static bool AssignSuspendedProcess(IntPtr job) {
      if (_suspendedProcess == IntPtr.Zero) throw new InvalidOperationException("no suspended child process handle");
      return AssignProcessToJobObject(job, _suspendedProcess);
    }
    public static void ResumeSuspendedChild() {
      if (_suspendedThread == IntPtr.Zero) throw new InvalidOperationException("no suspended child thread");
      uint result = ResumeThread(_suspendedThread);
      if (result == 0xFFFFFFFF) throw new InvalidOperationException("ResumeThread failed: " + Marshal.GetLastWin32Error());
    }
    public static int SuspendedPid { get { return _suspendedPid; } }
    public static void CloseSuspendedHandles() {
      if (_suspendedProcess != IntPtr.Zero) { CloseHandle(_suspendedProcess); _suspendedProcess = IntPtr.Zero; }
      if (_suspendedThread != IntPtr.Zero) { CloseHandle(_suspendedThread); _suspendedThread = IntPtr.Zero; }
      _suspendedPid = 0;
    }
    public static long QueryActiveProcessCount(string jobName) {
      IntPtr h = OpenJobObject(0x0004, false, jobName); // JOB_OBJECT_QUERY
      if (h == IntPtr.Zero) {
        // ERROR_FILE_NOT_FOUND (2) means the exact named Job no longer exists, which
        // after KILL_ON_JOB_CLOSE proves zero member processes. Any other error is a
        // real query failure and must not be read as a cleared state.
        int lastError = Marshal.GetLastWin32Error();
        if (lastError == 2 || lastError == 1168) return 0; // not found / not found
        return -1;
      }
      try {
        // JOBOBJECT_BASIC_ACCOUNTING_INFORMATION: four FILETIMEs (32 bytes), then
        // TotalPageFaultCount (4), TotalProcesses (4), ActiveProcesses (4) at offset 40.
        IntPtr buffer = Marshal.AllocHGlobal(48);
        try {
          uint retLen;
          if (!QueryInformationJobObject(h, 1, buffer, 48, out retLen)) return -1;
          int active = Marshal.ReadInt32(buffer, 40);
          return active;
        } finally { Marshal.FreeHGlobal(buffer); }
      } finally { CloseHandle(h); }
    }
    public static bool TerminateJob(string jobName) {
      // MAXIMUM_ALLOWED is required: a specific-rights reopen (even TERMINATE|QUERY)
      // yields a handle on which TerminateJobObject fails with ERROR_ACCESS_DENIED.
      IntPtr h = OpenJobObject(0x02000000, false, jobName);
      if (h == IntPtr.Zero) throw new InvalidOperationException("OpenJobObject(MAXIMUM_ALLOWED) failed for " + jobName + ": " + Marshal.GetLastWin32Error());
      try {
        bool ok = TerminateJobObject(h, 1);
        if (!ok) throw new InvalidOperationException("TerminateJobObject failed: " + Marshal.GetLastWin32Error());
        return true;
      } finally { CloseHandle(h); }
    }
  }
}
'@ -ErrorAction Stop | Out-Null
    if ($null -eq ('AgentsCommander.Review1283.ProofJobInterop' -as [type])) {
        throw "$Stage cannot load the fixture Job interop"
    }
}

function New-LocalProofFixtureJob {
    param(
        [Parameter(Mandatory)] [string]$JobName,
        [Parameter(Mandatory)] [string]$Stage
    )

    Load-ProofJobInterop -Stage $Stage
    $Type = 'AgentsCommander.Review1283.ProofJobInterop' -as [type]
    $Sid = [System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    if ([string]::IsNullOrWhiteSpace($Sid)) {
        throw "$Stage cannot establish the current-user SID for the protected fixture Job"
    }
    return $Type::CreateJob($JobName, $Sid)
}

function Start-LocalProofFixtureChildSuspended {
    param(
        [Parameter(Mandatory)] [string]$Stage
    )

    Load-ProofJobInterop -Stage $Stage
    $Type = 'AgentsCommander.Review1283.ProofJobInterop' -as [type]
    $ChildPid = $Type::StartSuspendedChild('pwsh.exe -NoLogo -NoProfile -NonInteractive -Command Start-Sleep -Seconds 120')
    if ($ChildPid -le 0) {
        throw "$Stage could not start the suspended fixture child"
    }
    return $ChildPid
}

function Resume-LocalProofFixtureChild {
    param(
        [Parameter(Mandatory)] [int]$ProcessId,
        [Parameter(Mandatory)] [string]$Stage
    )

    Load-ProofJobInterop -Stage $Stage
    $Type = 'AgentsCommander.Review1283.ProofJobInterop' -as [type]
    if ($Type::SuspendedPid -ne $ProcessId) {
        throw "$Stage suspended child PID differs from the recorded fixture child"
    }
    $Type::ResumeSuspendedChild()
}

function Close-LocalProofFixtureChildHandles {
    param([Parameter(Mandatory)] [string]$Stage)

    Load-ProofJobInterop -Stage $Stage
    $Type = 'AgentsCommander.Review1283.ProofJobInterop' -as [type]
    $Type::CloseSuspendedHandles()
}

function Add-LocalProofFixtureProcessToJob {
    param(
        [Parameter(Mandatory)] $JobHandle,
        [Parameter(Mandatory)] [int]$ProcessId,
        [Parameter(Mandatory)] [string]$Stage
    )

    Load-ProofJobInterop -Stage $Stage
    $Type = 'AgentsCommander.Review1283.ProofJobInterop' -as [type]
    if ($Type::SuspendedPid -ne $ProcessId) {
        throw "$Stage fixture child PID differs from the suspended child record"
    }
    $Assigned = $Type::AssignSuspendedProcess($JobHandle)
    if (-not $Assigned) {
        throw "$Stage could not assign the suspended fixture child to its exact Local Job"
    }
}

function Get-LocalProofFixtureJobActiveCount {
    param(
        [Parameter(Mandatory)] [string]$JobName,
        [Parameter(Mandatory)] [string]$Stage
    )

    Load-ProofJobInterop -Stage $Stage
    $Type = 'AgentsCommander.Review1283.ProofJobInterop' -as [type]
    $ActiveCount = $Type::QueryActiveProcessCount($JobName)
    if ($ActiveCount -lt 0) {
        throw "$Stage cannot open or query the exact Local fixture Job"
    }
    return [long]$ActiveCount
}


function Invoke-LocalProofCoordinatorCleanup {
    param(
        [Parameter(Mandatory)] [string]$ProofRunRoot,
        [Parameter(Mandatory)] [string]$ProofOwnerPath,
        [Parameter(Mandatory)] [string]$ProofContractPath,
        [Parameter(Mandatory)] [string]$ProofContractSha256,
        [Parameter(Mandatory)] [string]$FailureRecordPath,
        [Parameter(Mandatory)] [int]$CleanupTimeoutSeconds,
        [Parameter(Mandatory)] [string]$Stage
    )

    # Owner-only authority: current-process identity must match the protected owner
    # record before ANY cleanup enumeration, fixture-state access, Job open, process
    # control, root mutation, or peer observation.
    $OwnerIdentity = Assert-LocalProofCoordinatorOwnerIdentity -ProofOwnerPath $ProofOwnerPath -Stage "$Stage-owner-identity"

    $CanonicalRunRoot = ConvertTo-CanonicalAbsolutePath -Path $ProofRunRoot
    if (-not [System.IO.Directory]::Exists($CanonicalRunRoot)) {
        throw "$Stage cleanup run root is absent"
    }
    $ContractBytes = [System.IO.File]::ReadAllBytes($ProofContractPath)
    $ActualContractSha256 = Get-ByteSha256 -Bytes $ContractBytes
    if ($ActualContractSha256 -cne $ProofContractSha256) {
        throw "$Stage contract hash differs from the recorded contract"
    }

    # The injected Holder-failure scenario: validate the exact failure record before
    # opening any Job, then terminate only that exact Local fixture Job.
    $FailureRecord = $null
    if (-not [string]::IsNullOrWhiteSpace($FailureRecordPath) -and [System.IO.File]::Exists($FailureRecordPath)) {
        $FailureBytes = [System.IO.File]::ReadAllBytes($FailureRecordPath)
        $FailureJson = ([System.Text.UTF8Encoding]::new($false, $true)).GetString($FailureBytes)
        $FailureRecord = $FailureJson | ConvertFrom-Json -Depth 8
        if ($FailureRecord.contract_sha256 -cne $ProofContractSha256 -or
            [string]::IsNullOrWhiteSpace([string]$FailureRecord.job_name) -or
            [string]$FailureRecord.job_name -cnotmatch '^Local\\AgentsCommander-1283-cbm-[0-9A-F-]+$') {
            throw "$Stage CLEANUP_FAILED_DURABLE_HARD_STOP: failure record bindings are invalid"
        }
        if (-not ('AgentsCommander.Review1283.ProofJobInterop' -as [type])) {
            Load-ProofJobInterop -Stage "$Stage-interop-load"
        }
        $Terminated = [AgentsCommander.Review1283.ProofJobInterop]::TerminateJob($FailureRecord.job_name)
        if (-not $Terminated) {
            throw "$Stage CLEANUP_FAILED_DURABLE_HARD_STOP: could not open or terminate the exact Local fixture Job"
        }
        $Deadline = [DateTime]::UtcNow.AddSeconds($CleanupTimeoutSeconds)
        $ActiveCount = -1
        while ([DateTime]::UtcNow -lt $Deadline) {
            $ActiveCount = [AgentsCommander.Review1283.ProofJobInterop]::QueryActiveProcessCount($FailureRecord.job_name)
            if ($ActiveCount -le 0) { break }
            Start-Sleep -Milliseconds 100
        }
        if ($ActiveCount -gt 0) {
            throw "$Stage CLEANUP_FAILED_DURABLE_HARD_STOP: fixture Job still has active processes"
        }
        $CleanupRecordRootHash = Get-ByteSha256 -Bytes (([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($CanonicalRunRoot))
        $CleanupRecord = [pscustomobject][ordered]@{
            state = 'coordinator-holder-failure-cleanup'
            contract_sha256 = $ProofContractSha256
            root_hash = $CleanupRecordRootHash
            job_name = [string]$FailureRecord.job_name
            child_pid = [string]$FailureRecord.child_pid
            active_process_count_after = 0
            coordinator_pid = $OwnerIdentity.coordinator_pid
            cleaned_utc = [DateTime]::UtcNow.ToString('O')
        }
        $ResultsDir = Join-Path $CanonicalRunRoot 'results'
        if (-not [System.IO.Directory]::Exists($ResultsDir)) { [System.IO.Directory]::CreateDirectory($ResultsDir) | Out-Null }
        $CleanupJson = $CleanupRecord | ConvertTo-Json -Depth 8
        $CleanupBytes = ([System.Text.UTF8Encoding]::new($false, $true)).GetBytes($CleanupJson)
        $CleanupPath = Join-Path $ResultsDir 'coordinator-holder-failure-cleanup.json'
        $Stream = [System.IO.File]::Open($CleanupPath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::Read)
        try {
            $Stream.Write($CleanupBytes, 0, $CleanupBytes.Length)
            $Stream.Flush($true)
        }
        finally { $Stream.Dispose() }
    }

    # Remove the exact run root after the cleanup record is flushed. Never touch
    # production durable state or any unrecorded Job/process.
    [System.IO.Directory]::Delete($CanonicalRunRoot, $true)
    if ([System.IO.Directory]::Exists($CanonicalRunRoot)) {
        throw "$Stage CLEANUP_FAILED_DURABLE_HARD_STOP: run root survived cleanup"
    }
    return [pscustomobject][ordered]@{
        state = 'run-root-removed'
        proof_run_root = $CanonicalRunRoot
        coordinator_pid = $OwnerIdentity.coordinator_pid
        failure_cleanup_recorded = ($null -ne $FailureRecord)
        job_name = if ($null -ne $FailureRecord) { [string]$FailureRecord.job_name } else { $null }
        removed_utc = [DateTime]::UtcNow.ToString('O')
    }
}



# ---------------------------------------------------------------------------
# Step 8 exports (Section 22.1.0.a table). The lease helpers, the LocalProofFixture
# adapter, hard-stop, owner-identity, and cleanup functions are the module's public
# surface; the native control route remains private to the module construction
# scope and is never exported.
# ---------------------------------------------------------------------------
Export-ModuleMember -Function @(
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
)
