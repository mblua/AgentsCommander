use super::*;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

mod blocking_owned;
mod canonical_guard;
mod catalog_publication;
mod scope_mutation;
mod stage_e_conformance;
mod temp_inventory;
mod v1_upgrade;
mod windows_replace;
mod windows_replace_matrix;
mod wire_format;

fn timestamp(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .expect("valid test timestamp")
        .with_timezone(&Utc)
}

fn context_row(
    filename: &str,
    scope: &str,
    at: &str,
) -> Result<PublishedManifestRow, SeedManifestError> {
    PublishedManifestRow::project_context(
        ManifestPathIdentity::parse(ManifestPathEncoding::Utf8, format!(".ac/{filename}"))?,
        scope,
        timestamp(at),
    )
}

fn config_path(workgroup: &str, agent: &str, dest: &str, suffix: &str) -> ManifestPathIdentity {
    ManifestPathIdentity::parse(
        ManifestPathEncoding::Utf8,
        format!(".ac/{workgroup}/__agent_{agent}/{dest}/{suffix}"),
    )
    .expect("valid config path")
}

fn config_scope(workgroup: &str, agent: &str, dest: &str) -> String {
    format!("config:.ac/{workgroup}/__agent_{agent}/{dest}")
}

fn setup_project() -> (tempfile::TempDir, PathBuf) {
    // #1883 — install the shared crate logger instead of a raw
    // `env_logger` builder, so the #264 error sink is the process-wide
    // one the mailbox tests also probe.
    crate::logging::init_logger();
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    std::fs::create_dir_all(project.join(".ac")).expect("create project .ac");
    (temp, project)
}

fn canonical_path(project: &Path) -> PathBuf {
    project.join(".ac").join(SEED_MANIFEST_FILENAME)
}

fn read_disk_state(project: &Path) -> ManifestState {
    let bytes = std::fs::read(canonical_path(project)).expect("read canonical manifest");
    parse_manifest_bytes(&bytes).expect("parse canonical manifest")
}

fn serialize_raw_legacy_fixture(state: &ManifestState) -> Vec<u8> {
    let wire = SeedManifestWire {
        schema_version: SCHEMA_VERSION,
        coverage_version: COVERAGE_VERSION,
        coverage: COVERAGE.map(str::to_string).to_vec(),
        files: state
            .rows
            .values()
            .map(PublishedManifestRow::to_wire)
            .collect(),
    };
    let body = toml::to_string(&wire).expect("serialize raw current metadata");
    let body = body.trim_end_matches('\n');
    let mut bytes = Vec::with_capacity(MANAGED_HEADER.len() + body.len() + 1);
    bytes.extend_from_slice(MANAGED_HEADER.as_bytes());
    bytes.extend_from_slice(body.as_bytes());
    bytes.push(b'\n');
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.ends_with(b"\n\n"));
    bytes
}

fn exact_temp_paths(project: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(project.join(".ac"))
        .expect("read .ac")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .filter(|name| is_exact_temp_name(name))
                .map(|_| entry.path())
        })
        .collect()
}

#[cfg(windows)]
fn windows_ads_path(path: &Path, name: &str) -> PathBuf {
    let mut stream = path.as_os_str().to_os_string();
    stream.push(":");
    stream.push(name);
    PathBuf::from(stream)
}

#[cfg(windows)]
fn windows_get_dacl(path: &Path) -> Vec<u8> {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_INSUFFICIENT_BUFFER};
    use windows_sys::Win32::Security::{GetFileSecurityW, DACL_SECURITY_INFORMATION};

    let wide = absolute_verbatim_utf16(path).expect("verbatim path");
    let mut needed = 0_u32;
    // SAFETY: `wide` comes from absolute_verbatim_utf16 and stays NUL-terminated.
    // GetFileSecurityW allows null storage with size zero; `needed` is writable.
    let first = unsafe {
        GetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            0,
            &mut needed,
        )
    };
    assert_eq!(first, 0);
    // SAFETY: GetLastError takes no pointers and reads the thread-local error left
    // by the failed GetFileSecurityW; the intervening assertion succeeded.
    let raw_error = unsafe { GetLastError() };
    assert_eq!(raw_error, ERROR_INSUFFICIENT_BUFFER);
    let mut descriptor = vec![0_u8; usize::try_from(needed).expect("descriptor size")];
    // SAFETY: `descriptor` has exactly `needed` writable bytes and remains live along
    // with `wide` and the size output. The test allocator delegates to System (HeapAlloc on
    // Windows), whose allocation alignment suffices for a security descriptor; see
    // src-tauri/src/pty/terminal_snapshot/tests/resource.rs:82 and :109.
    let success = unsafe {
        GetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION,
            descriptor.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    assert_ne!(
        success,
        0,
        "GetFileSecurityW: {}",
        io::Error::last_os_error()
    );
    descriptor.truncate(usize::try_from(needed).expect("descriptor size"));
    descriptor
}

#[cfg(windows)]
fn windows_protect_existing_dacl(path: &Path) -> Vec<u8> {
    use windows_sys::Win32::Security::{
        GetSecurityDescriptorControl, GetSecurityDescriptorDacl, InitializeSecurityDescriptor,
        SetFileSecurityW, SetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    };

    let mut acl = windows_world_access_acl();
    let acl_ptr = acl.as_mut_ptr().cast::<ACL>();

    let mut descriptor = std::mem::MaybeUninit::<SECURITY_DESCRIPTOR>::zeroed();
    // SAFETY: `descriptor` is aligned SECURITY_DESCRIPTOR storage. InitializeSecurityDescriptor
    // receives its writable address and revision 1, the supported security descriptor revision.
    let initialized = unsafe { InitializeSecurityDescriptor(descriptor.as_mut_ptr().cast(), 1) };
    assert_ne!(
        initialized,
        0,
        "InitializeSecurityDescriptor: {}",
        io::Error::last_os_error()
    );
    // SAFETY: The zeroed SECURITY_DESCRIPTOR has integer and raw-pointer fields with valid
    // zero representations; InitializeSecurityDescriptor succeeded and initialized it.
    let mut descriptor = unsafe { descriptor.assume_init() };
    // SAFETY: `descriptor` was successfully initialized; `acl_ptr` addresses the live ACL
    // returned by windows_world_access_acl. SetSecurityDescriptorDacl borrows that ACL; both
    // values remain live through SetFileSecurityW.
    let dacl_set = unsafe {
        SetSecurityDescriptorDacl(
            (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
            1,
            acl_ptr,
            0,
        )
    };
    assert_ne!(
        dacl_set,
        0,
        "SetSecurityDescriptorDacl: {}",
        io::Error::last_os_error()
    );
    descriptor.Control |= SE_DACL_PROTECTED;
    let wide = absolute_verbatim_utf16(path).expect("verbatim path");
    // SAFETY: `wide` is NUL-terminated by absolute_verbatim_utf16; `descriptor` is initialized
    // with its DACL pointing into the still-live `acl`. SetFileSecurityW only borrows them.
    let success = unsafe {
        SetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            (&mut descriptor as *mut SECURITY_DESCRIPTOR).cast(),
        )
    };
    assert_ne!(
        success,
        0,
        "SetFileSecurityW: {}",
        io::Error::last_os_error()
    );
    let mut persisted = windows_get_dacl(path);
    let mut control = 0_u16;
    let mut revision = 0_u32;
    // SAFETY: `persisted` is the successful GetFileSecurityW descriptor from windows_get_dacl.
    // GetSecurityDescriptorControl borrows it and writes the live control/revision outputs.
    let control_ok = unsafe {
        GetSecurityDescriptorControl(persisted.as_mut_ptr().cast(), &mut control, &mut revision)
    };
    assert_ne!(control_ok, 0);
    assert_ne!(control & SE_DACL_PROTECTED, 0);
    let mut present = 0;
    let mut defaulted = 0;
    let mut persisted_acl = std::ptr::null_mut();
    // SAFETY: `persisted` is the live descriptor returned by windows_get_dacl; the present,
    // DACL pointer and defaulted outputs are writable locals for GetSecurityDescriptorDacl.
    let dacl_ok = unsafe {
        GetSecurityDescriptorDacl(
            persisted.as_mut_ptr().cast(),
            &mut present,
            &mut persisted_acl,
            &mut defaulted,
        )
    };
    assert_ne!(dacl_ok, 0);
    assert_ne!(present, 0);
    assert!(!persisted_acl.is_null());
    // SAFETY: GetSecurityDescriptorDacl succeeded and returned a non-null ACL within the
    // still-live `persisted` descriptor. The Windows-produced ACL header is initialized and
    // DWORD-aligned, so its AclSize field can be read.
    let persisted_size = unsafe { usize::from((*persisted_acl).AclSize) };
    // SAFETY: `persisted_acl` is the non-null DACL returned by GetSecurityDescriptorDacl.
    // Its AclSize covers initialized bytes within the single live `persisted` allocation; the
    // immutable slice is copied before that allocation is dropped or mutated.
    unsafe { std::slice::from_raw_parts(persisted_acl.cast::<u8>(), persisted_size) }.to_vec()
}

#[cfg(windows)]
fn windows_dacl_bytes(path: &Path) -> Vec<u8> {
    use windows_sys::Win32::Security::GetSecurityDescriptorDacl;

    let mut descriptor = windows_get_dacl(path);
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = std::ptr::null_mut();
    // SAFETY: `descriptor` is the successful GetFileSecurityW result from windows_get_dacl.
    // GetSecurityDescriptorDacl only reads it and writes the three live output locals.
    let success = unsafe {
        GetSecurityDescriptorDacl(
            descriptor.as_mut_ptr().cast(),
            &mut present,
            &mut acl,
            &mut defaulted,
        )
    };
    assert_ne!(success, 0);
    assert_ne!(present, 0);
    assert!(!acl.is_null());
    // SAFETY: GetSecurityDescriptorDacl succeeded and `acl` was checked non-null. It points
    // to the initialized, DWORD-aligned ACL header in the still-live `descriptor`.
    let size = unsafe { usize::from((*acl).AclSize) };
    // SAFETY: `acl` is the checked non-null DACL within `descriptor`; its initialized
    // AclSize bytes belong to that single live allocation. No mutation occurs while the
    // immutable slice is copied into its own vector.
    unsafe { std::slice::from_raw_parts(acl.cast::<u8>(), size) }.to_vec()
}

#[cfg(windows)]
fn windows_set_compression(path: &Path, format: u16) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_COMPRESSION;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    let file = open_regular_no_follow(path, true, false).expect("open compression fixture");
    let mut returned = 0_u32;
    // SAFETY: `file` owns the live synchronous handle from open_regular_no_follow. The
    // FSCTL_SET_COMPRESSION input is a live u16 of the declared size; `returned` is writable,
    // and DeviceIoControl allows no output buffer and null OVERLAPPED for this handle.
    let success = unsafe {
        DeviceIoControl(
            file.as_raw_handle() as HANDLE,
            FSCTL_SET_COMPRESSION,
            (&format as *const u16).cast(),
            u32::try_from(std::mem::size_of::<u16>()).expect("u16 size"),
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        success,
        0,
        "DeviceIoControl: {}",
        io::Error::last_os_error()
    );
}

#[cfg(windows)]
fn windows_set_attributes(path: &Path, attributes: u32) {
    use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;

    let wide = absolute_verbatim_utf16(path).expect("verbatim path");
    // SAFETY: absolute_verbatim_utf16 produced the live NUL-terminated `wide` path.
    // SetFileAttributesW borrows that path and receives attribute bits by value.
    let success = unsafe { SetFileAttributesW(wide.as_ptr(), attributes) };
    assert_ne!(
        success,
        0,
        "SetFileAttributesW: {}",
        io::Error::last_os_error()
    );
}

fn populated_state() -> ManifestState {
    let mut state = ManifestState::default();
    let context = context_row(
        "Context.AgentsCommander.md",
        "context:agentscommander",
        "2026-07-16T19:40:07.123Z",
    )
    .expect("context row");
    let config = PublishedManifestRow::replica_config(
        config_path("wg-14-dev-team", "architect", ".claude", "settings.json"),
        config_scope("wg-14-dev-team", "architect", ".claude"),
        ManifestSource::WorkspaceBase,
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .expect("config row");
    let catalog = PublishedManifestRow::coding_agent_catalog(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/coding-agents/agents.json".to_string(),
        )
        .expect("catalog path"),
        timestamp("2026-07-16T19:42:00.789Z"),
    )
    .expect("catalog row");
    let _ = state.upsert(config);
    let _ = state.upsert(catalog);
    let _ = state.upsert(context);
    state
}

#[cfg(windows)]
fn windows_world_access_acl() -> Vec<u8> {
    use windows_sys::Win32::Security::{
        AddAccessAllowedAceEx, InitializeAcl, ACCESS_ALLOWED_ACE, ACL, ACL_REVISION,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    let world_sid = [1_u8, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
    let acl_length = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        - std::mem::size_of::<u32>()
        + world_sid.len();
    let mut acl = vec![0_u8; acl_length];
    let acl_ptr = acl.as_mut_ptr().cast::<ACL>();
    // SAFETY: `acl` reserves the ACL header plus one ACCESS_ALLOWED_ACE and the SID, with
    // DWORD-multiple length. Its test allocator delegates to System/HeapAlloc (see
    // src-tauri/src/pty/terminal_snapshot/tests/resource.rs:82 and :109), so it is DWORD-aligned;
    // InitializeAcl receives that entire live writable buffer and ACL_REVISION.
    let initialized = unsafe {
        InitializeAcl(
            acl_ptr,
            u32::try_from(acl.len()).expect("ACL size"),
            ACL_REVISION,
        )
    };
    assert_ne!(
        initialized,
        0,
        "InitializeAcl: {}",
        io::Error::last_os_error()
    );
    // SAFETY: InitializeAcl succeeded for `acl_ptr`, and `acl_length` includes this ACE
    // and all 12 SID bytes. `world_sid` encodes revision 1, one subauthority, authority 1 and
    // subauthority 0 (S-1-1-0); it stays live while AddAccessAllowedAceEx copies the SID.
    let added = unsafe {
        AddAccessAllowedAceEx(
            acl_ptr,
            ACL_REVISION,
            0,
            FILE_ALL_ACCESS,
            world_sid.as_ptr().cast_mut().cast(),
        )
    };
    assert_ne!(
        added,
        0,
        "AddAccessAllowedAceEx: {}",
        io::Error::last_os_error()
    );

    acl
}
