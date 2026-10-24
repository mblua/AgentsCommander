//! The user's category catalog (#2232 phase 6, plan section 5).
//!
//! The system knows **four destinations and nothing else**:
//! `user | orchestrator | root | default_reply`. The catalog file maps each
//! user-chosen category name to one of those four, carries the yes/no question
//! that decides it, and (only where the destination needs it) a peer FQN or a
//! fixed default reply.
//!
//! Every invalid case becomes a visible abstention, never a guess. Validation
//! is per category so one broken entry cannot disable its valid siblings, and
//! the rejection reason names the offending category. A `default_reply` that
//! could express the user's approval is rejected **at load time** (plan section
//! 8), so a bad catalog fails visibly once instead of silently on every turn.
//!
//! Catalog validation uses `serde` and delegates disk reads to `filesystem`. It
//! never names the phone, session or commands subtrees, either config module
//! named in plan section 4, or the network module, which is what keeps it out
//! of the 88-module SCC.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

/// The four destinations, and nothing else. Any other value is an invalid
/// category (plan section 5).
pub const DESTINATIONS: [&str; 4] = ["user", "orchestrator", "root", "default_reply"];

/// Phrases a `default_reply` may never contain, compared case-insensitively at
/// load time (plan section 8). A matched phrase makes its category invalid, so
/// the loader can never manufacture the user's approval.
const APPROVAL_DENY_PHRASES: [&str; 7] = [
    "approved",
    "go ahead",
    "the user agrees",
    "authorised",
    "authorized",
    "lgtm",
    "ship it",
];

/// One of the four legal destinations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Destination {
    User,
    Orchestrator,
    Root,
    DefaultReply,
}

/// Whether a category may be acted on, and why not when it may not.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Category {
    /// The category is usable; `peer` is present exactly for `orchestrator`
    /// and `reply` exactly for `default_reply`.
    Valid {
        destination: Destination,
        peer: Option<String>,
        reply: Option<String>,
    },
    /// The category is unusable; this is the visible abstention reason.
    Invalid { reason: String },
}

/// A catalog entry: the question that decides it plus its validity.
///
/// `question` is `None` only when the file omits or blanks it; such an entry
/// has no askable question and cannot be requested.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CategoryEntry {
    pub question: Option<String>,
    pub category: Category,
}

/// What `resolve` found for one returned category id.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Resolution {
    Valid {
        destination: Destination,
        peer: Option<String>,
        reply: Option<String>,
    },
    Invalid {
        reason: String,
    },
    /// The id is not in this catalog at all (or the catalog never loaded).
    Unknown,
}

#[derive(Debug)]
enum CatalogState {
    /// No catalog file: the feature is inert (plan section 5).
    Missing,
    /// The file exists but the whole catalog is unusable.
    Unparseable { reason: String },
    /// Per-category validation already happened.
    Loaded(BTreeMap<String, CategoryEntry>),
}

/// The loaded user catalog, or the reason it is missing or unusable.
#[derive(Debug)]
pub struct Catalog {
    state: CatalogState,
}

impl Catalog {
    /// The absent-catalog value: inert, reason `NoCatalogFile`.
    pub fn missing() -> Self {
        Self {
            state: CatalogState::Missing,
        }
    }

    /// Read and validate the catalog file. A missing file is [`Self::missing`];
    /// any other read failure or parse failure is an unusable catalog.
    pub fn load(path: &Path) -> Self {
        match crate::filesystem::read_capture_catalog(path) {
            Ok(raw) => Self::from_json_str(&raw),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::missing(),
            Err(error) => Self {
                state: CatalogState::Unparseable {
                    reason: format!("catalog is unreadable: {error}"),
                },
            },
        }
    }

    /// Parse and validate the catalog document. Kept separate from [`Self::load`]
    /// so tests and callers can validate a document without touching disk.
    pub fn from_json_str(raw: &str) -> Self {
        let document: CatalogDocument = match serde_json::from_str(raw) {
            Ok(document) => document,
            Err(error) => {
                return Self {
                    state: CatalogState::Unparseable {
                        reason: format!("catalog is not valid JSON: {error}"),
                    },
                };
            }
        };
        let mut categories = BTreeMap::new();
        for (name, raw_category) in document.categories {
            categories.insert(name.clone(), raw_category.validate(&name));
        }
        Self {
            state: CatalogState::Loaded(categories),
        }
    }

    /// True when no catalog file exists, the inert case of plan section 5.
    pub fn is_missing(&self) -> bool {
        matches!(self.state, CatalogState::Missing)
    }

    /// The reason the whole catalog is unusable, when it is.
    pub fn unparseable_reason(&self) -> Option<&str> {
        match &self.state {
            CatalogState::Unparseable { reason } => Some(reason),
            _ => None,
        }
    }

    /// Every askable `(category id, question)` pair, byte-sorted by id.
    ///
    /// The sort is explicit (`sort_unstable` over a `Vec`) so the wire order can
    /// never depend on a map's iteration order (plan section 6.1). Invalid
    /// categories with a question stay in: if one wins the classifier abstains
    /// with its load-time reason rather than silently dropping it.
    pub fn questions_in_order(&self) -> Vec<(&str, &str)> {
        let CatalogState::Loaded(categories) = &self.state else {
            return Vec::new();
        };
        let mut questions: Vec<(&str, &str)> = categories
            .iter()
            .filter_map(|(name, entry)| {
                entry
                    .question
                    .as_deref()
                    .map(|question| (name.as_str(), question))
            })
            .collect();
        questions.sort_unstable_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
        questions
    }

    /// Resolve one returned category id, valid or not.
    pub fn resolve(&self, id: &str) -> Resolution {
        let CatalogState::Loaded(categories) = &self.state else {
            return Resolution::Unknown;
        };
        match categories.get(id) {
            None => Resolution::Unknown,
            Some(entry) => match &entry.category {
                Category::Valid {
                    destination,
                    peer,
                    reply,
                } => Resolution::Valid {
                    destination: *destination,
                    peer: peer.clone(),
                    reply: reply.clone(),
                },
                Category::Invalid { reason } => Resolution::Invalid {
                    reason: reason.clone(),
                },
            },
        }
    }
}

#[derive(Deserialize)]
struct CatalogDocument {
    categories: BTreeMap<String, RawCategory>,
}

/// The on-disk shape before validation. Every field is optional here so one
/// malformed entry is captured as an invalid category instead of failing the
/// whole document and taking its valid siblings down with it.
#[derive(Deserialize)]
struct RawCategory {
    #[serde(default)]
    destination: Option<String>,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    peer: Option<String>,
    #[serde(default)]
    reply: Option<String>,
}

impl RawCategory {
    fn validate(&self, name: &str) -> CategoryEntry {
        let question = non_empty(self.question.as_deref());
        let Some(question) = question else {
            return CategoryEntry {
                question: None,
                category: Category::Invalid {
                    reason: format!("category \"{name}\": missing question"),
                },
            };
        };
        let Some(destination) = self.destination.as_deref().and_then(destination_from_name) else {
            let shown = self.destination.as_deref().unwrap_or("<missing>");
            return CategoryEntry {
                question: Some(question.to_string()),
                category: Category::Invalid {
                    reason: format!(
                        "category \"{name}\": unknown destination \"{shown}\" (allowed: {})",
                        DESTINATIONS.join(", ")
                    ),
                },
            };
        };
        match destination {
            Destination::Orchestrator => {
                let Some(peer) = non_empty(self.peer.as_deref()) else {
                    return CategoryEntry {
                        question: Some(question.to_string()),
                        category: Category::Invalid {
                            reason: format!(
                                "category \"{name}\": destination \"orchestrator\" requires a peer"
                            ),
                        },
                    };
                };
                CategoryEntry {
                    question: Some(question.to_string()),
                    category: Category::Valid {
                        destination,
                        peer: Some(peer.to_string()),
                        reply: None,
                    },
                }
            }
            Destination::DefaultReply => {
                let Some(reply) = non_empty(self.reply.as_deref()) else {
                    return CategoryEntry {
                        question: Some(question.to_string()),
                        category: Category::Invalid {
                            reason: format!(
                                "category \"{name}\": destination \"default_reply\" \
requires a reply"
                            ),
                        },
                    };
                };
                if let Some(phrase) = approval_phrase_in(reply) {
                    return CategoryEntry {
                        question: Some(question.to_string()),
                        category: Category::Invalid {
                            reason: format!(
                                "category \"{name}\": reply rejected, it contains \
the approval phrase \"{phrase}\""
                            ),
                        },
                    };
                }
                CategoryEntry {
                    question: Some(question.to_string()),
                    category: Category::Valid {
                        destination,
                        peer: None,
                        reply: Some(reply.to_string()),
                    },
                }
            }
            Destination::User | Destination::Root => CategoryEntry {
                question: Some(question.to_string()),
                category: Category::Valid {
                    destination,
                    peer: None,
                    reply: None,
                },
            },
        }
    }
}

fn destination_from_name(value: &str) -> Option<Destination> {
    match value {
        "user" => Some(Destination::User),
        "orchestrator" => Some(Destination::Orchestrator),
        "root" => Some(Destination::Root),
        "default_reply" => Some(Destination::DefaultReply),
        _ => None,
    }
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|text| !text.is_empty())
}

/// The first deny-list phrase the reply contains, case-insensitively.
fn approval_phrase_in(reply: &str) -> Option<&'static str> {
    let lowered = reply.to_lowercase();
    APPROVAL_DENY_PHRASES
        .iter()
        .copied()
        .find(|phrase| lowered.contains(phrase))
}

#[cfg(test)]
mod tests;
