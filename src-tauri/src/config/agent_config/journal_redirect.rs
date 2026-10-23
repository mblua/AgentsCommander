use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    static DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub(crate) fn current() -> Option<PathBuf> {
    DIR.with(|dir| dir.borrow().clone())
}

/// Restores the previous directory on drop.
#[must_use]
pub(crate) struct Guard {
    previous: Option<PathBuf>,
}

pub(crate) fn set(dir: &Path) -> Guard {
    let previous = DIR.with(|cell| cell.replace(Some(dir.to_path_buf())));
    Guard { previous }
}

impl Drop for Guard {
    fn drop(&mut self) {
        DIR.with(|cell| *cell.borrow_mut() = self.previous.take());
    }
}
