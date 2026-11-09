//! #1157 - operator-editable templates for the messages `AgentsCommander` injects
//! into a coordinator PTY.
//!
//! The registry lives next to the executable, in `config_dir()`, as
//! `injected-messages.toml`. It is seeded on first start, refreshed per entry
//! when a newer default ships, and never overwritten once the operator has
//! edited an entry. Rendering never fails: any resolution problem degrades to
//! the embedded default.
//!
//! Three properties carry the design and must survive any future edit:
//!
//! - **Surgical writes only.** The writer locates a `template` value through
//!   [`toml::Spanned`] and replaces exactly that literal. A whole-document
//!   `toml::to_string` would drop every comment, reorder keys and rewrite `'''`
//!   as `"""`, which is precisely what the seeded file exists to preserve.
//! - **Verify before publish.** No candidate is ever written without being
//!   re-parsed first, so AC can never turn the operator's own config into an
//!   invalid file.
//! - **One authoritative forbidden-scalar set.** [`sanitize`](render::sanitize) derives
//!   its filter from `crate::pty_input::is_forbidden_pty_scalar`, the same predicate
//!   `validate_pty_input_text` uses, so the two can never drift.

mod provision;
mod registry;
mod render;

pub(crate) use provision::ensure_injected_messages;
pub(crate) use provision::known_message_ids;
pub(crate) use provision::reseed;
#[cfg(test)]
use provision::{provision, verify_candidate};
pub(crate) use registry::registry;
pub use registry::MessageWarnings;
pub(crate) use registry::ReseedTarget;
pub(crate) use registry::INJECTED_MESSAGES_FILENAME;
pub(crate) use registry::MAX_RENDERED_BYTES;
#[cfg(test)]
pub(crate) use registry::{
    EntryOutcome, ProvisionReport, COVERAGE_VERSION, DEFAULT_CI_STARTED_TEMPLATE,
    DEFAULT_CONTEXT_ALERT_TEMPLATE, DEFAULT_NOTICE_BLIND_GAP_TEMPLATE,
    INJECTED_MESSAGES_REFERENCE_FILENAME, INJECTED_MESSAGES_STATE_FILENAME, SCHEMA_VERSION,
};
pub(crate) use registry::{
    BRANCH_STALE_MESSAGE_ID, CI_FINISHED_MESSAGE_ID, CI_STARTED_MESSAGE_ID,
    CONTEXT_ALERT_MESSAGE_ID, NOTICE_BLIND_GAP_MESSAGE_ID,
};
pub(crate) use registry::{
    TOKEN_AT, TOKEN_BASE, TOKEN_BEHIND, TOKEN_BRANCH, TOKEN_GAP, TOKEN_MEMBER, TOKEN_OBSERVED,
    TOKEN_REPO, TOKEN_SHA, TOKEN_SINCE, TOKEN_THRESHOLDS, TOKEN_WORKGROUP,
};
pub(crate) use render::render;
#[cfg(test)]
pub(crate) use render::render_with_template;
#[cfg(test)]
use render::{expand_tokens, render_resolved, sanitize};

#[cfg(test)]
mod tests;
