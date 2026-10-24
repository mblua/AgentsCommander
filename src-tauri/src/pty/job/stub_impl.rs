/// No-op Job Object for non-Windows builds. The Win32 tree-kill primitive does
/// not exist here, so PtyManager simply never holds one (`for_child` -> None).
/// `#[allow(dead_code)]` because the unit struct is never constructed on this
/// platform (for_child always returns None).
#[allow(dead_code)]
pub struct JobObject;

impl JobObject {
    pub fn for_child(_pid: u32) -> Option<Self> {
        None
    }
    pub fn terminate(&self) {}
}

#[cfg(test)]
mod tests;
