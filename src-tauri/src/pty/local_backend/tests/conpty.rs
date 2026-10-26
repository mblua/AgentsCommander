use super::*;
use std::io::Read;
use std::path::Path;
use std::time::Duration;
#[test]
fn issue_3029_cursor_query_stream_contract() {
    let mut cases: Vec<(Vec<u8>, Vec<u8>, usize)> = vec![
        (b"\x1b[6n".to_vec(), vec![], 1),
        (
            b"before\x1b[6nafter\x1b[6n".to_vec(),
            b"beforeafter\x1b[6n".to_vec(),
            1,
        ),
    ];
    for bytes in [
        &b"plain \xc3\xa9"[..],
        b"\x1b[?6n",
        b"\x1b[6 n",
        b"\x1b[16n",
        b"\x1b[6\x07n",
        b"\x1b",
        b"\x1b[",
        b"\x1b[6",
        b"\x1b[6\x18n",
        b"\x1b[6\x1an",
        b"\xc2\x9b6n",
    ] {
        cases.push((bytes.to_vec(), bytes.to_vec(), 0));
    }
    for &opener in b"]PX^_" {
        for cancel in [false, true] {
            let mut prefix = vec![27, opener];
            prefix.extend_from_slice(b"literal\x1b[6n");
            if cancel {
                prefix.push(0x18);
            } else {
                prefix.extend_from_slice(&[27, 92]);
            }
            let mut input = prefix.clone();
            input.extend_from_slice(b"\x1b[6n");
            cases.push((input, prefix, 1));
        }
    }
    cases.push((
        b"\x1b]title\x1b[6n\x07\x1b[6n".to_vec(),
        b"\x1b]title\x1b[6n\x07".to_vec(),
        1,
    ));
    cases.push((
        b"\x1bP\x07\x1b[6n\x1a\x1b[6n".to_vec(),
        b"\x1bP\x07\x1b[6n\x1a".to_vec(),
        1,
    ));
    cases.push((b"\x1b\x1b[6n".to_vec(), vec![27], 1));
    for (input, expected, replies) in cases {
        // Every two-part split, plus all bytewise partitions of the four-byte query.
        for split in 0..=input.len() {
            let mut filter = OneShotCursorQuery::new();
            let mut actual = vec![];
            let mut count = 0;
            for chunk in [&input[..split], &input[split..]] {
                filter.feed(chunk, |bytes, query| {
                    actual.extend_from_slice(bytes);
                    count += usize::from(query);
                });
                assert!(filter.pending.len() <= 3);
            }
            filter.finish(|bytes| actual.extend_from_slice(bytes));
            assert_eq!(
                (actual, count),
                (expected.clone(), replies),
                "input {input:?}, split {split}"
            );
        }
    }
    for partitions in 0..8 {
        let mut filter = OneShotCursorQuery::new();
        let mut count = 0;
        let mut output = vec![];
        let mut start = 0;
        for end in 1..=4 {
            if end == 4 || partitions & (1 << (end - 1)) != 0 {
                filter.feed(&b"\x1b[6n"[start..end], |bytes, query| {
                    output.extend_from_slice(bytes);
                    count += usize::from(query);
                });
                assert!(filter.pending.len() <= 3);
                start = end;
            }
        }
        assert_eq!(count, 1);
        assert!(output.is_empty());
    }
}

struct CursorTestWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
    fail: u8,
}
impl Write for CursorTestWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let mut saved = self.bytes.lock().unwrap();
        if self.fail == 1 && !saved.is_empty() {
            return Err(std::io::Error::other("partial"));
        }
        let n = if self.fail == 1 { 1 } else { bytes.len() };
        saved.extend_from_slice(&bytes[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.fail == 2 {
            Err(std::io::Error::other("flush"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn issue_3029_cursor_reply_order_and_failure() {
    use std::sync::mpsc;
    // A channel only in the fake: caller-controlled EOF/Stop proves failure retains the drainer.
    struct Frames(mpsc::Receiver<Vec<u8>>);
    impl Read for Frames {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            match self.0.recv_timeout(Duration::from_secs(5)) {
                Ok(frame) => {
                    out[..frame.len()].copy_from_slice(&frame);
                    Ok(frame.len())
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => Ok(0),
                Err(error) => Err(std::io::Error::other(error)),
            }
        }
    }
    for fail in 0..5 {
        let saved = Arc::new(Mutex::new(vec![]));
        let original: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(CursorTestWriter {
                bytes: saved.clone(),
                fail,
            })));
        let weak = Arc::downgrade(&original);
        let replacement_bytes = Arc::new(Mutex::new(vec![]));
        let replacement: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(CursorTestWriter {
                bytes: replacement_bytes.clone(),
                fail: 0,
            })));
        let mut map = HashMap::from([(Uuid::nil(), original)]);
        // An already upgraded original remains valid, never redirects to the replacement.
        let retained = if fail == 4 { None } else { weak.upgrade() };
        map.insert(Uuid::nil(), replacement);
        let detector = IdleDetector::new(|_| {}, |_| {});
        let policy = Arc::new(Mutex::new(Vec::new()));
        let notices = policy.clone();
        assert!(detector
            .bind_raw_output_observer(1, move |id, at| notices.lock().unwrap().push((id, at))));
        let fanout = SessionIoFanout::new(Arc::new(Mutex::new(HashMap::new())), detector, None);
        let id = Uuid::new_v4();
        let token = fanout
            .register_session_for_test(id, crate::session::profile::IdleTuning::DEFAULT, 24, 80)
            .unwrap();
        let observer = Arc::new(CursorReplyObserver::default());
        let raw = Arc::new(Mutex::new(vec![]));
        let output = Arc::new(Mutex::new(vec![]));
        let (tx, rx) = mpsc::channel();
        let (seen_tx, seen_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();
        let (final_tx, final_rx) = mpsc::channel();
        let policy_worker = policy.clone();
        let observed = observer.clone();
        let bytes = saved.clone();
        let raw_copy = raw.clone();
        let out = output.clone();
        let worker = std::thread::spawn(move || {
            pump_cursor_query(
                &mut Frames(rx),
                |chunk| {
                    let at = Instant::now();
                    raw_copy.lock().unwrap().extend_from_slice(chunk);
                    let admitted = fanout.observe_raw_read_for_reader(&token, chunk.len(), at);
                    if fail == 3 && chunk == b"FINAL" {
                        assert!(!admitted);
                        final_tx.send(()).unwrap();
                    }
                    admitted
                },
                |chunk| {
                    fanout.handle_output(&token, &id.to_string(), chunk.to_vec());
                    out.lock().unwrap().extend_from_slice(chunk);
                    if chunk.ends_with(b"TAIL\x1b[6n") || chunk == b"FINAL" {
                        seen_tx.send(chunk.to_vec()).unwrap();
                    }
                },
                || {
                    assert_eq!(&*out.lock().unwrap(), b"\x1b[3;4Hhi");
                    assert_eq!(fanout.cursor_position_for_reader(&token), Some((3, 6)));
                    if fail == 3 {
                        fanout.remove_session(id);
                        fanout
                            .register_session_for_test(
                                id,
                                crate::session::profile::IdleTuning::DEFAULT,
                                24,
                                80,
                            )
                            .unwrap();
                    }
                    let result =
                        write_cursor_reply(&weak, fanout.cursor_position_for_reader(&token));
                    assert!(weak
                        .upgrade()
                        .is_none_or(|writer| writer.try_lock().is_ok()));
                    result
                },
                Some(&observed),
            );
            if fail == 3 {
                let replacement = fanout.registration_token_for_session(id).unwrap();
                assert_eq!(
                    fanout.cursor_position_for_reader(&replacement),
                    Some((1, 1))
                );
                assert_eq!(fanout.get_screen_snapshot(id).unwrap().sequence, 0);
            }
            assert!(policy_worker
                .lock()
                .unwrap()
                .iter()
                .all(|(observed_id, _)| *observed_id == id));
            drop(retained);
            done_tx.send(()).unwrap();
        });
        tx.send(b"\x1b[3;4Hhi\x1b[6nTAIL\x1b[6n".to_vec()).unwrap();
        seen_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("tail after reply failure");
        assert_eq!(observer.consumed.load(Ordering::SeqCst), 1);
        assert_eq!(
            observer.written.load(Ordering::SeqCst),
            usize::from(fail == 0)
        );
        assert!(!observer.finished.load(Ordering::SeqCst));
        assert!(
            map.contains_key(&Uuid::nil()),
            "reader must not clean up the instance"
        );
        let policy_before_final = policy.lock().unwrap().len();
        tx.send(b"FINAL".to_vec()).unwrap();
        if fail == 3 {
            final_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("actual FINAL read drained with refused admission");
        } else {
            seen_rx
                .recv_timeout(Duration::from_secs(5))
                .expect("Stop must retain final-frame drainer");
        }
        drop(tx);
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("EOF finishes reader");
        worker.join().unwrap();
        assert!(observer.finished.load(Ordering::SeqCst));
        if fail == 3 {
            assert_eq!(&*output.lock().unwrap(), b"\x1b[3;4HhiTAIL\x1b[6n");
            assert_eq!(policy.lock().unwrap().len(), policy_before_final);
            assert!(
                seen_rx.try_recv().is_err(),
                "refused FINAL never reaches filtered callback"
            );
        } else {
            assert_eq!(&*output.lock().unwrap(), b"\x1b[3;4HhiTAIL\x1b[6nFINAL");
            assert!(policy.lock().unwrap().len() > policy_before_final);
        }
        assert_eq!(&*raw.lock().unwrap(), b"\x1b[3;4Hhi\x1b[6nTAIL\x1b[6nFINAL");
        assert!(replacement_bytes.lock().unwrap().is_empty());
        match fail {
            0 => assert_eq!(&*bytes.lock().unwrap(), b"\x1b[3;6R"),
            1 => assert_eq!(&*bytes.lock().unwrap(), &[27]),
            2 => assert_eq!(&*bytes.lock().unwrap(), b"\x1b[3;6R"),
            _ => assert!(bytes.lock().unwrap().is_empty()),
        }
    }
}

#[test]
fn issue_3029_cursor_query_positive_control() {
    struct Startup {
        phase: u8,
        reply: Arc<Mutex<Vec<u8>>>,
    }
    impl Read for Startup {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let bytes: &[u8] = match self.phase {
                0 => b"\x1b[6n",
                1 => {
                    let deadline = Instant::now() + Duration::from_millis(100);
                    while &*self.reply.lock().unwrap() != b"\x1b[1;1R" {
                        if Instant::now() >= deadline {
                            self.phase = 2;
                            return Ok(0);
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    b"READY"
                }
                _ => return Ok(0),
            };
            self.phase += 1;
            out[..bytes.len()].copy_from_slice(bytes);
            Ok(bytes.len())
        }
    }
    let saved = Arc::new(Mutex::new(vec![]));
    let mut control = Startup {
        phase: 0,
        reply: saved.clone(),
    };
    let mut output = vec![];
    control.read_to_end(&mut output).unwrap();
    assert_eq!(
        &output, b"\x1b[6n",
        "fixture must withhold READY without CPR"
    );
    for terminal_error in [false, true] {
        for complete in [false, true] {
            let saved = Arc::new(Mutex::new(Vec::new()));
            let writer: Arc<Mutex<Box<dyn Write + Send>>> =
                Arc::new(Mutex::new(Box::new(CursorTestWriter {
                    bytes: saved.clone(),
                    fail: 0,
                })));

            let observer = CursorReplyObserver::default();
            let detector = IdleDetector::new(|_| {}, |_| {});
            let notices = Arc::new(Mutex::new(Vec::new()));
            let captured = notices.clone();
            assert!(detector.bind_raw_output_observer(1, move |id, at| captured
                .lock()
                .unwrap()
                .push((id, at))));
            let fanout = SessionIoFanout::new(Arc::new(Mutex::new(HashMap::new())), detector, None);
            let id = Uuid::new_v4();
            let token = fanout
                .register_session_for_test(id, crate::session::profile::IdleTuning::DEFAULT, 24, 80)
                .unwrap();
            struct FiniteReads {
                frames: std::collections::VecDeque<Vec<u8>>,
                reply: Arc<Mutex<Vec<u8>>>,
                terminal_error: bool,
            }
            impl Read for FiniteReads {
                fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                    let Some(bytes) = self.frames.pop_front() else {
                        return if self.terminal_error {
                            Err(std::io::Error::other("finite read error"))
                        } else {
                            Ok(0)
                        };
                    };
                    if bytes == b"READY" {
                        assert_eq!(&*self.reply.lock().unwrap(), b"\x1b[1;4R");
                    }
                    out[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
            }
            let mut reader = FiniteReads {
                frames: if complete {
                    vec![
                        b"abc".to_vec(),
                        vec![27],
                        b"[".to_vec(),
                        b"6n".to_vec(),
                        b"READY".to_vec(),
                    ]
                    .into()
                } else {
                    vec![b"abc".to_vec(), vec![27], b"[".to_vec()].into()
                },
                reply: saved.clone(),
                terminal_error,
            };
            let mut output = vec![];
            let mut read_times = vec![];
            pump_cursor_query(
                &mut reader,
                |bytes| {
                    let at = Instant::now();
                    read_times.push(at);
                    let before = notices.lock().unwrap().len();
                    let admitted = fanout.observe_raw_read_for_reader(&token, bytes.len(), at);
                    assert!(admitted);
                    assert_eq!(notices.lock().unwrap()[before], (id, at));
                    admitted
                },
                |bytes| {
                    fanout.handle_output(&token, &id.to_string(), bytes.to_vec());
                    output.extend_from_slice(bytes);
                },
                || {
                    write_cursor_reply(
                        &Arc::downgrade(&writer),
                        fanout.cursor_position_for_reader(&token),
                    )
                },
                Some(&observer),
            );
            assert_eq!(
                output.as_slice(),
                if complete {
                    &b"abcREADY"[..]
                } else {
                    &b"abc\x1b["[..]
                }
            );
            assert_eq!(
                read_times.len(),
                if complete { 5 } else { 3 },
                "EOF/error/zero produces no prefilter callback; retained prefix may notify later"
            );
            assert!(notices.lock().unwrap().len() >= read_times.len());
            assert_eq!(
                observer.consumed.load(Ordering::SeqCst),
                usize::from(complete)
            );
            assert_eq!(
                observer.written.load(Ordering::SeqCst),
                usize::from(complete)
            );
            assert!(observer.finished.load(Ordering::SeqCst));
            let snapshot = fanout.get_screen_snapshot(id).unwrap();
            assert!(!snapshot.data.windows(4).any(|bytes| bytes == b"\x1b[6n"));
        }
    }
}

fn cursor_test_backend() -> LocalProcessBackend {
    let sessions = Arc::new(tokio::sync::RwLock::new(
        crate::session::manager::SessionManager::new(),
    ));
    LocalProcessBackend::new(
        Arc::new(Mutex::new(HashMap::new())),
        IdleDetector::new(|_| {}, |_| {}),
        GitWatcher::new_headless_for_tests(sessions),
        None,
    )
}

fn cursor_test_spec(cwd: &Path, payload: &str) -> BackendSpawnSpec {
    BackendSpawnSpec {
        id: Uuid::new_v4(),
        agent_id: None,
        coding_agent: None,
        cmd: "cmd.exe".into(),
        args: vec!["/D".into(), "/Q".into(), "/C".into(), payload.into()],
        resolved_agent_host_shell: None,
        cwd: cwd.to_string_lossy().into_owned(),
        selected_cwd: None,
        cols: 80,
        rows: 24,
        container_image: None,
        configured_env: vec![],
        env_remove_keys: vec![],
        env_unset: vec![],
        extra_env: vec![],
        idle_tuning: crate::session::profile::IdleTuning::DEFAULT,
        output_target: crate::pty::output::PtyOutputTarget::noop(),
        resource_registration: None,
        logical_resource_slot: None,
        container_credential: None,
        container_repo_mounts: vec![],
        launch_witness: Default::default(),
    }
}

fn cursor_wait(mut predicate: impl FnMut() -> bool, description: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline, "deadline: {description}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

struct CursorSessionCleanup {
    backend: LocalProcessBackend,
    id: Uuid,
    process: windows_sys::Win32::Foundation::HANDLE,
}
impl CursorSessionCleanup {
    fn new(backend: &LocalProcessBackend, id: Uuid) -> Self {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
        };
        let pid = backend.ptys.lock().unwrap()[&id]
            .child
            .as_ref()
            .unwrap()
            .process_id()
            .unwrap();
        let process = unsafe {
            OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if process.is_null() {
            backend.kill(id).expect("cleanup after OpenProcess failure");
            panic!("cannot capture own child PID {pid}");
        }
        eprintln!("I3029 child PID={pid}");
        Self {
            backend: backend.clone(),
            id,
            process,
        }
    }
    fn stop(&mut self) {
        self.backend.kill(self.id).expect("Stop cleanup");
        assert!(!self.backend.ptys.lock().unwrap().contains_key(&self.id));
        assert!(self
            .backend
            .fanout
            .registration_token_for_session(self.id)
            .is_none());
        assert_eq!(
            unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(self.process, 5000)
            },
            windows_sys::Win32::Foundation::WAIT_OBJECT_0,
            "own child must be terminated"
        );
    }
}
impl Drop for CursorSessionCleanup {
    fn drop(&mut self) {
        // Cleanup is always driven by the caller, including when an assertion unwinds.
        let result = self.backend.kill(self.id);
        let exited = unsafe {
            windows_sys::Win32::System::Threading::WaitForSingleObject(self.process, 5000)
        };
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.process);
        }
        if std::thread::panicking() {
            eprintln!("I3029 cleanup result={result:?}, wait={exited}");
        } else {
            assert!(result.is_ok());
            assert_eq!(exited, windows_sys::Win32::Foundation::WAIT_OBJECT_0);
        }
    }
}

fn cursor_marker(backend: &LocalProcessBackend, id: Uuid, marker: &str) -> bool {
    backend
        .fanout
        .get_screen_snapshot(id)
        .is_some_and(|snapshot| String::from_utf8_lossy(&snapshot.data).contains(marker))
}

fn cursor_host_observer(observer: &CursorReplyObserver) {
    // cmd's version banner supplies the exact build without adding a product OS API.
    let version = std::process::Command::new("cmd.exe")
        .args(["/D", "/C", "ver"])
        .output()
        .unwrap();
    assert!(version.status.success());
    let version = String::from_utf8_lossy(&version.stdout);
    let consumed = observer.consumed.load(Ordering::SeqCst);
    let written = observer.written.load(Ordering::SeqCst);
    eprintln!("I3029 {version} consumed={consumed} written={written}");
    if version.contains("10.0.26300.9711") {
        assert_eq!(
            (consumed, written),
            (1, 1),
            "host B requires autonomous CPR"
        );
    }
}

#[test]
fn issue_3029_windows_hidden_startup_and_late_attach() {
    let cwd = tempfile::tempdir().unwrap();
    let backend = cursor_test_backend();
    let spec = cursor_test_spec(cwd.path(), "echo AC3029_READY");
    let id = spec.id;
    let observer = Arc::new(CursorReplyObserver::default());
    backend
        .cursor_observers
        .lock()
        .unwrap()
        .insert(id, observer.clone());
    backend.spawn_sync(spec).unwrap();
    let mut cleanup = CursorSessionCleanup::new(&backend, id);
    cursor_wait(
        || cursor_marker(&backend, id, "AC3029_READY"),
        "hidden first paint",
    );
    cursor_wait(
        || {
            matches!(
                backend.probe_child(id),
                ChildLiveness::Exited {
                    code: 0,
                    success: true
                }
            )
        },
        "exit0 before attach",
    );
    cursor_host_observer(&observer);
    let attached = backend
        .fanout
        .activate_terminal_output(id, "3029-test", true)
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&attached.data).contains("AC3029_READY"));
    assert!(!attached.data.windows(4).any(|bytes| bytes == b"\x1b[6n"));
    cleanup.stop();
}

#[test]
fn issue_3029_windows_input_resize_stop() {
    let cwd = tempfile::tempdir().unwrap();
    let backend = cursor_test_backend();
    for stop_while_waiting in [false, true] {
        let spec = cursor_test_spec(
            cwd.path(),
            "echo AC3029_READY & set /p AC3029_INPUT= & echo AC3029_AFTER",
        );
        let id = spec.id;
        let observer = Arc::new(CursorReplyObserver::default());
        backend
            .cursor_observers
            .lock()
            .unwrap()
            .insert(id, observer.clone());
        backend.spawn_sync(spec).unwrap();
        let mut cleanup = CursorSessionCleanup::new(&backend, id);
        cursor_wait(
            || cursor_marker(&backend, id, "AC3029_READY"),
            "ready before attach/input",
        );
        cursor_host_observer(&observer);
        backend.resize(id, 73, 19).unwrap();
        cursor_wait(
            || backend.fanout.get_pty_size(id) == Some((19, 73)),
            "parser resize",
        );
        assert_eq!(
            backend.ptys.lock().unwrap()[&id]
                .master
                .lock()
                .unwrap()
                .get_size()
                .map(|s| (s.rows, s.cols))
                .unwrap(),
            (19, 73)
        );
        let snapshot = backend
            .fanout
            .activate_terminal_output(id, "3029-input", true)
            .unwrap()
            .unwrap();
        assert!(String::from_utf8_lossy(&snapshot.data).contains("AC3029_READY"));
        assert!(!snapshot.data.windows(4).any(|bytes| bytes == b"\x1b[6n"));
        if !stop_while_waiting {
            write_to_local_pty(&backend.ptys, id, b"ok\r\n").unwrap();
            cursor_wait(
                || cursor_marker(&backend, id, "AC3029_AFTER"),
                "input delivered",
            );
            cursor_wait(
                || {
                    matches!(
                        backend.probe_child(id),
                        ChildLiveness::Exited {
                            code: 0,
                            success: true
                        }
                    )
                },
                "input exit0",
            );
        } else {
            assert!(matches!(backend.probe_child(id), ChildLiveness::Alive));
        }
        cleanup.stop();
    }
}

use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::sync::{Arc, Barrier};
use windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ;

fn assert_no_temp_files(dir: &Path) {
    for entry in fs::read_dir(dir).expect("read temp dir") {
        let name = entry
            .expect("dir entry")
            .file_name()
            .to_string_lossy()
            .to_string();
        assert!(!name.ends_with(".tmp"), "unexpected temp file: {name}");
    }
}

#[allow(clippy::permissions_set_readonly_false)]
#[test]
fn git_guard_writer_skips_unchanged_readonly_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("git-guard.ps1");
    fs::write(&path, "same").expect("write fixture");
    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).expect("set readonly");

    let result = write_git_guard_file_if_changed(&path, "same", &GitGuardTempSequence::new());

    let mut permissions = fs::metadata(&path).expect("metadata").permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).expect("clear readonly");

    result.expect("unchanged readonly file should be skipped");
    assert_eq!(fs::read_to_string(&path).expect("read file"), "same");
    assert_no_temp_files(dir.path());
}

#[test]
fn git_guard_writer_replaces_changed_file_without_temp_leftover() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("git.cmd");
    fs::write(&path, "old").expect("write fixture");

    write_git_guard_file_if_changed(&path, "new", &GitGuardTempSequence::new())
        .expect("replace changed file");

    assert_eq!(fs::read_to_string(&path).expect("read file"), "new");
    assert_no_temp_files(dir.path());
}

#[test]
fn git_guard_writer_retries_in_use_destination_until_released() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("git.cmd");
    fs::write(&path, "old").expect("write fixture");
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .expect("open destination without delete sharing");
    let barrier = Arc::new(Barrier::new(2));
    let writer_barrier = Arc::clone(&barrier);
    let writer_path = path.clone();

    let writer = std::thread::spawn(move || {
        writer_barrier.wait();
        write_git_guard_file_if_changed(&writer_path, "new", &GitGuardTempSequence::new())
    });

    barrier.wait();
    std::thread::sleep(Duration::from_millis(1000));
    drop(held);

    writer
        .join()
        .expect("writer thread")
        .expect("retry should publish after held handle is released");
    assert_eq!(fs::read_to_string(&path).expect("read file"), "new");
    assert_no_temp_files(dir.path());
}

#[allow(clippy::permissions_set_readonly_false)]
#[test]
fn git_guard_sequence_shared_unique_and_noop() {
    let owners = crate::AppOwners::new();
    let clone = owners.clone();
    let independent = crate::AppOwners::new();
    assert!(Arc::ptr_eq(&owners.git_guard, &clone.git_guard));
    assert!(!Arc::ptr_eq(&owners.git_guard, &independent.git_guard));
    let sessions = Arc::new(tokio::sync::RwLock::new(
        crate::session::manager::SessionManager::new(),
    ));
    let backend = LocalProcessBackend::with_owners(
        Arc::new(Mutex::new(HashMap::new())),
        IdleDetector::new(|_| {}, |_| {}),
        GitWatcher::new_headless_for_tests(sessions),
        None,
        owners.git_guard.clone(),
    );
    assert!(Arc::ptr_eq(&backend.git_guard, &owners.git_guard));
    assert!(Arc::ptr_eq(&backend.clone().git_guard, &owners.git_guard));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("git.cmd");
    let barrier = Arc::new(Barrier::new(5));
    let writers: Vec<_> = (0..4)
        .map(|_| {
            let sequence = clone.git_guard.clone();
            let barrier = barrier.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                barrier.wait();
                write_git_guard_file_if_changed(&path, "shared", &sequence).unwrap();
            })
        })
        .collect();
    barrier.wait();
    for writer in writers {
        writer.join().unwrap();
    }
    let used = owners.git_guard.0.load(Ordering::Relaxed);
    assert!((1..=4).contains(&used));
    assert_eq!(independent.git_guard.0.load(Ordering::Relaxed), 0);
    assert_eq!(fs::read_to_string(&path).unwrap(), "shared");
    assert_no_temp_files(dir.path());
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();
    let result = write_git_guard_file_if_changed(&path, "shared", &clone.git_guard);
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
    result.unwrap();
    assert_eq!(owners.git_guard.0.load(Ordering::Relaxed), used);

    // Independent zero-start roots collide with a preserved reservation sentinel.
    let path = dir.path().join("independent.cmd");
    let sentinel = dir
        .path()
        .join(format!(".independent.cmd.{}.0.tmp", std::process::id()));
    fs::write(&sentinel, "sentinel").unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let roots = [crate::AppOwners::new(), crate::AppOwners::new()];
    let writers: Vec<_> = roots
        .iter()
        .map(|root| {
            let sequence = root.git_guard.clone();
            let barrier = barrier.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                barrier.wait();
                write_git_guard_file_if_changed(&path, "complete", &sequence).unwrap();
            })
        })
        .collect();
    barrier.wait();
    for writer in writers {
        writer.join().unwrap();
    }
    assert_eq!(fs::read_to_string(&sentinel).unwrap(), "sentinel");
    assert_eq!(fs::read_to_string(&path).unwrap(), "complete");
    assert!(roots
        .iter()
        .any(|root| root.git_guard.0.load(Ordering::Relaxed) >= 2));
    fs::remove_file(&sentinel).unwrap();
    assert_no_temp_files(dir.path());
    let sequence = GitGuardTempSequence::new();
    let error = git_guard::write_git_guard_file_atomic(
        &dir.path().join("missing").join("git.cmd"),
        b"bytes",
        &sequence,
    )
    .unwrap_err();
    assert!(error.starts_with("create temp "), "{error}");
    assert_eq!(sequence.0.load(Ordering::Relaxed), 1);
    assert_no_temp_files(dir.path());

    let barrier = Arc::new(Barrier::new(5));
    let values: Vec<_> = (0..4)
        .map(|_| {
            let sequence = owners.git_guard.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                sequence.next()
            })
        })
        .collect();
    barrier.wait();
    let mut values: Vec<_> = values.into_iter().map(|t| t.join().unwrap()).collect();
    values.sort_unstable();
    assert_eq!(values, (used..used + 4).collect::<Vec<_>>());
}
