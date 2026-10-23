use super::*;
use std::collections::HashMap;

/// A child that panics exactly where portable-pty does when its inner mutex is
/// poisoned: inside the accessor, with the caller holding the PTY map guard.
#[derive(Debug)]
struct PoisonedChild;

impl portable_pty::ChildKiller for PoisonedChild {
    fn kill(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn clone_killer(&self) -> Box<dyn portable_pty::ChildKiller + Send + Sync> {
        Box::new(PoisonedChild)
    }
}

impl portable_pty::Child for PoisonedChild {
    fn try_wait(&mut self) -> std::io::Result<Option<portable_pty::ExitStatus>> {
        panic!("called `Result::unwrap()` on a poisoned mutex");
    }

    fn wait(&mut self) -> std::io::Result<portable_pty::ExitStatus> {
        panic!("called `Result::unwrap()` on a poisoned mutex");
    }

    fn process_id(&self) -> Option<u32> {
        None
    }

    #[cfg(windows)]
    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        panic!("called `Result::unwrap()` on a poisoned mutex");
    }
}

/// grinch D4: the probe runs while the global `ptys` guard is held. A panic inside
/// portable-pty must not unwind through that guard, because a poisoned `ptys` map
/// makes the next terminal write panic, with nothing in the log tying it back here.
#[test]
fn a_panicking_child_probe_never_poisons_the_pty_map() {
    let ptys: Mutex<HashMap<Uuid, Box<dyn portable_pty::Child + Send + Sync>>> =
        Mutex::new(HashMap::new());
    let id = Uuid::new_v4();
    ptys.lock().unwrap().insert(
        id,
        Box::new(PoisonedChild) as Box<dyn portable_pty::Child + Send + Sync>,
    );

    {
        // Exactly the shape of `probe_child`: guard held, probe called under it.
        let mut guard = ptys.lock().unwrap();
        let child = guard.get_mut(&id).expect("child");
        let liveness = probe_child_contained(child);
        assert!(
            matches!(liveness, ChildLiveness::Unqueryable(_)),
            "a probe that could not ask must never claim the child is alive"
        );
    }

    assert!(
        !ptys.is_poisoned(),
        "the ptys guard must be released normally, not unwound through"
    );
    assert!(
        ptys.lock().is_ok(),
        "the next terminal write still gets the lock"
    );
}
