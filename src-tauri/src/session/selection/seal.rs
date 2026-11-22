//! Sealing primitives for selection authority traits.
//!
//! `Token` is a private field marker and `Sealed` the supertrait that keeps
//! `authority` traits implementable only inside `session::selection`. No state, no logic.

#[derive(Clone)]
pub(crate) struct Token;

pub(crate) trait Sealed {}
