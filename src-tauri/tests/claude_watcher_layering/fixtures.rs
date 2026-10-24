use super::*;

pub(crate) struct Fixture {
    pub(crate) root: PathBuf,
    pub(crate) manifest: PathBuf,
    cleanup_on_drop: bool,
    drop_cleanup: Option<FixtureDropCleanup>,
}

impl Fixture {
    pub(crate) fn new(label: &str) -> Self {
        let parent = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("claude-watcher-layering");
        fs::create_dir_all(&parent).expect("fixture parent should be created");
        let counter = FIXTURE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let root = parent.join(format!("{label}-{}-{nanos}-{counter}", std::process::id()));
        fs::create_dir(&root).unwrap_or_else(|error| {
            panic!(
                "fixture directory {} should be exclusive: {error}",
                root.display()
            )
        });
        let manifest = root.join("crate");
        fs::create_dir(&manifest).expect("fixture manifest should be created");
        Self {
            root,
            manifest,
            cleanup_on_drop: true,
            drop_cleanup: None,
        }
    }

    pub(crate) fn with_telegram(label: &str) -> Self {
        let fixture = Self::new(label);
        fixture.write("src/lib.rs", "mod telegram;\n");
        fixture
    }

    pub(crate) fn write(&self, relative: &str, text: &str) {
        let path = self.manifest.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap_or_else(|error| {
                panic!(
                    "fixture parent {} could not be created: {error}",
                    parent.display()
                )
            });
        }
        fs::write(&path, text).unwrap_or_else(|error| {
            panic!(
                "fixture file {} could not be written: {error}",
                path.display()
            )
        });
    }

    pub(crate) fn write_outside_manifest(&self, relative: &str, text: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("outside fixture parent should be created");
        }
        fs::write(&path, text).expect("outside fixture file should be written");
    }

    pub(crate) fn index(&self) -> Result<ModuleIndex, GuardError> {
        self.index_as("fixture")
    }

    pub(crate) fn index_as(&self, crate_id: &str) -> Result<ModuleIndex, GuardError> {
        build_module_index(&self.manifest, &self.manifest.join("src/lib.rs"), crate_id)
    }

    pub(crate) fn cleanup(mut self) -> Result<(), GuardError> {
        self.cleanup_on_drop = false;
        fs::remove_dir_all(&self.root).map_err(|error| {
            GuardError::new(
                "fixture cleanup",
                format!(
                    "could not remove isolated fixture {}: {error}",
                    self.root.display()
                ),
            )
        })
    }
}

type FixtureRemoveDirAll = Box<dyn FnOnce(&Path) -> std::io::Result<()>>;

struct FixtureDropCleanup {
    remove_dir_all: FixtureRemoveDirAll,
    diagnostic: Box<dyn std::io::Write>,
}

pub(crate) struct RecordingDiagnostic {
    pub(crate) bytes: std::rc::Rc<std::cell::RefCell<Vec<u8>>>,
}

impl std::io::Write for RecordingDiagnostic {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.bytes.borrow_mut().extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) struct FailingDiagnostic {
    pub(crate) write_calls: std::rc::Rc<std::cell::Cell<usize>>,
}

impl std::io::Write for FailingDiagnostic {
    fn write(&mut self, _buffer: &[u8]) -> std::io::Result<usize> {
        self.write_calls.set(self.write_calls.get() + 1);
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "simulated diagnostic writer failure",
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Fixture {
    pub(crate) fn with_drop_cleanup(
        mut self,
        remove_dir_all: impl FnOnce(&Path) -> std::io::Result<()> + 'static,
        diagnostic: impl std::io::Write + 'static,
    ) -> Self {
        self.drop_cleanup = Some(FixtureDropCleanup {
            remove_dir_all: Box::new(remove_dir_all),
            diagnostic: Box::new(diagnostic),
        });
        self
    }
}

pub(crate) fn cleanup_fixture_on_drop(
    root: &Path,
    remove_dir_all: impl FnOnce(&Path) -> std::io::Result<()>,
    mut diagnostic: impl std::io::Write,
) {
    if let Err(error) = remove_dir_all(root) {
        let report_result = std::io::Write::write_fmt(
            &mut diagnostic,
            format_args!(
                "fixture cleanup fallback could not remove isolated fixture {}: {error}\n",
                root.display()
            ),
        );
        if let Err(report_error) = report_result {
            // Reporting failure cannot safely trigger another panic from Drop.
            drop(report_error);
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if self.cleanup_on_drop {
            if let Some(cleanup) = self.drop_cleanup.take() {
                cleanup_fixture_on_drop(&self.root, cleanup.remove_dir_all, cleanup.diagnostic);
            } else {
                cleanup_fixture_on_drop(
                    &self.root,
                    |root| fs::remove_dir_all(root),
                    std::io::stderr().lock(),
                );
            }
        }
    }
}

pub(crate) fn inline_spelling_report(label: &str, body: &str) -> Result<GuardReport, GuardError> {
    let fixture = Fixture::with_telegram(label);
    fixture.write(
        "src/telegram.rs",
        &format!(
            "pub mod bridge {{ pub struct BridgeLogger; }}\npub mod claude_watcher {{\n{body}\n}}\n"
        ),
    );
    let result = fixture
        .index_as(CRATE_ID)
        .and_then(|index| analyze_module_index(&index, CRATE_ID, TARGET_MODULE));
    let cleanup = fixture.cleanup();
    match (result, cleanup) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(GuardError::new(
            "fixture analysis and cleanup",
            format!("primary failure: {primary}; cleanup failure: {cleanup}"),
        )),
    }
}

pub(crate) fn out_of_line_spelling_report(
    label: &str,
    probe: &str,
) -> Result<GuardReport, GuardError> {
    let fixture = Fixture::with_telegram(label);
    fixture.write(
        "src/telegram.rs",
        "pub mod bridge { pub struct BridgeLogger; }\npub mod claude_watcher;\n",
    );
    fixture.write(
        "src/telegram/claude_watcher.rs",
        "#[cfg(any())]\n#[path = \"claude_watcher_layering_probe.rs\"]\nmod layering_probe;\n",
    );
    fixture.write("src/telegram/claude_watcher_layering_probe.rs", probe);
    let result = fixture
        .index_as(CRATE_ID)
        .and_then(|index| analyze_module_index(&index, CRATE_ID, TARGET_MODULE));
    let cleanup = fixture.cleanup();
    match (result, cleanup) {
        (Ok(report), Ok(())) => Ok(report),
        (Err(primary), Ok(())) => Err(primary),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Err(cleanup)) => Err(GuardError::new(
            "fixture analysis and cleanup",
            format!("primary failure: {primary}; cleanup failure: {cleanup}"),
        )),
    }
}
