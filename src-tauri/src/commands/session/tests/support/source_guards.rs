use std::collections::BTreeMap;
use std::path::PathBuf;

pub(crate) type SessionSources = BTreeMap<String, String>;
type SourcePredicate = fn(&SessionSources) -> bool;
const RESTART_REFUSAL: &str = "crate::config::archive_gate::probe_spawn_refusal(app,&cwd).await?;";
const SESSION_SOURCE: &str = "session.rs";
const CORE_SOURCE: &str = "session/create/core.rs";
const CREATION_SOURCE: &str = "session/create/core/creation.rs";
const CONTEXT_SOURCE: &str = "session/create/core/context.rs";
const BACKEND_SOURCE: &str = "session/create/core/backend.rs";
const PUBLICATION_SOURCE: &str = "session/create/core/publication.rs";
const RESTART_SOURCE: &str = "session/restart/execution.rs";
const CREATE_ENTRY: &str = "create_session_inner_core";
const RESTART_ENTRY: &str = "execute_restart_transaction_in_context";
const SPAWN: &str =
    "letspawn_result=PtyManager::spawn(pty_mgr,input.session.backend_kind,spawn_spec).await;";
const FENCE: &str = "t.session_may_exist=true;";
const UNFENCE: &str = "if!launch_witness.launched(){";
const MARK: &str = "letspawn_mark=";
const DROP_MARK: &str = "drop(spawn_mark);";
const PENDING: &str =
    "letpending_result=mgr.create_pending_session_with_binding_and_carrier_permit(";
const ROTATION: &str = "rotate_origin_memory_at_spawn";
const ROTATION_GATE: &str = "ifstart_fresh&&context_result.is_ok(){";
const FRESH: &str = "letstart_fresh=skip_auto_resume;";
const ARCHIVE_GATE: &str = concat!(
    "enforce_owned_unarchived_for_spawn(app,cwd,&session_label,create_owner,",
    "create_manager,lifecycle_domain,inline_transaction,)"
);

pub(crate) fn session_sources() -> SessionSources {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/commands");
    let mut sources = BTreeMap::new();
    for relative in [
        SESSION_SOURCE,
        "session/create.rs",
        "session/create/contracts.rs",
        "session/create/publication.rs",
        "session/create/admission.rs",
        "session/create/context_prep.rs",
        "session/create/effects.rs",
        "session/create/launch.rs",
        CORE_SOURCE,
        CREATION_SOURCE,
        CONTEXT_SOURCE,
        BACKEND_SOURCE,
        PUBLICATION_SOURCE,
        "session/restart.rs",
        "session/restart/context.rs",
        "session/restart/settlement.rs",
        RESTART_SOURCE,
    ] {
        let path = root.join(relative);
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                sources.insert(relative.to_string(), production_source(&source));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("read {}: {error}", path.display()),
        }
    }
    sources
}

fn normalized(source: &str) -> String {
    source.split_whitespace().collect()
}

fn syntax_mask(source: &str) -> String {
    let tokens = regex::Regex::new(r#"(?s)//[^\n]*|/\*.*?\*/|"(?:\\.|[^"\\])*""#).unwrap();
    tokens
        .replace_all(source, |m: &regex::Captures<'_>| " ".repeat(m[0].len()))
        .into_owned()
}

fn block_end(source: &str, start: usize) -> Option<usize> {
    let mask = syntax_mask(source);
    let opening = mask[start..].find('{')? + start;
    let mut depth = 0;
    for (offset, byte) in mask.as_bytes()[opening..].iter().enumerate() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(opening + offset + 1);
                }
            }
            _ => {}
        }
    }
    None
}

fn production_source(source: &str) -> String {
    let mut source = source.to_string();
    let attribute = regex::Regex::new(r"(?m)^[ \t]*#\[cfg\(test\)\]").unwrap();
    while let Some(found) = attribute.find(&syntax_mask(&source)) {
        let (start, after) = (found.start(), found.end());
        let mask = syntax_mask(&source);
        let rest = &mask[after..];
        let semicolon = rest.find(';');
        let brace = rest.find('{');
        let end = match (semicolon, brace) {
            (Some(semi), Some(open)) if semi < open => Some(after + semi + 1),
            (Some(semi), None) => Some(after + semi + 1),
            (_, Some(_)) => block_end(&source, after),
            _ => None,
        }
        .expect("complete cfg(test) item or statement");
        source.replace_range(start..end, "");
    }
    source
}

fn function(source: &str, name: &str) -> Option<String> {
    let start = source.find(&format!("fn {name}"))?;
    let end = block_end(source, start)?;
    Some(normalized(&source[start..end]))
}

fn definition_count(sources: &SessionSources, name: &str) -> usize {
    let entry = regex::Regex::new(&format!("fn {name}[<(]")).unwrap();
    sources.values().map(|s| entry.find_iter(s).count()).sum()
}

fn occurrences(sources: &SessionSources, needle: &str) -> usize {
    sources
        .values()
        .map(|s| normalized(s).matches(needle).count())
        .sum()
}

fn ordered(source: &str, needles: &[&str]) -> bool {
    needles
        .iter()
        .try_fold(0, |after, needle| {
            source[after..]
                .find(needle)
                .map(|at| after + at + needle.len())
        })
        .is_some()
}

fn create_stages(sources: &SessionSources) -> Option<[String; 4]> {
    let split = [
        CORE_SOURCE,
        CONTEXT_SOURCE,
        BACKEND_SOURCE,
        PUBLICATION_SOURCE,
    ];
    if split.iter().any(|path| sources.contains_key(*path)) {
        Some([
            expanded_function(sources, CORE_SOURCE, CREATE_ENTRY)?,
            expanded_function(sources, CONTEXT_SOURCE, "prepare")?,
            expanded_function(sources, BACKEND_SOURCE, "spawn")?,
            expanded_function(sources, PUBLICATION_SOURCE, "complete")?,
        ])
    } else {
        let whole = function(sources.get(SESSION_SOURCE)?, CREATE_ENTRY)?;
        Some([whole.clone(), whole.clone(), whole.clone(), whole])
    }
}

pub(crate) fn source_map_complete(sources: &SessionSources) -> bool {
    let restart = if sources.contains_key(RESTART_SOURCE) {
        RESTART_SOURCE
    } else {
        SESSION_SOURCE
    };
    create_stages(sources).is_some()
        && definition_count(sources, CREATE_ENTRY) == 1
        && definition_count(sources, RESTART_ENTRY) == 1
        && sources
            .get(restart)
            .is_some_and(|s| function(s, RESTART_ENTRY).is_some())
}

pub(crate) fn source_chain(sources: &SessionSources) -> bool {
    let Some([core, context, backend, _]) = create_stages(sources) else {
        return false;
    };
    if sources.contains_key(CORE_SOURCE) {
        [
            (&core, "context::prepare("),
            (&context, "backend::spawn("),
            (&backend, "publication::complete("),
        ]
        .iter()
        .all(|(body, call)| body.matches(call).count() == 1 && body.contains(".await"))
            && ["prepare", "spawn", "complete"]
                .iter()
                .all(|name| definition_count(sources, name) == 1)
    } else {
        source_map_complete(sources)
    }
}

fn first_effect(sources: &SessionSources) -> &str {
    if sources.contains_key(CORE_SOURCE) {
        "context::prepare("
    } else {
        SPAWN
    }
}

pub(crate) fn source_pending(sources: &SessionSources) -> bool {
    let Some([core, _, _, _]) = create_stages(sources) else {
        return false;
    };
    occurrences(sources, PENDING) == 1 && ordered(&core, &[PENDING, first_effect(sources)])
}

pub(crate) fn source_fence(sources: &SessionSources) -> bool {
    let Some([_, _, backend, _]) = create_stages(sources) else {
        return false;
    };
    [FENCE, SPAWN, UNFENCE]
        .iter()
        .all(|needle| occurrences(sources, needle) == 1)
        && occurrences(sources, "has_backend_session") == 0
        && ordered(&backend, &[FENCE, SPAWN, UNFENCE])
}

pub(crate) fn source_mark(sources: &SessionSources) -> bool {
    let Some([core, _, backend, _]) = create_stages(sources) else {
        return false;
    };
    occurrences(sources, MARK) == 1
        && occurrences(sources, DROP_MARK) == 1
        && ordered(&core, &[MARK, PENDING, first_effect(sources)])
        && ordered(&backend, &[SPAWN, DROP_MARK, "ifletErr(e)=spawn_result{"])
        && source_archive(sources)
}

pub(crate) fn source_effect(sources: &SessionSources) -> bool {
    let Some([_, _, backend, _]) = create_stages(sources) else {
        return false;
    };
    ordered(
        &backend,
        &[
            SPAWN,
            "ifletSome(effect)=backend_effect{effect.complete(",
            DROP_MARK,
            "ifletErr(e)=spawn_result{",
        ],
    )
}

pub(crate) fn source_rotation(sources: &SessionSources) -> bool {
    let Some([_, context, _, _]) = create_stages(sources) else {
        return false;
    };
    let call = format!("{ROTATION_GATE}crate::config::agent_memory::{ROTATION}(&cwd);}}");
    context.matches(&call).count() == 1
}

pub(crate) fn source_fresh(sources: &SessionSources) -> bool {
    occurrences(sources, "letstart_fresh=") == 1 && occurrences(sources, FRESH) == 1
}

pub(crate) fn source_archive(sources: &SessionSources) -> bool {
    let Some([core, _, _, _]) = create_stages(sources) else {
        return false;
    };
    occurrences(sources, ARCHIVE_GATE) == 2 && ordered(&core, &[ARCHIVE_GATE, MARK, PENDING])
}

pub(crate) fn source_budgets(sources: &SessionSources) -> bool {
    occurrences(sources, "note_coordinator_create_on_app(app,cwd)") == 1
        && occurrences(sources, ROTATION) == 2
        && occurrences(sources, "`rotate_origin_memory_at_spawn`returns`()`") == 1
}

pub(crate) fn source_restart(sources: &SessionSources) -> bool {
    let Some(source) = sources.get(RESTART_SOURCE) else {
        return false;
    };
    let Some(entry) = function(source, RESTART_ENTRY) else {
        return false;
    };
    let Some(preparation) = function(source, "prepare_restart") else {
        return false;
    };
    let Some(retirement) = function(source, "begin_restart_retirement") else {
        return false;
    };
    [RESTART_ENTRY, "prepare_restart", "begin_restart_retirement"]
        .iter()
        .all(|name| definition_count(sources, name) == 1)
        && ordered(
            &entry,
            &[
                "letprepared=prepare_restart(",
                ".await;",
                "}=prepared?;",
                "letretirement=begin_restart_retirement(transaction,context).await?;",
                "teardown_old_for_restart(transaction,uuid,old_was_dormant).await",
            ],
        )
        && ordered(
            &preparation,
            &[
                RESTART_REFUSAL,
                concat!(
                    "crate::config::sessions_persistence::enforce_creation_gate",
                    "(settings,&cwd,enforcement).await?;"
                ),
            ],
        )
        && ordered(
            &retirement,
            &[
                "letretirement=",
                "retire_lifecycle_entry_protection(app,authority).await",
                "Ok(retirement)",
            ],
        )
}

fn all_source_predicates(sources: &SessionSources) -> bool {
    source_map_complete(sources)
        && source_chain(sources)
        && source_pending(sources)
        && source_fence(sources)
        && source_mark(sources)
        && source_effect(sources)
        && source_rotation(sources)
        && source_fresh(sources)
        && source_archive(sources)
        && source_budgets(sources)
        && source_restart(sources)
}

type SourceLink = (&'static str, &'static str, &'static str, &'static str);
const LINKS: &[SourceLink] = &[
    (
        CREATE_ENTRY,
        "publication::await_startup(",
        PUBLICATION_SOURCE,
        "await_startup",
    ),
    (CREATE_ENTRY, "create_body(", CORE_SOURCE, "create_body"),
    (
        "create_body",
        "reserve_launch(",
        CORE_SOURCE,
        "reserve_launch",
    ),
    (
        "reserve_launch",
        "publication::mirror_fresh_intent(",
        PUBLICATION_SOURCE,
        "mirror_fresh_intent",
    ),
    (
        "reserve_launch",
        "create_pending(",
        CORE_SOURCE,
        "create_pending",
    ),
    (
        "create_pending",
        "context::bind_and_check_target(",
        CONTEXT_SOURCE,
        "bind_and_check_target",
    ),
    (
        "create_pending",
        "context::pending_is_observed(",
        CONTEXT_SOURCE,
        "pending_is_observed",
    ),
    (
        "create_pending",
        "context::apply_pending_options(",
        CONTEXT_SOURCE,
        "apply_pending_options",
    ),
    (
        "create_pending",
        "prepare_metadata(",
        CORE_SOURCE,
        "prepare_metadata",
    ),
    (
        "prepare_metadata",
        "context::apply_profile_metadata(",
        CONTEXT_SOURCE,
        "apply_profile_metadata",
    ),
    (
        "prepare_metadata",
        "creation::context_target(",
        CREATION_SOURCE,
        "context_target",
    ),
    (
        "prepare",
        "prepare_resume(",
        CONTEXT_SOURCE,
        "prepare_resume",
    ),
    (
        "prepare_resume",
        "prepare_instructions(",
        CONTEXT_SOURCE,
        "prepare_instructions",
    ),
    (
        "prepare_instructions",
        "run_context_work(",
        CONTEXT_SOURCE,
        "run_context_work",
    ),
    (
        "spawn",
        "record_effective_args(",
        BACKEND_SOURCE,
        "record_effective_args",
    ),
    (
        "spawn",
        "prepare_environment(",
        BACKEND_SOURCE,
        "prepare_environment",
    ),
    ("spawn", "spawn_backend(", BACKEND_SOURCE, "spawn_backend"),
    (
        "complete",
        "publish_tooling(",
        PUBLICATION_SOURCE,
        "publish_tooling",
    ),
    ("complete", "finalize(", PUBLICATION_SOURCE, "finalize"),
];

fn expanded_function(sources: &SessionSources, path: &str, name: &str) -> Option<String> {
    let mut body = function(sources.get(path)?, name)?;
    for &(_, call, target, helper) in LINKS.iter().filter(|link| link.0 == name) {
        if body.matches(call).count() != 1 || definition_count(sources, helper) != 1 {
            return None;
        }
        let callee = expanded_function(sources, target, helper)?;
        body = body.replacen(call, &format!("{call}{callee}"), 1);
    }
    Some(body)
}

pub(crate) fn expanded_creation(sources: &SessionSources) -> Option<String> {
    create_stages(sources).map(|stages| stages[0].clone())
}

#[cfg(test)]
mod tests;
