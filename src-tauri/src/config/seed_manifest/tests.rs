use super::*;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::sync::Arc;

#[tokio::test]
async fn blocking_owned_drop_before_first_poll_schedules_nothing() {
    struct DropProbe(Arc<AtomicBool>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.store(true, AtomicOrdering::Release);
        }
    }

    let ran = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicBool::new(false));
    let drop_probe = DropProbe(Arc::clone(&dropped));
    let future = run_blocking_owned({
        let ran = Arc::clone(&ran);
        move || {
            let _drop_probe = drop_probe;
            ran.store(true, AtomicOrdering::Release);
        }
    });
    drop(future);
    tokio::task::yield_now().await;
    assert!(!ran.load(AtomicOrdering::Acquire));
    assert!(dropped.load(AtomicOrdering::Acquire));
}

#[test]
fn blocking_owned_drop_while_queued_suppresses_user_work() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let (blocker_started_tx, blocker_started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            blocker_started_tx.send(()).expect("signal blocker");
            release_rx.recv().expect("release blocker");
        });
        blocker_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("blocking pool occupied");

        let ran = Arc::new(AtomicBool::new(false));
        let waiter = tokio::spawn(run_blocking_owned({
            let ran = Arc::clone(&ran);
            move || ran.store(true, AtomicOrdering::Release)
        }));
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        waiter.abort();
        let _ = waiter.await;
        release_tx.send(()).expect("release blocking pool");
        blocker.await.expect("join blocker");
        tokio::task::yield_now().await;
        assert!(!ran.load(AtomicOrdering::Acquire));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn blocking_owned_started_work_finishes_after_waiter_drop() {
    let started = Arc::new(AtomicBool::new(false));
    let completed = Arc::new(AtomicBool::new(false));
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let waiter = tokio::spawn(run_blocking_owned({
        let started = Arc::clone(&started);
        let completed = Arc::clone(&completed);
        move || {
            started.store(true, AtomicOrdering::Release);
            release_rx.recv().expect("release started work");
            completed.store(true, AtomicOrdering::Release);
        }
    }));
    while !started.load(AtomicOrdering::Acquire) {
        tokio::task::yield_now().await;
    }
    waiter.abort();
    let _ = waiter.await;
    release_tx.send(()).expect("release worker");
    tokio::time::timeout(Duration::from_secs(1), async {
        while !completed.load(AtomicOrdering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("started work must remain self-owned");
}

#[tokio::test]
async fn blocking_owned_surfaces_worker_panic_as_join_error() {
    let error = run_blocking_owned(|| -> () { panic!("injected blocking panic") })
        .await
        .expect_err("panic must surface as a join error");
    let BlockingTaskError::Join(join_error) = &error else {
        panic!("expected typed join error, got {error:?}");
    };
    assert!(join_error.is_panic());
    let source = std::error::Error::source(&error).expect("join error remains the source");
    assert!(source.downcast_ref::<tokio::task::JoinError>().is_some());
}

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
    let raw_error = unsafe { GetLastError() };
    assert_eq!(raw_error, ERROR_INSUFFICIENT_BUFFER);
    let mut descriptor = vec![0_u8; usize::try_from(needed).expect("descriptor size")];
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
        AddAccessAllowedAceEx, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        InitializeAcl, InitializeSecurityDescriptor, SetFileSecurityW, SetSecurityDescriptorDacl,
        ACCESS_ALLOWED_ACE, ACL, ACL_REVISION, DACL_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, SECURITY_DESCRIPTOR, SE_DACL_PROTECTED,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;

    let world_sid = [1_u8, 1, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0];
    let acl_length = std::mem::size_of::<ACL>() + std::mem::size_of::<ACCESS_ALLOWED_ACE>()
        - std::mem::size_of::<u32>()
        + world_sid.len();
    let mut acl = vec![0_u8; acl_length];
    let acl_ptr = acl.as_mut_ptr().cast::<ACL>();
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

    let mut descriptor = std::mem::MaybeUninit::<SECURITY_DESCRIPTOR>::zeroed();
    let initialized = unsafe { InitializeSecurityDescriptor(descriptor.as_mut_ptr().cast(), 1) };
    assert_ne!(
        initialized,
        0,
        "InitializeSecurityDescriptor: {}",
        io::Error::last_os_error()
    );
    let mut descriptor = unsafe { descriptor.assume_init() };
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
    let control_ok = unsafe {
        GetSecurityDescriptorControl(persisted.as_mut_ptr().cast(), &mut control, &mut revision)
    };
    assert_ne!(control_ok, 0);
    assert_ne!(control & SE_DACL_PROTECTED, 0);
    let mut present = 0;
    let mut defaulted = 0;
    let mut persisted_acl = std::ptr::null_mut();
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
    let persisted_size = unsafe { usize::from((*persisted_acl).AclSize) };
    unsafe { std::slice::from_raw_parts(persisted_acl.cast::<u8>(), persisted_size) }.to_vec()
}

#[cfg(windows)]
fn windows_dacl_bytes(path: &Path) -> Vec<u8> {
    use windows_sys::Win32::Security::GetSecurityDescriptorDacl;

    let mut descriptor = windows_get_dacl(path);
    let mut present = 0;
    let mut defaulted = 0;
    let mut acl = std::ptr::null_mut();
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
    let size = unsafe { usize::from((*acl).AclSize) };
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

#[test]
fn golden_empty_manifest_is_exact() {
    let bytes = serialize_state(&ManifestState::default()).expect("serialize empty");
    assert_eq!(
        bytes,
        concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 1\n",
            "coverage_version = 2\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
            "files = []\n"
        )
        .as_bytes()
    );
    assert!(!bytes.windows(2).any(|window| window == b"\r\n"));
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.ends_with(b"\n\n"));
}

#[test]
fn golden_populated_manifest_is_exact_sorted_and_omits_legacy_config() {
    let state = populated_state();
    assert_eq!(state.rows.len(), 3);
    assert_eq!(state.canonical_rows().count(), 2);
    let bytes = serialize_state(&state).expect("serialize populated");
    let expected = concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 1\n",
        "coverage_version = 2\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/Context.AgentsCommander.md\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"project_context_template\"\n",
        "scope = \"context:agentscommander\"\n",
        "source = \"builtin\"\n",
        "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/coding-agents/agents.json\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"coding_agent_catalog\"\n",
        "scope = \"catalog:coding-agents\"\n",
        "source = \"builtin\"\n",
        "last_seeded_at = \"2026-07-16T19:42:00.789Z\"\n"
    );
    assert_eq!(bytes, expected.as_bytes());
    assert_eq!(exact_serialized_len(&state).unwrap(), bytes.len());
    assert!(bytes.ends_with(b"\n"));
    assert!(!bytes.ends_with(b"\n\n"));
    assert!(!bytes
        .windows(b"replica_config_file".len())
        .any(|window| { window == b"replica_config_file" }));
    assert!(!String::from_utf8_lossy(&bytes)
        .contains(".ac/wg-14-dev-team/__agent_architect/.claude/settings.json"));
    let parsed = parse_manifest_bytes(&bytes).unwrap();
    assert_eq!(parsed.rows.len(), 2);
    assert!(parsed
        .rows
        .values()
        .all(|row| row.kind.is_canonical_output()));
    assert_eq!(serialize_state(&parsed).unwrap(), bytes);
}

#[test]
fn legacy_only_state_serializes_as_empty_canonical_manifest() {
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    assert_eq!(legacy.rows.len(), 1);
    assert_eq!(legacy.canonical_rows().count(), 0);
    let canonical = serialize_state(&legacy).expect("serialize canonical output");
    assert_eq!(
        canonical,
        serialize_state(&ManifestState::default()).unwrap()
    );
    assert_eq!(exact_serialized_len(&legacy).unwrap(), canonical.len());
    let parsed = parse_manifest_bytes(&canonical).unwrap();
    assert!(parsed.rows.is_empty());
    assert_eq!(serialize_state(&parsed).unwrap(), canonical);
}

#[test]
fn valid_legacy_replica_config_rows_remain_readable() {
    let scope = config_scope("wg-1-team", "alpha", ".claude");
    let source = ManifestSource::WorkspaceBase;
    let at = timestamp("2026-07-16T19:41:12.456Z");
    let mut legacy = ManifestState::default();
    for suffix in ["settings.json", "hooks.json"] {
        let _ = legacy.upsert(
            PublishedManifestRow::replica_config(
                config_path("wg-1-team", "alpha", ".claude", suffix),
                scope.clone(),
                source,
                at,
            )
            .unwrap(),
        );
    }
    let raw = serialize_raw_legacy_fixture(&legacy);
    let parsed = parse_manifest_bytes(&raw).expect("valid legacy rows remain readable");
    assert_eq!(parsed.rows.len(), 2);
    let paths = parsed
        .rows
        .values()
        .map(|row| row.path.serialized().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![
            ".ac/wg-1-team/__agent_alpha/.claude/hooks.json".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/settings.json".to_string(),
        ]
    );
    for row in parsed.rows.values() {
        assert_eq!(row.kind, ManifestFileKind::ReplicaConfigFile);
        assert_eq!(row.scope, scope);
        assert_eq!(row.source, source);
        assert_eq!(row.published_at, at);
    }
}

#[test]
fn insertion_order_does_not_change_bytes() {
    let first = populated_state();
    let mut second = ManifestState::default();
    for row in first.rows.values().rev() {
        let _ = second.upsert(row.clone());
    }
    assert_eq!(
        serialize_state(&first).unwrap(),
        serialize_state(&second).unwrap()
    );
}

#[test]
fn strict_wire_rejects_missing_unknown_duplicate_and_noncanonical_values() {
    let empty = String::from_utf8(serialize_state(&ManifestState::default()).unwrap()).unwrap();
    let without_files = empty.replace("files = []\n", "");
    assert!(matches!(
        parse_manifest_bytes(without_files.as_bytes()),
        Err(SeedManifestError::Parse(_))
    ));

    let unknown = empty.replace("schema_version = 1\n", "schema_version = 1\nextra = 1\n");
    assert!(matches!(
        parse_manifest_bytes(unknown.as_bytes()),
        Err(SeedManifestError::Parse(_))
    ));

    let duplicate = empty.replace(
        "schema_version = 1\n",
        "schema_version = 1\nschema_version = 1\n",
    );
    assert!(matches!(
        parse_manifest_bytes(duplicate.as_bytes()),
        Err(SeedManifestError::Parse(_))
    ));

    let wrong_coverage = empty.replace("coverage_version = 2", "coverage_version = 3");
    assert!(matches!(
        parse_manifest_bytes(wrong_coverage.as_bytes()),
        Err(SeedManifestError::Validation(_))
    ));

    let unknown_enum = String::from_utf8(serialize_state(&populated_state()).unwrap())
        .unwrap()
        .replace("source = \"builtin\"", "source = \"mystery\"");
    assert!(matches!(
        parse_manifest_bytes(unknown_enum.as_bytes()),
        Err(SeedManifestError::Parse(_))
    ));

    let noncanonical_time = String::from_utf8(serialize_state(&populated_state()).unwrap())
        .unwrap()
        .replace("2026-07-16T19:40:07.123Z", "2026-07-16T19:40:07.123+00:00");
    assert!(matches!(
        parse_manifest_bytes(noncanonical_time.as_bytes()),
        Err(SeedManifestError::Validation(_))
    ));
}

#[test]
fn strict_state_rejects_duplicate_identity_and_mixed_config_batches() {
    let row = PublishedManifestRow::replica_config(
        config_path("wg-1-team", "alpha", ".claude", "settings.json"),
        config_scope("wg-1-team", "alpha", ".claude"),
        ManifestSource::WorkspaceBase,
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap()
    .to_wire();
    let duplicate_wire = SeedManifestWire {
        schema_version: 1,
        coverage_version: 2,
        coverage: COVERAGE.map(str::to_string).to_vec(),
        files: vec![row.clone(), row.clone()],
    };
    assert!(matches!(
        ManifestState::from_wire(duplicate_wire),
        Err(SeedManifestError::Validation(_))
    ));

    let mut mixed = row;
    mixed.path = ".ac/wg-1-team/__agent_alpha/.claude/other.json".to_string();
    mixed.source = ManifestSource::MatrixBase;
    let mixed_wire = SeedManifestWire {
        schema_version: 1,
        coverage_version: 2,
        coverage: COVERAGE.map(str::to_string).to_vec(),
        files: vec![
            PublishedManifestRow::replica_config(
                config_path("wg-1-team", "alpha", ".claude", "settings.json"),
                config_scope("wg-1-team", "alpha", ".claude"),
                ManifestSource::WorkspaceBase,
                timestamp("2026-07-16T19:41:12.456Z"),
            )
            .unwrap()
            .to_wire(),
            mixed,
        ],
    };
    assert!(matches!(
        ManifestState::from_wire(mixed_wire),
        Err(SeedManifestError::Validation(_))
    ));

    let first = PublishedManifestRow::replica_config(
        config_path("wg-1-team", "alpha", ".claude", "settings.json"),
        config_scope("wg-1-team", "alpha", ".claude"),
        ManifestSource::WorkspaceBase,
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap()
    .to_wire();
    let mut different_time = first.clone();
    different_time.path = ".ac/wg-1-team/__agent_alpha/.claude/later.json".to_string();
    different_time.last_seeded_at = "2026-07-16T19:41:12.457Z".to_string();
    let mixed_time_wire = SeedManifestWire {
        schema_version: 1,
        coverage_version: 2,
        coverage: COVERAGE.map(str::to_string).to_vec(),
        files: vec![first, different_time],
    };
    assert!(matches!(
        ManifestState::from_wire(mixed_time_wire),
        Err(SeedManifestError::Validation(_))
    ));
}

#[test]
fn path_codecs_are_lossless_canonical_and_component_wise() {
    let readable = ManifestPathIdentity::parse(
        ManifestPathEncoding::Utf8,
        ".ac/wg-1-team/__agent_alpha/.claude/settings.json".to_string(),
    )
    .unwrap();
    assert_eq!(readable.encoding(), ManifestPathEncoding::Utf8);

    let mut unix_bytes = b".ac/wg-1-team/__agent_alpha/.claude/file".to_vec();
    unix_bytes.push(0xff);
    let unix = ManifestPathIdentity::parse(
        ManifestPathEncoding::UnixBytesHex,
        encode_lower_hex_bytes(&unix_bytes),
    )
    .unwrap();
    assert!(unix.starts_with_utf8_components(&[
        ".ac".to_string(),
        "wg-1-team".to_string(),
        "__agent_alpha".to_string(),
    ]));

    let mut windows_units = ".ac/wg-1-team/__agent_alpha/.claude/file"
        .encode_utf16()
        .collect::<Vec<_>>();
    windows_units.push(0xd800);
    let windows = ManifestPathIdentity::parse(
        ManifestPathEncoding::WindowsUtf16Hex,
        encode_lower_hex_u16(&windows_units),
    )
    .unwrap();
    assert!(windows.starts_with_utf8_components(&[
        ".ac".to_string(),
        "wg-1-team".to_string(),
        "__agent_alpha".to_string(),
    ]));
    assert_ne!(unix.key(), windows.key());

    let canonical_hex = encode_lower_hex_bytes(b".ac/readable");
    assert!(
        ManifestPathIdentity::parse(ManifestPathEncoding::UnixBytesHex, canonical_hex).is_err()
    );
    assert!(ManifestPathIdentity::parse(
        ManifestPathEncoding::UnixBytesHex,
        "2E6163ff".to_string()
    )
    .is_err());
    assert!(
        ManifestPathIdentity::parse(ManifestPathEncoding::Utf8, ".ac/../escape".to_string())
            .is_err()
    );
    assert!(
        ManifestPathIdentity::parse(ManifestPathEncoding::Utf8, "/.ac/absolute".to_string())
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn native_unix_invalid_byte_path_round_trips() {
    use std::os::unix::ffi::OsStringExt;

    let path = PathBuf::from(".ac").join(std::ffi::OsString::from_vec(vec![b'f', 0xff]));
    let identity = ManifestPathIdentity::from_relative_path(&path).unwrap();
    assert_eq!(identity.encoding(), ManifestPathEncoding::UnixBytesHex);
    let reparsed =
        ManifestPathIdentity::parse(identity.encoding(), identity.serialized().to_string())
            .unwrap();
    assert_eq!(identity, reparsed);
}

#[cfg(windows)]
#[test]
fn native_windows_unpaired_utf16_path_round_trips() {
    use std::os::windows::ffi::OsStringExt;

    let path = PathBuf::from(".ac").join(std::ffi::OsString::from_wide(&[u16::from(b'f'), 0xd800]));
    let identity = ManifestPathIdentity::from_relative_path(&path).unwrap();
    assert_eq!(identity.encoding(), ManifestPathEncoding::WindowsUtf16Hex);
    let reparsed =
        ManifestPathIdentity::parse(identity.encoding(), identity.serialized().to_string())
            .unwrap();
    assert_eq!(identity, reparsed);
}

#[test]
fn bounds_and_exact_outbound_counter_are_enforced() {
    let oversized = format!(".ac/{}", "x".repeat(MAX_FIELD_BYTES));
    assert!(matches!(
        ManifestPathIdentity::parse(ManifestPathEncoding::Utf8, oversized),
        Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::PathBytes,
            ..
        })
    ));

    let escaped = PublishedManifestRow::replica_config(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/wg-1-team/__agent_alpha/.claude/quote\"-slash\\-tab\t-del\u{7f}".to_string(),
        )
        .unwrap(),
        config_scope("wg-1-team", "alpha", ".claude"),
        ManifestSource::CatalogDefault,
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap();
    let mut state = ManifestState::default();
    let _ = state.upsert(escaped);
    let _ = state.upsert(
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    let serialized = serialize_state(&state).unwrap();
    assert!(!serialized.is_empty());
    assert_eq!(exact_serialized_len(&state).unwrap(), serialized.len());
    let output_limit = u64::try_from(serialized.len() - 1).unwrap();
    assert!(matches!(
        exact_serialized_len_with_limit(&state, output_limit),
        Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::OutputBytes,
            limit,
            ..
        }) if limit == output_limit
    ));

    let oversized_scope = format!("config:{}", "x".repeat(MAX_FIELD_BYTES));
    assert!(matches!(
        PublishedScopeBatch::new(
            oversized_scope,
            ManifestSource::WorkspaceBase,
            Vec::new(),
            timestamp("2026-07-16T19:41:12.456Z"),
        ),
        Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::ScopeBytes,
            ..
        })
    ));

    let mut total = usize::MAX;
    assert!(matches!(
        checked_add(&mut total, 1),
        Err(SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::ArithmeticOverflow,
            ..
        })
    ));
}

#[test]
fn bounded_reader_rejects_the_first_extra_byte() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(temp.path(), b"12345").unwrap();
    let mut file = File::open(temp.path()).unwrap();
    let error = read_bounded_file(&mut file, 5, 4, temp.path()).unwrap_err();
    assert!(matches!(
        error,
        SeedManifestError::ResourceBound {
            kind: ResourceBoundKind::InputBytes,
            limit: 4,
            observed_at_least: 5
        }
    ));
}

// -----------------------------------------------------------------------
// #1968 - the narrow read-only catalog-publication query.
// -----------------------------------------------------------------------

fn catalog_publication_row() -> Result<PublishedManifestRow, SeedManifestError> {
    PublishedManifestRow::coding_agent_catalog(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/coding-agents/agents.json".to_string(),
        )?,
        timestamp("2026-09-12T13:30:00.000Z"),
    )
}

fn ac_dir_entries(project: &Path) -> Vec<std::ffi::OsString> {
    let mut names: Vec<std::ffi::OsString> = std::fs::read_dir(project.join(".ac"))
        .expect("read .ac")
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    names.sort();
    names
}

#[test]
fn managed_catalog_has_catalog_publication_missing_manifest_is_false_and_creates_nothing() {
    let (_temp, project) = setup_project();
    let entries_before = ac_dir_entries(&project);
    assert!(!has_catalog_publication(&project).expect("query missing manifest"));
    assert!(
        !canonical_path(&project).exists(),
        "the query never creates"
    );
    assert_eq!(entries_before, ac_dir_entries(&project));
}

#[test]
fn managed_catalog_has_catalog_publication_matches_only_the_exact_canonical_row() {
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    guard.release();
    assert!(
        !has_catalog_publication(&project).expect("context-only manifest"),
        "a context row is not a catalog publication"
    );

    let bytes_before = std::fs::read(canonical_path(&project)).unwrap();
    let entries_before = ac_dir_entries(&project);
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            catalog_publication_row().unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    guard.release();
    assert!(has_catalog_publication(&project).expect("catalog row"));

    // Repeated reads change neither bytes nor the directory entry set.
    let bytes_after = std::fs::read(canonical_path(&project)).unwrap();
    let entries_after = ac_dir_entries(&project);
    for _ in 0..3 {
        assert!(has_catalog_publication(&project).expect("repeat query"));
    }
    assert_ne!(bytes_before, bytes_after, "the row was recorded");
    assert_eq!(
        std::fs::read(canonical_path(&project)).unwrap(),
        bytes_after
    );
    assert_eq!(entries_before, entries_after);
}

#[test]
fn managed_catalog_has_catalog_publication_malformed_future_and_directory_are_err() {
    fn inventory(
        root: &std::path::Path,
    ) -> Vec<(
        std::path::PathBuf,
        bool,
        u64,
        bool,
        std::time::SystemTime,
        Vec<u8>,
    )> {
        #[cfg(test)]
        type TreeInventoryRows = Vec<(
            std::path::PathBuf,
            bool,
            u64,
            bool,
            std::time::SystemTime,
            Vec<u8>,
        )>;
        fn visit(root: &std::path::Path, path: &std::path::Path, rows: &mut TreeInventoryRows) {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            rows.push((
                path.strip_prefix(root).unwrap().to_path_buf(),
                metadata.is_dir(),
                metadata.len(),
                metadata.permissions().readonly(),
                metadata.modified().unwrap(),
                if metadata.is_file() {
                    std::fs::read(path).unwrap()
                } else {
                    Vec::new()
                },
            ));
            if metadata.is_dir() {
                let mut entries: Vec<_> = std::fs::read_dir(path)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .collect();
                entries.sort();
                for entry in entries {
                    visit(root, &entry, rows);
                }
            }
        }
        let mut rows = Vec::new();
        visit(root, root, &mut rows);
        rows
    }
    let (_temp, project) = setup_project();
    std::fs::write(canonical_path(&project), b"not = [valid toml").unwrap();
    assert!(has_catalog_publication(&project).is_err());

    let (_temp, project) = setup_project();
    std::fs::write(
        canonical_path(&project),
        "schema_version = 99\ncoverage_version = 99\ncoverage = []\nfiles = []\n",
    )
    .unwrap();
    assert!(has_catalog_publication(&project).is_err());

    let (_temp, project) = setup_project();
    std::fs::create_dir(canonical_path(&project)).unwrap();
    std::fs::write(canonical_path(&project).join("keep.txt"), b"keep").unwrap();
    let unrelated = project.join(".ac").join("unrelated-target.toml");
    std::fs::write(
        &unrelated,
        b"# unrelated target
",
    )
    .unwrap();
    let before = inventory(&project);
    assert!(has_catalog_publication(&project).is_err());
    for _ in 0..3 {
        let error = has_catalog_publication(&project).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&canonical_path(&project).display().to_string()),
            "{error}"
        );
        assert_eq!(inventory(&project), before);
    }
    assert_eq!(
        std::fs::read(canonical_path(&project).join("keep.txt")).unwrap(),
        b"keep"
    );
    assert_eq!(
        std::fs::read(unrelated).unwrap(),
        b"# unrelated target
"
    );
}

#[test]
fn managed_catalog_has_catalog_publication_reads_under_a_held_writer_lock() {
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        catalog_publication_row().unwrap(),
    );
    // The writer still holds the sidecar lock; the query takes NO lock, so
    // it must see the committed bytes and never block once the guard drops.
    let started = Instant::now();
    assert!(has_catalog_publication(&project).expect("read under held writer"));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "the read must not wait on the writer lock"
    );
    let bytes = std::fs::read(canonical_path(&project)).unwrap();
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), bytes);
    assert!(has_catalog_publication(&project).expect("read after restart"));
}

/// Raw coverage-v2 manifest text carrying exactly one `[[files]]` row, used
/// to feed the real parser combinations the row constructors reject.
fn raw_manifest_with_row(
    path: &str,
    encoding: &str,
    kind: &str,
    scope: &str,
    source: &str,
) -> String {
    format!(
        concat!(
            "schema_version = 1\n",
            "coverage_version = 2\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
            "\n",
            "[[files]]\n",
            "path = \"{path}\"\n",
            "path_encoding = \"{encoding}\"\n",
            "kind = \"{kind}\"\n",
            "scope = \"{scope}\"\n",
            "source = \"{source}\"\n",
            "last_seeded_at = \"2026-09-12T13:30:00.000Z\"\n",
        ),
        path = path,
        encoding = encoding,
        kind = kind,
        scope = scope,
        source = source,
    )
}

#[cfg(windows)]
fn symlink_file_at(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_file(target, link)
}

#[cfg(unix)]
fn symlink_file_at(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[test]
fn managed_catalog_has_catalog_publication_wrong_rows_are_err_or_false_without_writes() {
    let cases: [(&str, &str, &str, &str, &str, bool); 5] = [
        // Correct control: exact path, utf8, catalog kind, canonical scope, builtin.
        (
            ".ac/coding-agents/agents.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "builtin",
            true,
        ),
        // Wrong path.
        (
            ".ac/coding-agents/agents-list.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "builtin",
            false,
        ),
        // Wrong scope.
        (
            ".ac/coding-agents/agents.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:other",
            "builtin",
            false,
        ),
        // Wrong source.
        (
            ".ac/coding-agents/agents.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "user",
            false,
        ),
        // Wrong encoding.
        (
            ".ac/coding-agents/agents.json",
            "unix_bytes_hex",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "builtin",
            false,
        ),
    ];
    for (path, encoding, kind, scope, source, expect_true) in cases {
        let (_temp, project) = setup_project();
        std::fs::write(
            canonical_path(&project),
            raw_manifest_with_row(path, encoding, kind, scope, source),
        )
        .unwrap();
        let bytes_before = std::fs::read(canonical_path(&project)).unwrap();
        let entries_before = ac_dir_entries(&project);
        let result = has_catalog_publication(&project);
        if expect_true {
            assert!(
                result.expect("the correct control parses"),
                "{path} {scope} {source}"
            );
        } else {
            assert!(
                result.is_err(),
                "expected Err for {path} {scope} {source}: {result:?}"
            );
        }
        assert_eq!(
            std::fs::read(canonical_path(&project)).unwrap(),
            bytes_before
        );
        assert_eq!(ac_dir_entries(&project), entries_before);
    }

    // Wrong kind: a well-formed context row parses, but it never matches
    // the catalog identity, so the honest answer is `false` (no proof).
    let (_temp, project) = setup_project();
    std::fs::write(
        canonical_path(&project),
        raw_manifest_with_row(
            ".ac/Context.AgentsCommander.md",
            "utf8",
            "project_context_template",
            "context:agentscommander",
            "builtin",
        ),
    )
    .unwrap();
    let bytes_before = std::fs::read(canonical_path(&project)).unwrap();
    let entries_before = ac_dir_entries(&project);
    assert!(!has_catalog_publication(&project).expect("the context row parses"));
    assert_eq!(
        std::fs::read(canonical_path(&project)).unwrap(),
        bytes_before
    );
    assert_eq!(ac_dir_entries(&project), entries_before);
}

#[test]
fn managed_catalog_has_catalog_publication_link_is_err_without_writes() {
    let (_temp, project) = setup_project();
    let target = project.join(".ac").join("real-manifest-target.toml");
    std::fs::write(&target, b"# fixture target\n").unwrap();
    symlink_file_at(&target, &canonical_path(&project))
        .expect("real manifest symlink fixture (not a silent skip)");
    let entries_before = ac_dir_entries(&project);
    let target_before = std::fs::read(&target).unwrap();
    assert!(has_catalog_publication(&project).is_err());
    assert!(std::fs::symlink_metadata(canonical_path(&project))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(ac_dir_entries(&project), entries_before);
    assert_eq!(std::fs::read(&target).unwrap(), target_before);
}

#[cfg(windows)]
#[test]
fn managed_catalog_has_catalog_publication_share_denied_read_is_err_without_writes() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_WRITE};

    let (_temp, project) = setup_project();
    std::fs::write(
        canonical_path(&project),
        raw_manifest_with_row(
            ".ac/coding-agents/agents.json",
            "utf8",
            "coding_agent_catalog",
            "catalog:coding-agents",
            "builtin",
        ),
    )
    .unwrap();
    let bytes_before = std::fs::read(canonical_path(&project)).unwrap();
    let entries_before = ac_dir_entries(&project);
    // A real handle that denies READ sharing: the query's own read open
    // fails with a real sharing violation (an OS error, not an injection).
    let blocker = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .open(canonical_path(&project))
        .unwrap();
    let result = has_catalog_publication(&project);
    assert!(result.is_err(), "{result:?}");
    assert_eq!(ac_dir_entries(&project), entries_before);
    drop(blocker);
    assert_eq!(
        std::fs::read(canonical_path(&project)).unwrap(),
        bytes_before
    );
}

#[test]
fn fixed_buffer_stream_comparison_detects_content_and_length_changes() {
    let temp = tempfile::NamedTempFile::new().unwrap();
    let bytes = vec![b'a'; RAW_COMPARE_BUFFER_BYTES * 2 + 7];
    std::fs::write(temp.path(), &bytes).unwrap();
    let mut file = File::open(temp.path()).unwrap();
    assert!(stream_matches(&mut file, &bytes).unwrap());
    let mut changed = bytes.clone();
    changed[RAW_COMPARE_BUFFER_BYTES + 1] = b'b';
    assert!(!stream_matches(&mut file, &changed).unwrap());
    assert!(!stream_matches(&mut file, &bytes[..bytes.len() - 1]).unwrap());
}

#[test]
fn independent_handles_contend_and_persistent_lock_releases() {
    let (_temp, project) = setup_project();
    let first = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let started = Instant::now();
    let error =
        ProjectSeedManifestGuard::acquire_with_timeout(&project, Duration::from_millis(120))
            .unwrap_err();
    assert!(matches!(error, SeedManifestError::BusyTimeout { .. }));
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(started.elapsed() < Duration::from_secs(1));
    let lock_path = project.join(".ac").join(SEED_MANIFEST_LOCK_FILENAME);
    assert!(lock_path.is_file());
    first.release();

    let second =
        ProjectSeedManifestGuard::acquire_with_timeout(&project, Duration::from_millis(250))
            .unwrap();
    second.release();
    assert!(lock_path.is_file(), "unlock must not delete the lock file");
}

#[cfg(windows)]
#[test]
fn lock_share_denial_is_precontention_lock_io_and_preserves_the_lock_file() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

    let (_temp, project) = setup_project();
    let lock_path = project.join(".ac").join(SEED_MANIFEST_LOCK_FILENAME);
    let original = b"persistent lock sentinel";
    std::fs::write(&lock_path, original).unwrap();
    let canonical_lock_path = std::fs::canonicalize(&lock_path).unwrap();
    let blocker = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&lock_path)
        .unwrap();

    let error =
        ProjectSeedManifestGuard::acquire_with_timeout(&project, Duration::from_millis(100))
            .unwrap_err();
    match &error {
        SeedManifestError::LockIo {
            path,
            saw_contention,
            source,
        } => {
            assert_eq!(path, &canonical_lock_path);
            assert!(!saw_contention);
            assert_eq!(
                source
                    .raw_os_error()
                    .and_then(|raw| u32::try_from(raw).ok()),
                Some(ERROR_SHARING_VIOLATION)
            );
        }
        other => panic!("expected pre-contention LockIo, got {other:?}"),
    }
    assert_eq!(
        error.degraded_reason(),
        ManifestDegradedReason::LockUnavailable
    );
    drop(blocker);
    assert_eq!(std::fs::read(&lock_path).unwrap(), original);
}

#[test]
fn canonical_diagnostics_and_guard_debug_never_expose_manifest_contents() {
    let secret = "PRIVATE-SEED-CONTENT-7f9c1a";
    let syntax_bytes = format!("{secret} = [\n");
    let syntax_error = parse_manifest_bytes(syntax_bytes.as_bytes()).unwrap_err();
    assert!(matches!(syntax_error, SeedManifestError::Parse(_)));
    assert!(!format!("{syntax_error}").contains(secret));
    assert!(!format!("{syntax_error:?}").contains(secret));
    assert!(!format!("{syntax_error}").contains("1 |"));

    let invalid_wire = format!(
        concat!(
            "schema_version = 1\n",
            "coverage_version = 2\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]\n",
            "\n",
            "[[files]]\n",
            "path = \".ac/{secret}\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"project_context_template\"\n",
            "scope = \"context:agentscommander\"\n",
            "source = \"builtin\"\n",
            "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n"
        ),
        secret = secret
    );
    let validation_error = parse_manifest_bytes(invalid_wire.as_bytes()).unwrap_err();
    assert!(matches!(validation_error, SeedManifestError::Validation(_)));
    assert!(!format!("{validation_error}").contains(secret));
    assert!(!format!("{validation_error:?}").contains(secret));

    let (_invalid_temp, invalid_project) = setup_project();
    std::fs::write(canonical_path(&invalid_project), syntax_bytes.as_bytes()).unwrap();
    let mut invalid_guard = ProjectSeedManifestGuard::acquire(&invalid_project).unwrap();
    let invalid_debug = format!("{invalid_guard:?}");
    assert!(!invalid_debug.contains(secret));
    assert_eq!(
        invalid_guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
    );
    invalid_guard.release();
    assert_eq!(
        std::fs::read(canonical_path(&invalid_project)).unwrap(),
        syntax_bytes.as_bytes()
    );

    let (_temp, project) = setup_project();
    std::fs::write(
        canonical_path(&project),
        serialize_raw_legacy_fixture(&populated_state()),
    )
    .unwrap();
    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let held = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(held.rows.len(), 3);
    assert!(held
        .rows
        .values()
        .any(|row| row.kind == ManifestFileKind::ReplicaConfigFile));
    let debug = format!("{guard:?}");
    assert!(debug.contains("raw_len"));
    assert!(debug.contains("row_count"));
    for content in [
        "Context.AgentsCommander.md",
        "settings.json",
        "context:agentscommander",
        "workspace_base",
    ] {
        assert!(!debug.contains(content), "debug leaked {content}");
    }
    guard.release();
}

#[test]
fn corrupt_and_future_canonical_bytes_are_preserved_read_only() {
    for (name, bytes, expected_reason) in [
        (
            "corrupt",
            b"<<<<<<< conflict\n".as_slice(),
            ManifestDegradedReason::InvalidCanonical,
        ),
        (
            "future",
            concat!(
                "schema_version = 2\n",
                "coverage_version = 1\n",
                "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
                "files = []\n"
            )
            .as_bytes(),
            ManifestDegradedReason::FutureSchema,
        ),
    ] {
        let (_temp, project) = setup_project();
        let path = canonical_path(&project);
        std::fs::write(&path, bytes).unwrap();
        let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
        let outcome = guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        );
        assert_eq!(
            outcome,
            ManifestRecordOutcome::PublishedUnrecorded(expected_reason),
            "case {name}"
        );
        guard.release();
        assert_eq!(std::fs::read(path).unwrap(), bytes, "case {name}");
    }
}

#[test]
fn nonregular_canonical_fails_closed() {
    let (_temp, project) = setup_project();
    std::fs::create_dir(canonical_path(&project)).unwrap();
    let error = ProjectSeedManifestGuard::acquire(&project).unwrap_err();
    assert!(matches!(error, SeedManifestError::UnsafePath { .. }));
}

#[cfg(unix)]
#[test]
fn symlink_canonical_and_lock_fail_closed_without_following() {
    use std::os::unix::fs::symlink;

    for filename in [SEED_MANIFEST_FILENAME, SEED_MANIFEST_LOCK_FILENAME] {
        let (_temp, project) = setup_project();
        let outside = project.join("outside");
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, project.join(".ac").join(filename)).unwrap();
        let error = ProjectSeedManifestGuard::acquire(&project).unwrap_err();
        assert!(matches!(error, SeedManifestError::UnsafePath { .. }));
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");
    }
}

#[test]
fn prior_temp_policy_is_platform_specific_and_preserves_lookalikes() {
    let (_temp, project) = setup_project();
    let ac = project.join(".ac");
    std::fs::write(
        canonical_path(&project),
        serialize_state(&ManifestState::default()).unwrap(),
    )
    .unwrap();
    let valid = ac.join(".seed-manifest.00000000-0000-0000-0000-000000000000.tmp");
    let uppercase = ac.join(".seed-manifest.00000000-0000-0000-0000-00000000000A.tmp");
    let malformed = ac.join(".seed-manifest.not-a-uuid.tmp");
    let directory = ac.join(".seed-manifest.11111111-1111-1111-1111-111111111111.tmp");
    std::fs::write(&valid, b"stale").unwrap();
    std::fs::write(&uppercase, b"user").unwrap();
    std::fs::write(&malformed, b"user").unwrap();
    std::fs::create_dir(&directory).unwrap();

    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    guard.release();
    #[cfg(unix)]
    assert!(!valid.exists());
    #[cfg(windows)]
    assert!(valid.is_file());
    assert!(uppercase.is_file());
    assert!(malformed.is_file());
    assert!(directory.is_dir());
}

#[test]
fn second_hard_link_on_temp_blocks_publication() {
    let (_temp, project) = setup_project();
    let alias = project.join(".ac").join("temp-alias");
    let hooks = TestFilesystemHooks {
        before_temp_validation: Some(Arc::new({
            let alias = alias.clone();
            move |temp| {
                std::fs::hard_link(temp, &alias).expect("create adversarial temp hard link");
            }
        })),
        ..TestFilesystemHooks::default()
    };
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
            .unwrap();
    let outcome = guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::UnsafePath)
    );
    assert!(!canonical_path(&project).exists());
    assert!(alias.is_file());
    guard.release();
}

#[test]
fn raw_external_edit_conflict_preserves_external_bytes() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        initial.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    initial.release();

    let external = b"external lock-unaware edit\n".to_vec();
    let hooks = TestFilesystemHooks {
        before_raw_conflict_check: Some(Arc::new({
            let external = external.clone();
            move |canonical| std::fs::write(canonical, &external).unwrap()
        })),
        ..TestFilesystemHooks::default()
    };
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
            .unwrap();
    let outcome = guard.publication_permit().record_file(
        &activation,
        context_row(
            "Context.coordinator.md",
            "context:coordinator",
            "2026-07-16T19:42:00.000Z",
        )
        .unwrap(),
    );
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ExternalEdit)
    );
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), external);
}

// ---- #1318 v1 -> v2 coverage migration + catalog row -------------------

/// A byte-exact v1 manifest (context row + replica-config row with distinct
/// timestamps), exactly what an old build wrote.
fn v1_manifest_fixture() -> String {
    concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 1\n",
        "coverage_version = 1\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/Context.AgentsCommander.md\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"project_context_template\"\n",
        "scope = \"context:agentscommander\"\n",
        "source = \"builtin\"\n",
        "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n",
        "\n",
        "[[files]]\n",
        "path = \".ac/wg-1-dev-team/__agent_alpha/.claude/settings.json\"\n",
        "path_encoding = \"utf8\"\n",
        "kind = \"replica_config_file\"\n",
        "scope = \"config:.ac/wg-1-dev-team/__agent_alpha/.claude\"\n",
        "source = \"workspace_base\"\n",
        "last_seeded_at = \"2026-07-16T19:41:12.456Z\"\n"
    )
    .to_string()
}

#[test]
fn v1_manifest_upgrades_to_v2_preserving_supported_rows_and_omitting_legacy_config() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    let v1 = v1_manifest_fixture();
    std::fs::write(&path, &v1).unwrap();

    // First acquire runs the one-shot upgrade under the held lock.
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let disk = std::fs::read_to_string(&path).unwrap();
    assert!(
        disk.contains("coverage_version = 2"),
        "upgraded manifest must declare coverage v2: {disk}"
    );
    assert!(
        disk.contains(
            "coverage = [\"project_context_templates\", \"replica_config_folders\", \"coding_agent_catalog\"]"
        ),
        "upgraded manifest must carry the 3-item v2 coverage: {disk}"
    );
    // Supported rows and timestamps remain on disk; the legacy config row is
    // retained only in the held state.
    let state = read_disk_state(&project);
    assert_eq!(state.rows.len(), 1);
    assert!(disk.contains("2026-07-16T19:40:07.123Z"));
    assert!(!disk.contains("replica_config_file"));
    assert!(!disk.contains(".ac/wg-1-dev-team/__agent_alpha/.claude/settings.json"));
    assert!(!disk.contains("2026-07-16T19:41:12.456Z"));
    let held = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(held.rows.len(), 2);
    assert!(held
        .rows
        .values()
        .any(|row| row.kind == ManifestFileKind::ReplicaConfigFile));
    // The guard is a fully writable tracked snapshot: publications record.
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            PublishedManifestRow::coding_agent_catalog(
                ManifestPathIdentity::parse(
                    ManifestPathEncoding::Utf8,
                    ".ac/coding-agents/agents.json".to_string(),
                )
                .unwrap(),
                timestamp("2026-07-16T19:42:00.000Z"),
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    let held_after_record = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(held_after_record.rows.len(), 3);
    assert!(held_after_record
        .rows
        .values()
        .any(|row| row.kind == ManifestFileKind::ReplicaConfigFile));
    guard.release();

    // Runs exactly once: a second acquire parses v2 strictly and never
    // re-enters the migration.
    let after_first = std::fs::read(&path).unwrap();
    let guard2 = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let reacquired = match &guard2.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(reacquired.rows.len(), 2);
    assert!(reacquired
        .rows
        .values()
        .all(|row| row.kind.is_canonical_output()));
    guard2.release();
    assert_eq!(std::fs::read(&path).unwrap(), after_first);
    let state = read_disk_state(&project);
    assert_eq!(state.rows.len(), 2);
    let disk = String::from_utf8(after_first).unwrap();
    assert!(disk.contains("coding_agent_catalog"));
    assert!(!disk.contains("replica_config_file"));
}

#[test]
fn v1_with_different_coverage_list_stays_strictly_invalid() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    // v1 header with a DIFFERENT (3-item, wrong order) coverage list.
    let v1 = v1_manifest_fixture().replace(
        "coverage = [\"project_context_templates\", \"replica_config_folders\"]",
        "coverage = [\"replica_config_folders\", \"project_context_templates\"]",
    );
    std::fs::write(&path, &v1).unwrap();

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
    );
    guard.release();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        v1,
        "non-exact v1 shape stays byte-preserved"
    );
}

#[test]
fn coverage_version_three_stays_invalid_and_non_migratable() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    // v1 -> v2 -> the WRONG value for this build (3): the strict parse
    // rejects it and the migration must not touch it (not the exact v1
    // shape: coverage_version != 1).
    let wrong = v1_manifest_fixture().replace("coverage_version = 1", "coverage_version = 3");
    std::fs::write(&path, &wrong).unwrap();

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
    );
    guard.release();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        wrong,
        "wrong coverage version stays byte-preserved"
    );
}

#[test]
fn bound_violating_v1_file_stays_preserved_read_only() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    // An over-long path field: the wire deserializers enforce MAX_FIELD_BYTES
    // on the strict parse (ResourceBound, never migratable) AND on the
    // migration's lenient pre-parse, so the file stays preserved either way.
    let overlong = format!("x{}\n", "a".repeat(MAX_FIELD_BYTES + 1));
    let v1 = format!(
        concat!(
            "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
            "schema_version = 1\n",
            "coverage_version = 1\n",
            "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
            "\n",
            "[[files]]\n",
            "path = \"{}\"\n",
            "path_encoding = \"utf8\"\n",
            "kind = \"project_context_template\"\n",
            "scope = \"context:agentscommander\"\n",
            "source = \"builtin\"\n",
            "last_seeded_at = \"2026-07-16T19:40:07.123Z\"\n"
        ),
        overlong
    );
    std::fs::write(&path, &v1).unwrap();

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    // The strict parse classifies the over-long path as a parse failure on
    // this TOML surface (the lenient pre-parse fails the same way), and the
    // migration is never entered; either way the contract is preservation
    // with the writer disabled.
    let outcome = guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    assert!(matches!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(
            ManifestDegradedReason::InvalidCanonical | ManifestDegradedReason::ResourceBound(_)
        )
    ));
    guard.release();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        v1.as_bytes(),
        "bound-violating v1 file stays byte-preserved"
    );
}

#[test]
fn future_schema_snapshot_never_migrated() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    let future = concat!(
        "# Managed by AgentsCommander. Diagnostic only; never grants file ownership.\n",
        "schema_version = 2\n",
        "coverage_version = 1\n",
        "coverage = [\"project_context_templates\", \"replica_config_folders\"]\n",
        "files = []\n"
    );
    std::fs::write(&path, future).unwrap();

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::FutureSchema)
    );
    guard.release();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        future.as_bytes(),
        "future-schema manifest stays byte-preserved"
    );
}

#[test]
fn external_edit_conflict_during_upgrade_preserves_v1_bytes() {
    let (_temp, project) = setup_project();
    let path = canonical_path(&project);
    std::fs::write(&path, v1_manifest_fixture()).unwrap();

    // A lock-unaware external writer replaces the file between the
    // migration's re-read and its atomic write: `write_canonical`'s
    // raw-conflict check must fail and preserve the external bytes.
    let external = b"external lock-unaware edit\n".to_vec();
    let hooks = TestFilesystemHooks {
        before_raw_conflict_check: Some(Arc::new({
            let external = external.clone();
            move |canonical| std::fs::write(canonical, &external).unwrap()
        })),
        ..TestFilesystemHooks::default()
    };
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
            .unwrap();
    // The migration's own write fails the raw-conflict check and restores the
    // pre-migration degraded snapshot (InvalidCanonical), so the next
    // publication reports the restored reason; the load-bearing assertion is
    // that the external bytes win and no partial v2 upgrade ever lands.
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::InvalidCanonical)
    );
    guard.release();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        external,
        "the external edit wins; v1 bytes never partially upgraded"
    );
}

#[test]
fn coding_agent_catalog_row_requires_exact_path_scope_source_encoding() {
    let at = timestamp("2026-07-16T19:43:00.000Z");
    let ok = PublishedManifestRow::coding_agent_catalog(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/coding-agents/agents.json".to_string(),
        )
        .unwrap(),
        at,
    )
    .expect("exact catalog row is accepted");
    assert_eq!(ok.kind, ManifestFileKind::CodingAgentCatalog);
    assert_eq!(ok.scope, "catalog:coding-agents");
    assert_eq!(ok.source, ManifestSource::Builtin);
    assert_eq!(ok.path.serialized, ".ac/coding-agents/agents.json");
    assert_eq!(ok.path.encoding, ManifestPathEncoding::Utf8);

    // Wrong path -> rejected.
    assert!(PublishedManifestRow::coding_agent_catalog(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/coding-agents/other.json".to_string(),
        )
        .unwrap(),
        at,
    )
    .is_err());
    // Non-utf8 encoding -> rejected. A WindowsUtf16Hex wire shape only
    // parses for paths that are NOT valid Unicode (e.g. an unpaired
    // surrogate), exactly like the existing round-trip fixtures.
    let mut windows_units: Vec<u16> = ".ac/coding-agents/agents.json".encode_utf16().collect();
    windows_units.push(0xd800);
    let windows_hex = windows_units
        .iter()
        .map(|unit| format!("{unit:04x}"))
        .collect::<String>();
    assert!(PublishedManifestRow::coding_agent_catalog(
        ManifestPathIdentity::parse(ManifestPathEncoding::WindowsUtf16Hex, windows_hex).unwrap(),
        at,
    )
    .is_err());

    // A catalog row records under a held gate, alongside context rows.
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            PublishedManifestRow::coding_agent_catalog(
                ManifestPathIdentity::parse(
                    ManifestPathEncoding::Utf8,
                    ".ac/coding-agents/agents.json".to_string(),
                )
                .unwrap(),
                at,
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    guard.release();
    let disk = std::fs::read_to_string(canonical_path(&project)).unwrap();
    assert!(disk.contains("kind = \"coding_agent_catalog\""));
    assert!(disk.contains("scope = \"catalog:coding-agents\""));
    assert!(disk.contains("path = \".ac/coding-agents/agents.json\""));
    assert!(disk.contains("source = \"builtin\""));
}

#[test]
fn lock_identity_replacement_is_detected() {
    let (_temp, project) = setup_project();
    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let lock_path = project.join(".ac").join(SEED_MANIFEST_LOCK_FILENAME);
    std::fs::remove_file(&lock_path).unwrap();
    std::fs::write(&lock_path, b"replacement").unwrap();
    assert!(matches!(
        guard.revalidate_owner(),
        Err(SeedManifestError::UnsafePath { .. })
    ));
    guard.release();
}

#[test]
fn pinned_owner_chain_detects_replacement() {
    let (_temp, project) = setup_project();
    let ac = project.join(".ac");
    let owner = ac.join("wg-1-team").join("__agent_alpha");
    std::fs::create_dir_all(&owner).unwrap();
    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let chain = guard
        .pin_owner_chain_from_ac(Path::new("wg-1-team/__agent_alpha"))
        .unwrap();
    #[cfg(not(windows))]
    {
        std::fs::rename(ac.join("wg-1-team"), ac.join("wg-old")).unwrap();
        std::fs::create_dir_all(&owner).unwrap();
    }
    #[cfg(windows)]
    let chain = {
        let mut chain = chain;
        chain.directories[0].identity.file ^= 1;
        chain
    };
    assert!(matches!(
        chain.revalidate(),
        Err(SeedManifestError::UnsafePath { .. })
    ));
    guard.release();
}

#[test]
fn upsert_and_same_millisecond_noop_use_atomic_writer_once() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let row = context_row(
        "Context.AgentsCommander.md",
        "context:agentscommander",
        "2026-07-16T19:40:07.123Z",
    )
    .unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .record_file(&activation, row.clone()),
        ManifestRecordOutcome::Recorded
    );
    let first = std::fs::read(canonical_path(&project)).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(&activation, row),
        ManifestRecordOutcome::Unchanged
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), first);

    let later = context_row(
        "Context.AgentsCommander.md",
        "context:agentscommander",
        "2026-07-16T19:40:07.124Z",
    )
    .unwrap();
    assert_eq!(
        guard.publication_permit().record_file(&activation, later),
        ManifestRecordOutcome::Recorded
    );
    guard.release();
    assert!(
        String::from_utf8(std::fs::read(canonical_path(&project)).unwrap())
            .unwrap()
            .contains("2026-07-16T19:40:07.124Z")
    );
}

#[test]
fn config_scope_replacement_is_canonical_noop_and_preserves_in_memory_semantics() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let scope = config_scope("wg-1-team", "alpha", ".claude");
    let first = PublishedScopeBatch::new(
        scope.clone(),
        ManifestSource::WorkspaceBase,
        vec![
            config_path("wg-1-team", "alpha", ".claude", "a"),
            config_path("wg-1-team", "alpha", ".claude", "b"),
        ],
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap();
    let second = PublishedScopeBatch::new(
        scope.clone(),
        ManifestSource::MatrixBase,
        vec![
            config_path("wg-1-team", "alpha", ".claude", "b"),
            config_path("wg-1-team", "alpha", ".claude", "c"),
        ],
        timestamp("2026-07-16T19:42:12.456Z"),
    )
    .unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    let canonical = std::fs::read(canonical_path(&project)).unwrap();
    assert_eq!(
        guard.publication_permit().replace_scope(&activation, first),
        ManifestRecordOutcome::Unchanged
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), canonical);
    let paths = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state
            .rows
            .values()
            .map(|row| row.path.serialized().to_string())
            .collect::<Vec<_>>(),
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(
        paths,
        vec![
            ".ac/Context.AgentsCommander.md".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/a".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/b".to_string(),
        ]
    );
    assert_eq!(
        guard
            .publication_permit()
            .replace_scope(&activation, second),
        ManifestRecordOutcome::Unchanged
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), canonical);
    let paths = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state
            .rows
            .values()
            .map(|row| row.path.serialized().to_string())
            .collect::<Vec<_>>(),
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(
        paths,
        vec![
            ".ac/Context.AgentsCommander.md".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/b".to_string(),
            ".ac/wg-1-team/__agent_alpha/.claude/c".to_string(),
        ]
    );
    let empty = PublishedScopeBatch::new(
        scope,
        ManifestSource::MatrixBase,
        Vec::new(),
        timestamp("2026-07-16T19:43:12.456Z"),
    )
    .unwrap();
    assert_eq!(
        guard.publication_permit().replace_scope(&activation, empty),
        ManifestRecordOutcome::Unchanged
    );
    let final_paths = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state
            .rows
            .values()
            .map(|row| row.path.serialized().to_string())
            .collect::<Vec<_>>(),
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(
        final_paths,
        vec![".ac/Context.AgentsCommander.md".to_string()]
    );
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), canonical);
    assert_eq!(read_disk_state(&project).rows.len(), 1);
}

#[test]
fn empty_scope_against_absence_is_noop_without_header_file() {
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let batch = PublishedScopeBatch::new(
        config_scope("wg-1-team", "alpha", ".claude"),
        ManifestSource::WorkspaceBase,
        Vec::new(),
        timestamp("2026-07-16T19:41:12.456Z"),
    )
    .unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .replace_scope(&ManifestActivationToken::for_test(), batch),
        ManifestRecordOutcome::Unchanged
    );
    guard.release();
    assert!(!canonical_path(&project).exists());
}

#[test]
fn overbound_removal_from_raw_legacy_scope_writes_canonical_without_partial_rows() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let scope = config_scope("wg-1-team", "alpha", ".claude");
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            scope.clone(),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    std::fs::write(
        canonical_path(&project),
        serialize_raw_legacy_fixture(&legacy),
    )
    .unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().remove_unrecordable_scope(
            &activation,
            scope,
            ResourceBoundKind::Rows,
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ResourceBound(
            ResourceBoundKind::Rows
        ))
    );
    guard.release();
    assert!(read_disk_state(&project).rows.is_empty());
}

#[test]
fn failed_overbound_scope_removal_reports_the_persistence_failure() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let scope = config_scope("wg-1-team", "alpha", ".claude");
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            scope.clone(),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    std::fs::write(
        canonical_path(&project),
        serialize_raw_legacy_fixture(&legacy),
    )
    .unwrap();

    let external = b"lock-unaware external edit".to_vec();
    let hook_bytes = external.clone();
    let hooks = TestFilesystemHooks {
        before_raw_conflict_check: Some(Arc::new(move |canonical| {
            std::fs::write(canonical, &hook_bytes).expect("write external edit");
        })),
        ..TestFilesystemHooks::default()
    };
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, DEFAULT_LOCK_TIMEOUT, hooks)
            .unwrap();
    assert_eq!(
        guard.publication_permit().remove_unrecordable_scope(
            &activation,
            scope,
            ResourceBoundKind::Rows,
        ),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ExternalEdit)
    );
    let restored = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(restored.rows.len(), 1);
    assert!(restored.rows.values().any(|row| {
        row.path.serialized() == ".ac/wg-1-team/__agent_alpha/.claude/settings.json"
    }));
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), external);
}

#[test]
fn lifecycle_filters_are_component_wise_for_foreign_encoded_rows() {
    let mut bytes = b".ac/wg-1-team/__agent_alpha/.claude/foreign".to_vec();
    bytes.push(0xff);
    let foreign = ManifestPathIdentity::parse(
        ManifestPathEncoding::UnixBytesHex,
        encode_lower_hex_bytes(&bytes),
    )
    .unwrap();
    let mut state = ManifestState::default();
    let _ = state.upsert(
        PublishedManifestRow::replica_config(
            foreign,
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    let _ = state.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-2-team", "beta", ".claude", "keep"),
            config_scope("wg-2-team", "beta", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    let filter = ManifestLifecycleFilter::config_path_prefix(
        ManifestPathIdentity::parse(
            ManifestPathEncoding::Utf8,
            ".ac/wg-1-team/__agent_alpha".to_string(),
        )
        .unwrap(),
    )
    .unwrap();
    let journal = state.remove_matching(|row| filter.matches(row));
    assert!(journal.changed());
    assert_eq!(state.rows.len(), 1);
    assert!(state
        .rows
        .values()
        .next()
        .unwrap()
        .path
        .serialized()
        .contains("wg-2-team"));
}

#[test]
fn lifecycle_filter_prunes_legacy_config_and_writes_canonical_supported_rows() {
    let (_temp, project) = setup_project();
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-2-team", "beta", ".claude", "settings.json"),
            config_scope("wg-2-team", "beta", ".claude"),
            ManifestSource::MatrixBase,
            timestamp("2026-07-16T19:42:12.456Z"),
        )
        .unwrap(),
    );
    let raw = serialize_raw_legacy_fixture(&legacy);
    std::fs::write(canonical_path(&project), &raw).unwrap();
    let activation = ManifestActivationToken::for_test();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let absent = ManifestLifecycleFilter::replica_component("__agent_absent".to_string()).unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .apply_lifecycle_filter(&activation, absent),
        ManifestRecordOutcome::Unchanged
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), raw);
    let unchanged = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(unchanged, &legacy);

    let alpha = ManifestLifecycleFilter::replica_component("__agent_alpha".to_string()).unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .apply_lifecycle_filter(&activation, alpha),
        ManifestRecordOutcome::Recorded
    );
    let held = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(held.rows.len(), 2);
    assert!(held
        .rows
        .values()
        .any(|row| row.path.serialized().contains("wg-2-team")));
    assert!(!held
        .rows
        .values()
        .any(|row| row.path.serialized().contains("wg-1-team")));
    let disk = std::fs::read(canonical_path(&project)).unwrap();
    assert!(!String::from_utf8_lossy(&disk).contains("replica_config_file"));
    guard.release();
    let parsed = parse_manifest_bytes(&disk).unwrap();
    assert_eq!(parsed.rows.len(), 1);
    let context = parsed.rows.values().next().unwrap();
    assert_eq!(context.kind, ManifestFileKind::ProjectContextTemplate);
    assert_eq!(context.path.serialized(), ".ac/Context.AgentsCommander.md");
    assert_eq!(context.scope, "context:agentscommander");
    assert_eq!(context.source, ManifestSource::Builtin);
    assert_eq!(context.published_at, timestamp("2026-07-16T19:40:07.123Z"));
    assert_eq!(serialize_state(&parsed).unwrap(), disk);
}

#[test]
fn lifecycle_filter_persistence_failure_rolls_back_and_preserves_external_bytes() {
    let (_temp, project) = setup_project();
    let mut legacy = ManifestState::default();
    let _ = legacy.upsert(
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    let _ = legacy.upsert(
        PublishedManifestRow::replica_config(
            config_path("wg-1-team", "alpha", ".claude", "settings.json"),
            config_scope("wg-1-team", "alpha", ".claude"),
            ManifestSource::WorkspaceBase,
            timestamp("2026-07-16T19:41:12.456Z"),
        )
        .unwrap(),
    );
    std::fs::write(
        canonical_path(&project),
        serialize_raw_legacy_fixture(&legacy),
    )
    .unwrap();
    let external = b"lock-unaware lifecycle edit".to_vec();
    let hook_bytes = external.clone();
    let hooks = TestFilesystemHooks {
        before_raw_conflict_check: Some(Arc::new(move |canonical| {
            std::fs::write(canonical, &hook_bytes).expect("write external edit");
        })),
        ..TestFilesystemHooks::default()
    };
    let filter = ManifestLifecycleFilter::replica_component("__agent_alpha".to_string()).unwrap();
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, DEFAULT_LOCK_TIMEOUT, hooks)
            .unwrap();
    assert_eq!(
        guard
            .publication_permit()
            .apply_lifecycle_filter(&ManifestActivationToken::for_test(), filter,),
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ExternalEdit)
    );
    let restored = match &guard.snapshot {
        CanonicalSnapshot::Writable { state, .. } => state,
        CanonicalSnapshot::ReadOnly { .. } => panic!("writable snapshot expected"),
    };
    assert_eq!(restored, &legacy);
    guard.release();
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), external);
}

#[test]
fn degraded_permit_never_records_even_with_an_activation_token() {
    let permit =
        ProjectPublicationPermit::degraded_without_guard(ManifestDegradedReason::LockUnavailable);
    assert!(!permit.is_tracked());
    let outcome = permit.record_file(
        &ManifestActivationToken::production(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::LockUnavailable)
    );
}

#[test]
fn production_activation_token_records_through_a_tracked_permit() {
    // Stage F: the production constructor is a real, non-test activation path.
    // A tracked permit driven by it records exactly like the test token, so
    // production emission is genuinely wired and not a dormant no-op.
    let (_temp, project) = setup_project();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let outcome = guard.publication_permit().record_file(
        &ManifestActivationToken::production(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    assert_eq!(outcome, ManifestRecordOutcome::Recorded);
    guard.release();
    let disk = read_disk_state(&project);
    assert_eq!(disk.rows.len(), 1);
}

#[test]
fn v1_coverage_declaration_is_exhaustive() {
    // The declaration must name every coverage boundary exactly
    // once. The exhaustive match makes adding a `V1CoverageBoundary` variant
    // without listing it here (or vice versa) a compile/count failure, so the
    // Stage F coverage checklist cannot silently drop a boundary.
    assert_eq!(V1_COVERAGE_BOUNDARIES.len(), 17);
    for boundary in V1_COVERAGE_BOUNDARIES {
        // Exhaustive match: a new variant forces an update here.
        match boundary {
            V1CoverageBoundary::DirectCreateAcProjectFreshRoot
            | V1CoverageBoundary::ContextCreate
            | V1CoverageBoundary::ContextUpdate
            | V1CoverageBoundary::ContextSelfHeal
            | V1CoverageBoundary::ContextOverwrite
            | V1CoverageBoundary::CoordinatorStatelessV2ToV4
            | V1CoverageBoundary::CoordinatorStatelessV3ToV4
            | V1CoverageBoundary::CoordinatorSeededV3ToV4
            | V1CoverageBoundary::ConfigExactPublish
            | V1CoverageBoundary::ConfigOverBoundPublish
            | V1CoverageBoundary::ConfigFailedRestore
            | V1CoverageBoundary::LifecycleReplicaRemoval
            | V1CoverageBoundary::LifecycleWorkgroupRemoval
            | V1CoverageBoundary::LifecycleTeamDeletion
            | V1CoverageBoundary::LifecycleAgentMatrixDeletion
            | V1CoverageBoundary::PendingInclusiveDeleteProtection
            | V1CoverageBoundary::CatalogSeed => {}
        }
    }
    // No duplicates: every declared boundary is distinct.
    for (index, boundary) in V1_COVERAGE_BOUNDARIES.iter().enumerate() {
        assert!(
            !V1_COVERAGE_BOUNDARIES[index + 1..].contains(boundary),
            "duplicate coverage boundary {boundary:?}"
        );
    }
}

#[test]
fn exact_temp_name_requires_lowercase_hyphenated_uuid() {
    assert!(is_exact_temp_name(
        ".seed-manifest.00000000-0000-0000-0000-000000000000.tmp"
    ));
    assert!(!is_exact_temp_name(
        ".seed-manifest.00000000000000000000000000000000.tmp"
    ));
    assert!(!is_exact_temp_name(
        ".seed-manifest.00000000-0000-0000-0000-00000000000A.tmp"
    ));
    assert!(!is_exact_temp_name(".seed-manifest.not-a-uuid.tmp"));
}

#[test]
fn prior_temp_inventory_is_hard_bounded_and_preserves_uninspected_entries() {
    let (_temp, project) = setup_project();
    let ac = project.join(".ac");
    let candidate_count = MAX_TEMP_INVENTORY_ENTRIES + 5;
    for index in 0..candidate_count {
        let name = format!(
            "{SEED_MANIFEST_TEMP_PREFIX}{}{SEED_MANIFEST_TEMP_SUFFIX}",
            Uuid::from_u128(u128::try_from(index + 1).unwrap()).hyphenated()
        );
        std::fs::write(ac.join(name), b"candidate").unwrap();
    }
    for index in 0..(MAX_TEMP_DIAGNOSTIC_SAMPLES + 5) {
        std::fs::write(
            ac.join(format!(".seed-manifest.malformed-{index}.tmp")),
            b"lookalike",
        )
        .unwrap();
    }

    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let inventory = guard.inventory_prior_temps();
    assert!(inventory.scan_truncated);
    assert!(inventory.exact_count > 0);
    assert!(
        inventory
            .exact_count
            .saturating_add(inventory.malformed_count)
            <= u64::try_from(MAX_TEMP_INVENTORY_ENTRIES).unwrap()
    );
    assert!(inventory.exact_names.len() <= MAX_TEMP_INVENTORY_ENTRIES);
    assert!(inventory.exact_samples.len() <= MAX_TEMP_DIAGNOSTIC_SAMPLES);
    assert!(inventory.malformed_samples.len() <= MAX_TEMP_DIAGNOSTIC_SAMPLES);
    guard.release();
    assert_eq!(exact_temp_paths(&project).len(), candidate_count);
}

#[test]
fn prior_temp_inventory_counts_and_samples_malformed_names_exactly() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (_temp, project) = setup_project();
    let ac = project.join(".ac");
    for index in 0..3_u128 {
        let name = format!(
            "{SEED_MANIFEST_TEMP_PREFIX}{}{SEED_MANIFEST_TEMP_SUFFIX}",
            Uuid::from_u128(index + 1).hyphenated()
        );
        std::fs::write(ac.join(name), b"candidate").unwrap();
    }
    let malformed_count = MAX_TEMP_DIAGNOSTIC_SAMPLES + 5;
    for index in 0..malformed_count {
        std::fs::write(
            ac.join(format!(".seed-manifest.malformed-{index}.tmp")),
            b"lookalike",
        )
        .unwrap();
    }

    let diagnostic_count = Arc::new(AtomicUsize::new(0));
    let hooks = TestFilesystemHooks {
        on_prior_temp_diagnostic: Some(Arc::new({
            let diagnostic_count = Arc::clone(&diagnostic_count);
            move |inventory| {
                diagnostic_count.fetch_add(1, Ordering::SeqCst);
                assert_eq!(inventory.exact_count, 3);
                assert_eq!(
                    inventory.malformed_count,
                    u64::try_from(malformed_count).unwrap()
                );
                assert_eq!(inventory.exact_samples.len(), 3);
                assert_eq!(
                    inventory.malformed_samples.len(),
                    MAX_TEMP_DIAGNOSTIC_SAMPLES
                );
            }
        })),
        ..TestFilesystemHooks::default()
    };
    let guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
            .unwrap();
    assert_eq!(diagnostic_count.load(Ordering::SeqCst), 1);

    let inventory = guard.inventory_prior_temps();
    assert!(!inventory.scan_truncated);
    assert!(inventory.scan_error.is_none());
    assert_eq!(inventory.entry_errors, 0);
    assert_eq!(inventory.exact_count, 3);
    assert_eq!(
        inventory.malformed_count,
        u64::try_from(malformed_count).unwrap()
    );
    assert_eq!(inventory.malformed_native_count, 0);
    assert_eq!(inventory.exact_names.len(), 3);
    assert_eq!(inventory.exact_samples.len(), 3);
    assert_eq!(
        inventory.malformed_samples.len(),
        MAX_TEMP_DIAGNOSTIC_SAMPLES
    );
    assert!(inventory
        .malformed_samples
        .iter()
        .all(|sample| sample.starts_with(SEED_MANIFEST_TEMP_PREFIX)
            && sample.ends_with(SEED_MANIFEST_TEMP_SUFFIX)));
    guard.release();
}

#[cfg(any(unix, windows))]
#[test]
fn native_non_unicode_temp_shape_is_counted_without_lossy_materialization() {
    let (_temp, project) = setup_project();
    let guard = ProjectSeedManifestGuard::acquire(&project).unwrap();

    #[cfg(unix)]
    let native_name = {
        use std::os::unix::ffi::OsStringExt;

        let mut bytes = SEED_MANIFEST_TEMP_PREFIX.as_bytes().to_vec();
        bytes.push(0xff);
        bytes.extend_from_slice(SEED_MANIFEST_TEMP_SUFFIX.as_bytes());
        std::ffi::OsString::from_vec(bytes)
    };
    #[cfg(windows)]
    let native_name = {
        use std::os::windows::ffi::OsStringExt;

        let mut units = SEED_MANIFEST_TEMP_PREFIX.encode_utf16().collect::<Vec<_>>();
        units.push(0xd800);
        units.extend(SEED_MANIFEST_TEMP_SUFFIX.encode_utf16());
        std::ffi::OsString::from_wide(&units)
    };

    assert!(native_name.to_str().is_none());
    std::fs::write(project.join(".ac").join(&native_name), b"lookalike").unwrap();
    let inventory = guard.inventory_prior_temps();
    assert!(!inventory.scan_truncated);
    assert_eq!(inventory.malformed_count, 1);
    assert_eq!(inventory.malformed_native_count, 1);
    assert_eq!(
        inventory.malformed_samples,
        vec!["<native-non-unicode-temp-name>".to_string()]
    );
    assert!(!inventory
        .malformed_samples
        .iter()
        .any(|sample| sample.contains('\u{fffd}')));
    guard.release();
}

#[cfg(windows)]
#[test]
fn open_read_write_temp_handle_conflicts_with_replace_even_with_delete_access() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_SHARING_VIOLATION, GENERIC_READ, GENERIC_WRITE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        ReplaceFileW, DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    let (_temp, project) = setup_project();
    let canonical = canonical_path(&project);
    std::fs::write(&canonical, b"old canonical").unwrap();
    for include_delete in [false, true] {
        let temp_path = project.join(".ac").join(format!(
            ".seed-manifest.{}-writer-conflict.tmp",
            if include_delete { "delete" } else { "ordinary" }
        ));
        let file = if include_delete {
            OpenOptions::new()
                .read(true)
                .write(true)
                .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .create_new(true)
                .open(&temp_path)
                .unwrap()
        } else {
            open_regular_no_follow(&temp_path, true, true).unwrap()
        };
        let mut opened = OpenedRegularFile::from_file(&temp_path, file).unwrap();
        let replacement = b"replacement";
        opened.file.write_all(replacement).unwrap();
        opened.file.sync_all().unwrap();
        opened.refresh_facts().unwrap();
        let temp_wide = absolute_verbatim_utf16(&temp_path).unwrap();
        let canonical_wide = absolute_verbatim_utf16(&canonical).unwrap();
        let success = unsafe {
            ReplaceFileW(
                canonical_wide.as_ptr(),
                temp_wide.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        assert_eq!(success, 0);
        let raw_error = unsafe { GetLastError() };
        assert_eq!(raw_error, ERROR_SHARING_VIOLATION);
        assert_eq!(std::fs::read(&canonical).unwrap(), b"old canonical");
        assert_eq!(std::fs::read(&temp_path).unwrap(), replacement);
        cleanup_current_owned_temp(&opened, Some(replacement)).unwrap();
        drop(opened);
        assert!(!temp_path.exists());
    }
}

#[cfg(windows)]
#[test]
fn zero_access_witness_replace_preserves_old_handle_identity_with_zero_links() {
    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let mut first = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        first.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    first.release();

    let canonical = canonical_path(&project);
    let old = OpenedRegularFile::open_existing(&canonical, false).unwrap();
    let old_identity = old.identity;
    let mut second = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        second.publication_permit().record_file(
            &activation,
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    second.release();

    let final_file = OpenedRegularFile::open_existing(&canonical, false).unwrap();
    assert_ne!(final_file.identity, old_identity);
    let old_facts = handle_facts(&old.file).unwrap();
    assert_eq!(old_facts.identity, old_identity);
    assert_eq!(old_facts.links, 0);
}

#[cfg(windows)]
#[test]
fn destination_share_denial_returns_typed_error_and_preserves_both_states() {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Foundation::ERROR_SHARING_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};

    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        initial.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    initial.release();

    let canonical = canonical_path(&project);
    let original = std::fs::read(&canonical).unwrap();
    let mut desired = read_disk_state(&project);
    let _ = desired.upsert(
        context_row(
            "Context.coordinator.md",
            "context:coordinator",
            "2026-07-16T19:42:00.000Z",
        )
        .unwrap(),
    );
    let desired_bytes = serialize_state(&desired).unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let blocker = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(&canonical)
        .unwrap();

    let error = guard.write_canonical(desired_bytes).unwrap_err();
    match &error {
        SeedManifestError::WindowsNamespaceFailure {
            operation,
            raw_error,
            canonical_state,
            temp_state,
            ..
        } => {
            assert_eq!(*operation, WindowsNamespaceOperation::ReplaceFile);
            assert_eq!(*raw_error, ERROR_SHARING_VIOLATION);
            assert!(matches!(
                canonical_state.as_ref(),
                WindowsPathState::Same {
                    bytes_match: true,
                    ..
                }
            ));
            assert!(matches!(
                temp_state.as_ref(),
                WindowsPathState::Same {
                    links: 1,
                    bytes_match: true,
                    ..
                }
            ));
        }
        other => panic!("expected typed stable-state namespace failure, got {other:?}"),
    }
    assert_eq!(
        error.degraded_reason(),
        ManifestDegradedReason::PersistenceFailure
    );
    guard.release();
    drop(blocker);
    assert_eq!(std::fs::read(&canonical).unwrap(), original);
    assert!(exact_temp_paths(&project).is_empty());
}

#[cfg(windows)]
#[test]
fn writer_supports_non_ascii_verbatim_paths_beyond_max_path() {
    use std::os::windows::ffi::OsStrExt;

    let temp = tempfile::tempdir().unwrap();
    let mut project = temp.path().join("project");
    while project.as_os_str().encode_wide().count() < 430 {
        project.push("長い-path-component-0123456789abcdef");
    }
    std::fs::create_dir_all(project.join(".ac")).unwrap();
    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    guard.release();
    assert!(canonical_path(&project).is_file());
}

#[cfg(windows)]
#[test]
fn verbatim_conversion_covers_long_unc_paths_and_rejects_interior_nul() {
    use std::os::windows::ffi::OsStringExt;

    let unc = PathBuf::from(format!(
        "\\\\server\\share\\{}",
        "長い-unc-component-0123456789abcdef\\".repeat(12)
    ));
    let converted = absolute_verbatim_utf16(&unc).unwrap();
    let expected_prefix = "\\\\?\\UNC\\server\\share\\"
        .encode_utf16()
        .collect::<Vec<_>>();
    assert!(converted.starts_with(&expected_prefix));
    assert_eq!(converted.last(), Some(&0));
    assert!(converted.len() > 260);

    let with_nul = PathBuf::from(std::ffi::OsString::from_wide(&[
        u16::from(b'C'),
        u16::from(b':'),
        u16::from(b'\\'),
        u16::from(b'a'),
        0,
        u16::from(b'b'),
    ]));
    let error = absolute_verbatim_utf16(&with_nul).unwrap_err();
    assert!(matches!(
        error,
        SeedManifestError::Validation(message)
            if message == "Windows namespace path contains an interior NUL"
    ));
}

#[cfg(windows)]
#[test]
fn windows_namespace_power_loss_durability_is_explicitly_unclaimed() {
    assert!(!std::hint::black_box(
        WINDOWS_NAMESPACE_POWER_LOSS_DURABILITY_CLAIMED
    ));
}

#[cfg(windows)]
#[test]
fn named_stream_injected_into_temp_aborts_and_preserves_candidate() {
    let (_temp, project) = setup_project();
    let hooks = TestFilesystemHooks {
        before_temp_validation: Some(Arc::new(|temp| {
            std::fs::write(windows_ads_path(temp, "injected"), b"foreign stream").unwrap();
        })),
        ..TestFilesystemHooks::default()
    };
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
            .unwrap();
    let outcome = guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::UnsafePath)
    );
    guard.release();
    assert!(!canonical_path(&project).exists());
    assert_eq!(exact_temp_paths(&project).len(), 1);
}

#[cfg(windows)]
#[test]
fn read_only_canonical_is_preserved_without_creating_a_temp() {
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_READONLY;

    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let mut first = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        first.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    first.release();
    let canonical = canonical_path(&project);
    let original = std::fs::read(&canonical).unwrap();
    let attributes = OpenedRegularFile::open_existing(&canonical, false)
        .unwrap()
        .attributes;
    windows_set_attributes(&canonical, attributes | FILE_ATTRIBUTE_READONLY);

    let mut guard = ProjectSeedManifestGuard::acquire(&project).unwrap();
    let outcome = guard.publication_permit().record_file(
        &activation,
        context_row(
            "Context.coordinator.md",
            "context:coordinator",
            "2026-07-16T19:42:00.000Z",
        )
        .unwrap(),
    );
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::ReadOnlyCanonical)
    );
    guard.release();
    assert_eq!(std::fs::read(&canonical).unwrap(), original);
    assert!(exact_temp_paths(&project).is_empty());
    windows_set_attributes(&canonical, attributes & !FILE_ATTRIBUTE_READONLY);
}

#[cfg(windows)]
#[test]
fn injected_replace_errors_follow_1175_1176_and_1177_recovery_policy() {
    use windows_sys::Win32::Foundation::{
        ERROR_UNABLE_TO_MOVE_REPLACEMENT, ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
        ERROR_UNABLE_TO_REMOVE_REPLACED,
    };

    for raw_error in [
        ERROR_UNABLE_TO_REMOVE_REPLACED,
        ERROR_UNABLE_TO_MOVE_REPLACEMENT,
        ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
    ] {
        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            initial.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded
        );
        initial.release();
        let canonical = canonical_path(&project);
        let original = std::fs::read(&canonical).unwrap();
        let renamed_old = project.join(".ac").join("old-destination-recovery");
        let hooks = TestFilesystemHooks {
            windows_namespace_call: Some(Arc::new({
                let renamed_old = renamed_old.clone();
                move |operation, _temp, canonical| {
                    assert_eq!(operation, WindowsNamespaceOperation::ReplaceFile);
                    if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT {
                        std::fs::remove_file(canonical).unwrap();
                    } else if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT_2 {
                        std::fs::rename(canonical, &renamed_old).unwrap();
                    }
                    Err(raw_error)
                }
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        let outcome = guard.publication_permit().record_file(
            &activation,
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        );
        guard.release();

        if raw_error == ERROR_UNABLE_TO_REMOVE_REPLACED {
            assert_eq!(
                outcome,
                ManifestRecordOutcome::PublishedUnrecorded(
                    ManifestDegradedReason::PersistenceFailure
                )
            );
            assert_eq!(std::fs::read(&canonical).unwrap(), original);
            assert!(exact_temp_paths(&project).is_empty());
        } else {
            assert_eq!(
                outcome,
                ManifestRecordOutcome::PublishedUnrecorded(
                    ManifestDegradedReason::RecoveryRequired
                )
            );
            let candidates = exact_temp_paths(&project);
            assert_eq!(candidates.len(), 1);
            if raw_error == ERROR_UNABLE_TO_MOVE_REPLACEMENT {
                assert!(!canonical.exists());
                std::fs::write(
                    &canonical,
                    serialize_state(&ManifestState::default()).unwrap(),
                )
                .unwrap();
                let next = ProjectSeedManifestGuard::acquire(&project).unwrap();
                next.release();
                assert!(candidates[0].is_file());
            } else {
                assert!(!canonical.exists());
                assert_eq!(std::fs::read(&renamed_old).unwrap(), original);
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn injected_replace_error_matrix_retains_raw_code_and_never_retries() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_UNABLE_TO_MOVE_REPLACEMENT,
        ERROR_UNABLE_TO_MOVE_REPLACEMENT_2, ERROR_UNABLE_TO_REMOVE_REPLACED,
    };

    #[derive(Clone, Copy, Debug)]
    enum Mutation {
        StableNames,
        RemoveCanonical,
        RenameCanonical,
    }

    #[derive(Clone, Copy, Debug)]
    enum ExpectedError {
        StableFailure,
        RecoveryRequired,
        Partial(WindowsReplacePartial),
    }

    #[derive(Clone, Copy, Debug)]
    struct Case {
        label: &'static str,
        raw_error: u32,
        mutation: Mutation,
        expected: ExpectedError,
        canonical_present: bool,
        temp_present: bool,
        renamed_old_present: bool,
    }

    let cases = [
        Case {
            label: "1175 documented stable names",
            raw_error: ERROR_UNABLE_TO_REMOVE_REPLACED,
            mutation: Mutation::StableNames,
            expected: ExpectedError::StableFailure,
            canonical_present: true,
            temp_present: false,
            renamed_old_present: false,
        },
        Case {
            label: "1175 contradictory missing canonical",
            raw_error: ERROR_UNABLE_TO_REMOVE_REPLACED,
            mutation: Mutation::RemoveCanonical,
            expected: ExpectedError::RecoveryRequired,
            canonical_present: false,
            temp_present: true,
            renamed_old_present: false,
        },
        Case {
            label: "1176 documented removed canonical",
            raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT,
            mutation: Mutation::RemoveCanonical,
            expected: ExpectedError::Partial(
                WindowsReplacePartial::CanonicalRemovedReplacementAtTemp,
            ),
            canonical_present: false,
            temp_present: true,
            renamed_old_present: false,
        },
        Case {
            label: "1176 contradictory stable names",
            raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT,
            mutation: Mutation::StableNames,
            expected: ExpectedError::Partial(
                WindowsReplacePartial::CanonicalRemovedReplacementAtTemp,
            ),
            canonical_present: true,
            temp_present: true,
            renamed_old_present: false,
        },
        Case {
            label: "1177 documented renamed old destination",
            raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
            mutation: Mutation::RenameCanonical,
            expected: ExpectedError::Partial(
                WindowsReplacePartial::ReplacementEnrichedOldDestinationRenamed,
            ),
            canonical_present: false,
            temp_present: true,
            renamed_old_present: true,
        },
        Case {
            label: "1177 contradictory stable names",
            raw_error: ERROR_UNABLE_TO_MOVE_REPLACEMENT_2,
            mutation: Mutation::StableNames,
            expected: ExpectedError::Partial(
                WindowsReplacePartial::ReplacementEnrichedOldDestinationRenamed,
            ),
            canonical_present: true,
            temp_present: true,
            renamed_old_present: false,
        },
        Case {
            label: "other access denied stable names",
            raw_error: ERROR_ACCESS_DENIED,
            mutation: Mutation::StableNames,
            expected: ExpectedError::StableFailure,
            canonical_present: true,
            temp_present: false,
            renamed_old_present: false,
        },
        Case {
            label: "other sharing error contradictory state",
            raw_error: ERROR_SHARING_VIOLATION,
            mutation: Mutation::RemoveCanonical,
            expected: ExpectedError::RecoveryRequired,
            canonical_present: false,
            temp_present: true,
            renamed_old_present: false,
        },
    ];

    for case in cases {
        let (_temp, project) = setup_project();
        let activation = ManifestActivationToken::for_test();
        let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
        assert_eq!(
            initial.publication_permit().record_file(
                &activation,
                context_row(
                    "Context.AgentsCommander.md",
                    "context:agentscommander",
                    "2026-07-16T19:40:07.123Z",
                )
                .unwrap(),
            ),
            ManifestRecordOutcome::Recorded,
            "{}",
            case.label
        );
        initial.release();

        let canonical = canonical_path(&project);
        let original = std::fs::read(&canonical).unwrap();
        let renamed_old = project.join(".ac").join("renamed-old-destination");
        let call_count = Arc::new(AtomicUsize::new(0));
        let hooks = TestFilesystemHooks {
            windows_namespace_call: Some(Arc::new({
                let call_count = Arc::clone(&call_count);
                let renamed_old = renamed_old.clone();
                move |operation, _temp, canonical| {
                    assert_eq!(operation, WindowsNamespaceOperation::ReplaceFile);
                    call_count.fetch_add(1, Ordering::SeqCst);
                    match case.mutation {
                        Mutation::StableNames => {}
                        Mutation::RemoveCanonical => {
                            std::fs::remove_file(canonical).unwrap();
                        }
                        Mutation::RenameCanonical => {
                            std::fs::rename(canonical, &renamed_old).unwrap();
                        }
                    }
                    Err(case.raw_error)
                }
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        let mut desired = read_disk_state(&project);
        let _ = desired.upsert(
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        );
        let error = guard
            .write_canonical(serialize_state(&desired).unwrap())
            .unwrap_err();
        assert_eq!(call_count.load(Ordering::SeqCst), 1, "{}", case.label);

        let observed_raw = match &error {
            SeedManifestError::WindowsNamespaceFailure {
                raw_error,
                canonical_state,
                temp_state,
                ..
            } => {
                assert!(matches!(case.expected, ExpectedError::StableFailure));
                assert!(matches!(
                    canonical_state.as_ref(),
                    WindowsPathState::Same {
                        bytes_match: true,
                        ..
                    }
                ));
                assert!(matches!(
                    temp_state.as_ref(),
                    WindowsPathState::Same {
                        bytes_match: true,
                        ..
                    }
                ));
                *raw_error
            }
            SeedManifestError::WindowsReplaceRecoveryRequired { raw_error, .. } => {
                assert!(matches!(case.expected, ExpectedError::RecoveryRequired));
                *raw_error
            }
            SeedManifestError::WindowsReplacePartial {
                raw_error, partial, ..
            } => {
                let ExpectedError::Partial(expected_partial) = case.expected else {
                    panic!("{} returned an unexpected partial error", case.label);
                };
                assert_eq!(*partial, expected_partial);
                *raw_error
            }
            other => panic!("{} returned unexpected error {other:?}", case.label),
        };
        assert_eq!(observed_raw, case.raw_error, "{}", case.label);
        let display = format!("{error}");
        assert!(
            display.contains(&case.raw_error.to_string()),
            "{}",
            case.label
        );
        assert!(
            display.contains(&format!("0x{:08x}", case.raw_error)),
            "{}",
            case.label
        );
        guard.release();

        assert_eq!(canonical.exists(), case.canonical_present, "{}", case.label);
        if case.canonical_present {
            assert_eq!(
                std::fs::read(&canonical).unwrap(),
                original,
                "{}",
                case.label
            );
        }
        assert_eq!(
            exact_temp_paths(&project).len(),
            usize::from(case.temp_present),
            "{}",
            case.label
        );
        assert_eq!(
            renamed_old.exists(),
            case.renamed_old_present,
            "{}",
            case.label
        );
        if case.renamed_old_present {
            assert_eq!(
                std::fs::read(&renamed_old).unwrap(),
                original,
                "{}",
                case.label
            );
        }
    }
}

#[cfg(windows)]
#[test]
fn injected_move_failures_clean_only_the_exact_prepublication_state() {
    use windows_sys::Win32::Foundation::ERROR_ACCESS_DENIED;

    for move_before_error in [false, true] {
        let (_temp, project) = setup_project();
        let hooks = TestFilesystemHooks {
            windows_namespace_call: Some(Arc::new(move |operation, temp, canonical| {
                assert_eq!(operation, WindowsNamespaceOperation::MoveFileEx);
                if move_before_error {
                    std::fs::rename(temp, canonical).unwrap();
                }
                Err(ERROR_ACCESS_DENIED)
            })),
            ..TestFilesystemHooks::default()
        };
        let mut guard =
            ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
                .unwrap();
        let outcome = guard.publication_permit().record_file(
            &ManifestActivationToken::for_test(),
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        );
        guard.release();
        if move_before_error {
            assert_eq!(
                outcome,
                ManifestRecordOutcome::PublishedUnrecorded(
                    ManifestDegradedReason::RecoveryRequired
                )
            );
            assert!(canonical_path(&project).is_file());
        } else {
            assert_eq!(
                outcome,
                ManifestRecordOutcome::PublishedUnrecorded(
                    ManifestDegradedReason::PersistenceFailure
                )
            );
            assert!(!canonical_path(&project).exists());
            assert!(exact_temp_paths(&project).is_empty());
        }
    }
}

#[cfg(windows)]
#[test]
fn successful_call_with_substituted_final_identity_is_not_recorded_or_cleaned() {
    let (_temp, project) = setup_project();
    let foreign = b"foreign final object".to_vec();
    let hooks = TestFilesystemHooks {
        windows_namespace_call: Some(Arc::new({
            let foreign = foreign.clone();
            move |operation, temp, canonical| {
                assert_eq!(operation, WindowsNamespaceOperation::MoveFileEx);
                std::fs::rename(temp, canonical).unwrap();
                std::fs::remove_file(canonical).unwrap();
                std::fs::write(canonical, &foreign).unwrap();
                Ok(())
            }
        })),
        ..TestFilesystemHooks::default()
    };
    let mut guard =
        ProjectSeedManifestGuard::acquire_with_hooks(&project, Duration::from_secs(1), hooks)
            .unwrap();
    let outcome = guard.publication_permit().record_file(
        &ManifestActivationToken::for_test(),
        context_row(
            "Context.AgentsCommander.md",
            "context:agentscommander",
            "2026-07-16T19:40:07.123Z",
        )
        .unwrap(),
    );
    guard.release();
    assert_eq!(
        outcome,
        ManifestRecordOutcome::PublishedUnrecorded(ManifestDegradedReason::IdentityConflict)
    );
    assert_eq!(std::fs::read(canonical_path(&project)).unwrap(), foreign);
}

#[cfg(windows)]
#[test]
fn handle_bound_cleanup_never_deletes_a_substituted_path() {
    let (_temp, project) = setup_project();
    let path = project
        .join(".ac")
        .join(".seed-manifest.00000000-0000-0000-0000-000000000001.tmp");
    let mut opened = OpenedRegularFile::create_new(&path).unwrap();
    opened.file.write_all(b"owned").unwrap();
    opened.file.sync_all().unwrap();
    opened.refresh_facts().unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"substitution").unwrap();
    assert!(matches!(
        cleanup_current_owned_temp(&opened, Some(b"owned")),
        Err(SeedManifestError::UnsafePath { .. })
    ));
    drop(opened);
    assert_eq!(std::fs::read(path).unwrap(), b"substitution");
}

#[cfg(windows)]
#[test]
fn replace_preserves_certified_ntfs_metadata_surface_and_both_compression_directions() {
    use windows_sys::Win32::Storage::FileSystem::{
        COMPRESSION_FORMAT_DEFAULT, COMPRESSION_FORMAT_NONE, FILE_ATTRIBUTE_COMPRESSED,
    };

    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        initial.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    initial.release();

    let canonical = canonical_path(&project);
    let protected_dacl = windows_protect_existing_dacl(&canonical);
    let ads = windows_ads_path(&canonical, "agentscommander-stage-a");
    std::fs::write(&ads, b"destination stream").unwrap();
    windows_set_compression(&canonical, COMPRESSION_FORMAT_DEFAULT);
    let before = OpenedRegularFile::open_existing(&canonical, false).unwrap();
    assert_ne!(before.attributes & FILE_ATTRIBUTE_COMPRESSED, 0);
    let creation_time = before.creation_time;
    drop(before);

    let mut compressed = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        compressed.publication_permit().record_file(
            &activation,
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    compressed.release();
    let after_compressed = OpenedRegularFile::open_existing(&canonical, false).unwrap();
    assert_eq!(after_compressed.creation_time, creation_time);
    assert_ne!(after_compressed.attributes & FILE_ATTRIBUTE_COMPRESSED, 0);
    assert_eq!(windows_dacl_bytes(&canonical), protected_dacl);
    assert_eq!(std::fs::read(&ads).unwrap(), b"destination stream");
    drop(after_compressed);

    windows_set_compression(&canonical, COMPRESSION_FORMAT_NONE);
    assert_eq!(
        OpenedRegularFile::open_existing(&canonical, false)
            .unwrap()
            .attributes
            & FILE_ATTRIBUTE_COMPRESSED,
        0
    );
    let mut uncompressed = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        uncompressed.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:43:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    uncompressed.release();
    assert_eq!(
        OpenedRegularFile::open_existing(&canonical, false)
            .unwrap()
            .attributes
            & FILE_ATTRIBUTE_COMPRESSED,
        0
    );
    assert_eq!(std::fs::read(&ads).unwrap(), b"destination stream");
}

#[cfg(windows)]
#[test]
fn ordinary_attribute_inputs_are_observed_without_a_preservation_assertion() {
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_ARCHIVE, FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_NOT_CONTENT_INDEXED, FILE_ATTRIBUTE_SYSTEM,
    };

    let (_temp, project) = setup_project();
    let activation = ManifestActivationToken::for_test();
    let mut initial = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        initial.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:40:07.123Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    initial.release();
    let canonical = canonical_path(&project);
    let selected = FILE_ATTRIBUTE_ARCHIVE
        | FILE_ATTRIBUTE_HIDDEN
        | FILE_ATTRIBUTE_SYSTEM
        | FILE_ATTRIBUTE_NOT_CONTENT_INDEXED;
    windows_set_attributes(&canonical, selected);
    let mut set_case = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        set_case.publication_permit().record_file(
            &activation,
            context_row(
                "Context.coordinator.md",
                "context:coordinator",
                "2026-07-16T19:42:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    set_case.release();
    let final_set = OpenedRegularFile::open_existing(&canonical, false)
        .unwrap()
        .attributes
        & selected;

    windows_set_attributes(&canonical, FILE_ATTRIBUTE_NORMAL);
    let mut clear_case = ProjectSeedManifestGuard::acquire(&project).unwrap();
    assert_eq!(
        clear_case.publication_permit().record_file(
            &activation,
            context_row(
                "Context.AgentsCommander.md",
                "context:agentscommander",
                "2026-07-16T19:43:00.000Z",
            )
            .unwrap(),
        ),
        ManifestRecordOutcome::Recorded
    );
    clear_case.release();
    let final_clear = OpenedRegularFile::open_existing(&canonical, false)
        .unwrap()
        .attributes
        & selected;
    eprintln!(
        "ordinary attribute non-guarantee observation set=0x{final_set:08x} clear=0x{final_clear:08x}"
    );
}
