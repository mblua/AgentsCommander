#[derive(Clone)]
pub(crate) struct CommitCapability {
    pub(crate) _private: super::seal::Token,
}

#[cfg(test)]
impl CommitCapability {
    pub(crate) fn for_test() -> Self {
        Self {
            _private: super::seal::Token,
        }
    }
}
