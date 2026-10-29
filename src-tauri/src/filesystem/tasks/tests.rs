use std::path::PathBuf;

// Failures are injected only in fixtures, in this thread and this module.
thread_local! {
    pub(crate) static IO_MUTATION: std::cell::RefCell<Option<(String, PathBuf, Vec<u8>)>> =
        const { std::cell::RefCell::new(None) };
    pub(crate) static IO_FAULT: std::cell::RefCell<(Option<String>, Vec<String>)> =
        const { std::cell::RefCell::new((None, Vec::new())) };
}
pub(crate) fn io_boundary(name: &str) -> std::io::Result<()> {
    if std::env::var("AC_TASK_OPS_CHILD_BOUNDARY").as_deref() == Ok(name)
        && std::env::var_os("AC_TASK_OPS_CHILD_FIXTURE").is_some()
    {
        std::process::exit(71);
    }
    IO_MUTATION.with(|v| {
        let mut v = v.borrow_mut();
        if v.as_ref().is_some_and(|(point, _, _)| point == name) {
            let (_, path, bytes) = v.take().unwrap();
            std::fs::write(path, bytes).unwrap();
        }
    });
    IO_FAULT.with(|f| {
        let mut f = f.borrow_mut();
        f.1.push(name.into());
        if f.0.as_deref() == Some(name) {
            Err(std::io::Error::other(format!("fixture: {name}")))
        } else {
            Ok(())
        }
    })
}

#[test]
fn boundary_consumes_mutation_before_trace_and_fault() {
    struct Slots {
        mutation: Option<(String, PathBuf, Vec<u8>)>,
        fault: (Option<String>, Vec<String>),
        path: PathBuf,
    }
    impl Drop for Slots {
        fn drop(&mut self) {
            IO_MUTATION.with(|slot| *slot.borrow_mut() = self.mutation.take());
            IO_FAULT.with(|slot| *slot.borrow_mut() = std::mem::take(&mut self.fault));
            let _ = std::fs::remove_file(&self.path);
        }
    }
    let mut thread_hash = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(&std::thread::current().id(), &mut thread_hash);
    let path = std::env::temp_dir().join(format!(
        "ac-task-boundary-{}-{}.tmp",
        std::process::id(),
        std::hash::Hasher::finish(&thread_hash)
    ));
    let _slots = Slots {
        mutation: IO_MUTATION.with(|slot| slot.borrow_mut().take()),
        fault: IO_FAULT.with(|slot| std::mem::take(&mut *slot.borrow_mut())),
        path: path.clone(),
    };
    std::fs::write(&path, b"before").unwrap();
    IO_MUTATION.with(|slot| {
        *slot.borrow_mut() = Some(("control".into(), path.clone(), b"changed".to_vec()));
    });
    IO_FAULT.with(|slot| slot.borrow_mut().0 = Some("control".into()));
    assert_eq!(
        io_boundary("control").unwrap_err().to_string(),
        "fixture: control"
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"changed");
    IO_MUTATION.with(|slot| assert!(slot.borrow().is_none()));
    IO_FAULT.with(|slot| {
        assert_eq!(slot.borrow().1, ["control"]);
        slot.borrow_mut().0 = None;
    });
    std::fs::write(&path, b"after").unwrap();
    io_boundary("control").unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), b"after");
    IO_FAULT.with(|slot| assert_eq!(slot.borrow().1, ["control", "control"]));
}
