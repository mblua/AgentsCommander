/// No-op Job Object for non-Windows builds. The Win32 tree-kill primitive does
/// not exist here, so PtyManager simply never holds one (`for_child` -> None).
pub struct JobObject;

impl JobObject {
    pub fn for_child(_pid: u32) -> Option<Self> {
        None
    }
    #[expect(clippy::unused_self, reason = "Keep the no-op JobObject method API.")]
    pub fn terminate(&self) {}
}

#[cfg(test)]
mod tests;
