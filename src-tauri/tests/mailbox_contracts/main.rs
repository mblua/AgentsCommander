//! Mailbox contracts over the production module tree (#5190).
//!
//! `mailbox_production_contracts` discovers the real tree once and runs both
//! contracts over it. The two control tests run the same discovery and verifier
//! on temporary trees, so every rejection is proven to name its contract and path.

mod contracts;
mod provenance;
mod sources;

use std::fs;
use std::path::Path;

use contracts::{verify, Report, CONTEXT_INFO};
use sources::discover;

const FACADE: &str = "src-tauri/src/phone/mailbox.rs";
const DIR: &str = "src-tauri/src/phone/mailbox";

#[test]
fn mailbox_production_contracts() {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the repository root");
    let tree = discover(repo, FACADE).unwrap_or_else(|e| panic!("discovery: {e}"));
    for event in &tree.events {
        println!("{event}");
    }
    let report = verify(&tree).unwrap_or_else(|errors| panic!("{}", errors.join("\n")));
    assert_eq!(
        report,
        Report {
            context_info: 1,
            context_warn: 1,
            remote_info: 1,
            delivered_variants: 2,
        }
    );
}

// ------------------------------------------------------------------ fixtures

const FACADE_SRC: &str = "\
mod clocks;
mod delivery;
#[cfg(test)]
mod tests;

pub fn after_tests() {}
";

const CLOCKS_SRC: &str = "\
use crate::config::coordinator_clocks::CoordinatorClocks;

// A comment naming clocks.mark_auto_closed(&marker_fqn, t) is data.
pub fn read(clocks: &CoordinatorClocks) -> bool {
    let example = \"clocks.mark_auto_closed(&marker_fqn, t)\";
    clocks.auto_closed_at(example).is_some()
}

#[cfg(test)]
fn inline_restore(clocks: &mut CoordinatorClocks, fqn: &str, t: u64) {
    clocks.mark_auto_closed(&fqn, t);
}
";

const DELIVERY_SRC: &str = "\
use crate::config::teams::split_project_prefix;

pub struct Mailbox;

impl Mailbox {
    async fn deliver(&self, session_id: Uuid, target: &InternalSystemTarget,
        notice: &InternalSystemNotice) -> Result<(), String> {
        match self.inject_internal_system_notice(session_id, target, notice).await {
            Ok(()) => return Ok(()),
            Err(error) if err_is_pty_session_missing(&error) => {
                log::warn!(
                    \"[context-alert] coordinator session {session_id} vanished \\
                    before injection: {error}\"
                );
            }
            Err(error) => return Err(error),
        }
        Ok(())
    }

    async fn inject_internal_system_notice(&self, session_id: Uuid,
        target: &InternalSystemTarget, notice: &InternalSystemNotice) -> Result<(), String> {
        let payload = render(notice);
        #[cfg(test)]
        if let Some(hooks) = &self.test_hooks {
            return Ok(());
        }
        crate::pty::inject::inject_internal_notice_into_supported_agent_session(
            session_id, &payload, |s| guard(s),
        )
        .await?;
        app.note_post_boundary_content_to_session(session_id).await;
        Self::log_internal_system_notice_delivery(session_id, target, notice);
        Ok(())
    }

    fn log_internal_system_notice_delivery(session_id: Uuid,
        target: &InternalSystemTarget, notice: &InternalSystemNotice) {
        let (project, _) = split_project_prefix(target.fqn());
        match notice {
            InternalSystemNotice::ContextAlert { member, workgroup, observed, thresholds } => {
                log::info!(
                    \"[context-alert] delivered coordinatorSession={} project={} \\
                    workgroup={} member={} observed={} thresholds={:?}\",
                    session_id,
                    project.unwrap_or(\"\"),
                    workgroup,
                    member,
                    observed,
                    thresholds
                );
            }
            InternalSystemNotice::RemoteActivity { kind, .. } => {
                log::info!(
                    \"[remote-activity] delivered coordinatorSession={} kind={:?}\",
                    session_id,
                    kind
                );
            }
        }
    }
}
";

const TESTS_SRC: &str = "\
mod nested;

#[test]
fn restores_in_tests() {
    clocks.lock().unwrap().mark_auto_closed(&fqn, t);
}
";

const NESTED_SRC: &str = "\
pub fn transitive(clocks: &mut CoordinatorClocks, marker_fqn: &str, t: u64) {
    clocks.mark_auto_closed(&marker_fqn, t);
    let restore = CoordinatorClocks::mark_auto_closed;
    log::info!(\"[context-alert] delivered only in a test\");
}
";

/// A temporary repository holding a mailbox tree; paths are relative to the facade dir.
struct Fixture {
    files: Vec<(String, String)>,
}

impl Fixture {
    fn base() -> Self {
        Fixture {
            files: vec![
                ("../mailbox.rs".into(), FACADE_SRC.into()),
                ("clocks.rs".into(), CLOCKS_SRC.into()),
                ("delivery.rs".into(), DELIVERY_SRC.into()),
                ("tests.rs".into(), TESTS_SRC.into()),
                ("tests/nested.rs".into(), NESTED_SRC.into()),
            ],
        }
    }

    fn get(&mut self, file: &str) -> &mut String {
        &mut self
            .files
            .iter_mut()
            .find(|(f, _)| f == file)
            .unwrap_or_else(|| panic!("fixture file {file}"))
            .1
    }

    /// Replaces exactly one occurrence of `from` in `file`.
    fn edit(mut self, file: &str, from: &str, to: &str) -> Self {
        let text = self.get(file);
        assert_eq!(text.matches(from).count(), 1, "{file}: {from:?}");
        *text = text.replacen(from, to, 1);
        self
    }

    fn append(mut self, file: &str, text: &str) -> Self {
        self.get(file).push_str(text);
        self
    }

    fn add(mut self, file: &str, text: &str) -> Self {
        self.files.push((file.into(), text.into()));
        self
    }

    fn rename(mut self, from: &str, to: &str) -> Self {
        let slot = self
            .files
            .iter_mut()
            .find(|(f, _)| f == from)
            .expect("rename source");
        slot.0 = to.into();
        self
    }

    fn remove(mut self, file: &str) -> Self {
        self.files.retain(|(f, _)| f != file);
        self
    }

    fn write(&self) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("temp dir");
        let base = dir.path().join(DIR);
        fs::create_dir_all(&base).expect("mailbox dir");
        for (file, text) in &self.files {
            let path = base.join(file);
            fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
            fs::write(path, text).expect("write fixture");
        }
        dir
    }

    fn run(&self) -> Result<Report, Vec<String>> {
        let dir = self.write();
        let tree = discover(dir.path(), FACADE).map_err(|e| vec![format!("discovery: {e}")])?;
        verify(&tree)
    }

    fn accepts(self, why: &str) {
        if let Err(errors) = self.run() {
            panic!("{why}: rejected:\n{}", errors.join("\n"));
        }
    }

    /// Requires a rejection whose diagnostic names `contract` and `file`.
    fn rejects(self, why: &str, contract: &str, file: &str) {
        let path = if file.starts_with("src-tauri") {
            file.to_string()
        } else {
            format!("{DIR}/{file}")
        };
        match self.run() {
            Ok(report) => panic!("{why}: accepted with {report:?}"),
            Err(errors) => assert!(
                errors
                    .iter()
                    .any(|e| e.contains(contract) && e.contains(&path)),
                "{why}: want {contract:?} at {path}, got:\n{}",
                errors.join("\n")
            ),
        }
    }
}

const T10: &str = "T10 close-marker restore";
const DELIVERY: &str = "delivery.rs";

#[test]
fn mailbox_contract_controls() {
    let report = Fixture::base()
        .run()
        .unwrap_or_else(|e| panic!("base: {}", e.join("\n")));
    assert_eq!(
        report,
        Report {
            context_info: 1,
            context_warn: 1,
            remote_info: 1,
            delivered_variants: 2
        }
    );
    assert!(CONTEXT_INFO.starts_with("[context-alert] delivered "));
    close_marker_controls();
    argument_controls();
    format_and_role_controls();
    delivery_path_controls();
    provenance_controls();
    guard_and_exit_controls();
    scope_controls();
}

fn close_marker_controls() {
    let restore =
        "\npub fn restore(clocks: &mut CoordinatorClocks, marker_fqn: &str, t: u64) {\n    \
                   clocks.mark_auto_closed(&marker_fqn, t);\n}\n";
    Fixture::base().append("../mailbox.rs", restore).rejects(
        "restore after `mod tests;` in the facade",
        T10,
        "src-tauri/src/phone/mailbox.rs",
    );
    Fixture::base()
        .append(
            "clocks.rs",
            "\nfn r(c: &mut CoordinatorClocks) { c.mark_auto_closed(&fqn, t); }\n",
        )
        .rejects("method restore in a production module", T10, "clocks.rs::r");
    Fixture::base()
        .edit(
            "clocks.rs",
            "CoordinatorClocks;",
            "CoordinatorClocks as Alias;",
        )
        .append(
            "clocks.rs",
            "\nfn u(g: &mut Alias) { CoordinatorClocks::mark_auto_closed(&mut g, &alias, t); }\n",
        )
        .rejects("UFCS restore", T10, "clocks.rs::u");
    Fixture::base()
        .append(
            "clocks.rs",
            "\n#[cfg(not(test))]\nfn n(c: &mut X) { c.mark_auto_closed(&f, t); }\n",
        )
        .rejects("restore under cfg(not(test))", T10, "clocks.rs::n");
    Fixture::base()
        .append(
            "clocks.rs",
            "\nfn v(c: &mut CoordinatorClocks) {\n    let restore = \
             CoordinatorClocks::mark_auto_closed;\n    \
             restore(c, fqn, t);\n}\n",
        )
        .rejects("restore taken as a value", T10, "clocks.rs::v");
    Fixture::base()
        .append("clocks.rs", "\npub type Alias = CoordinatorClocks;\n")
        .append(
            "delivery.rs",
            "\nfn pass_on() { helper(Alias::mark_auto_closed); }\n",
        )
        .rejects(
            "alias passed to a helper of another module",
            T10,
            "delivery.rs::pass_on",
        );
    Fixture::base()
        .append(
            "clocks.rs",
            "\n#[cfg(test)]\nfn w() { let r = Alias::mark_auto_closed; r(); }\n",
        )
        .append(
            "clocks.rs",
            "\npub struct Other;\nimpl Other {\n    fn mark_auto_closed(&mut self) {}\n}\n\
             fn h() { let f = Other::mark_auto_closed; }\n",
        )
        .accepts("test-only value and a homonym of another type");
    Fixture::base()
        .append(
            "clocks.rs",
            "\nfn m(c: &mut X) { debug_assert!(c.mark_auto_closed(&f, t)); }\n",
        )
        .rejects("restore inside a macro", T10, "clocks.rs::m");
    Fixture::base()
        .append(
            "clocks.rs",
            "\nfn q() { println!(\"mark_auto_closed(&marker_fqn)\"); }\n",
        )
        .accepts("macro literal naming the mutator");
}

fn argument_controls() {
    let fields = "ContextAlert { member, workgroup, observed, thresholds }";
    Fixture::base()
        .edit(
            DELIVERY,
            fields,
            "ContextAlert { member: workgroup, workgroup: member, observed, thresholds }",
        )
        .rejects("swapped destructuring", "argument 3", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            "                    workgroup,\n",
            "                    group,\n",
        )
        .edit(
            DELIVERY,
            "thresholds } => {\n",
            "thresholds } => {\n                let group = member;\n",
        )
        .rejects("permuted intermediate alias", "argument 3", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            fields,
            "ContextAlert { member: who, workgroup: group, observed, thresholds }",
        )
        .edit(
            DELIVERY,
            "                    workgroup,\n                    member,\n",
            "                    group,\n                    who,\n",
        )
        .accepts("renamed bindings in order");
    Fixture::base()
        .edit(
            DELIVERY,
            "                    observed,\n                    thresholds\n",
            "                    thresholds,\n                    observed\n",
        )
        .rejects("two arguments swapped", "argument 5", DELIVERY);
    Fixture::base()
        .edit(DELIVERY, "                    observed,\n", "")
        .rejects("argument removed", "want 6 arguments", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            "                    thresholds\n",
            "                    thresholds,\n                    kind\n",
        )
        .rejects("seventh argument", "want 6 arguments", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            "project.unwrap_or(\"\")",
            "project.unwrap_or(\"none\")",
        )
        .rejects("fallback replaced", "argument 2", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            "                    thresholds\n                );",
            "                    thresholds,\n                );",
        )
        .accepts("trailing comma");
}

fn format_and_role_controls() {
    let info = concat!(
        "\"[context-alert] delivered coordinatorSession={} project={} \\\n",
        "                    workgroup"
    );
    Fixture::base()
        .edit(
            DELIVERY,
            info,
            concat!(
                "\"[context-alert] delivered coordinatorSession={} project={}  \\\n",
                "                    workgroup"
            ),
        )
        .rejects("one format byte changed", "log format", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            info,
            "\"[context-alert] delivered coordinatorSession={} \\\n  project={} workgroup",
        )
        .accepts("reflowed literal");
    let warn = "log::warn!(\n                    \"[context-alert] coordinator";
    Fixture::base()
        .edit(
            DELIVERY,
            warn,
            "log::error!(\n                    \"[context-alert] coordinator",
        )
        .rejects("warn level changed", "log roles", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            warn,
            "log::warn!(\n                    \"[context-alarm] coordinator",
        )
        .rejects("warn prefix changed", "log roles", DELIVERY);
    Fixture::base()
        .append(
            DELIVERY,
            "\nfn twice() { log::info!(\"[remote-activity] again\"); }\n",
        )
        .rejects("remote info duplicated", "log roles", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            "\"[remote-activity] delivered",
            "\"[context-alert] delivered",
        )
        .rejects("prefixes swapped, total kept", "log roles", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            "Err(error) if err_is_pty_session_missing(&error) => {",
            "Err(error) if retry(&error) => {",
        )
        .rejects("warn outside its error arm", "log roles", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            "log::info!(\n                    \"[remote-activity]",
            "log::debug!(\n                    \"[remote-activity]",
        )
        .rejects("remote level changed", "log roles", DELIVERY);
}

fn delivery_path_controls() {
    let call = "        Self::log_internal_system_notice_delivery(session_id, target, notice);\n";
    Fixture::base()
        .edit(
            DELIVERY,
            call,
            &format!(
                concat!(
                "        if matches!(notice, InternalSystemNotice::RemoteActivity {{ .. }}) {{\n",
                "    {call}        }}\n"
            ),
                call = call
            ),
        )
        .rejects(
            "call only for RemoteActivity",
            "delivery ContextAlert",
            DELIVERY,
        );
    Fixture::base()
        .edit(
            DELIVERY,
            call,
            &format!(
                concat!(
                    "        if matches!(notice, InternalSystemNotice::ContextAlert {{ .. }}) {{\n",
                    "    {call}        }}\n"
                ),
                call = call
            ),
        )
        .rejects(
            "call only for ContextAlert",
            "delivery RemoteActivity",
            DELIVERY,
        );
    Fixture::base().edit(DELIVERY, call, "").rejects(
        "helper orphaned",
        "delivery ContextAlert",
        DELIVERY,
    );
    Fixture::base()
        .edit(DELIVERY, call, &format!("{call}{call}"))
        .rejects("helper called twice", "delivery RemoteActivity", DELIVERY);
    Fixture::base()
        .edit(DELIVERY, call, &format!("        return Ok(());\n{call}"))
        .rejects("success before the call", "delivery ContextAlert", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            "InternalSystemNotice::ContextAlert { member,",
            "InternalSystemNotice::RemoteActivity { member,",
        )
        .edit(
            DELIVERY,
            "InternalSystemNotice::RemoteActivity { kind, .. }",
            "InternalSystemNotice::ContextAlert { kind, .. }",
        )
        .rejects("arm labels swapped", "delivery ContextAlert", DELIVERY);
    let moved = DELIVERY_SRC
        .split("    fn log_internal_system_notice_delivery")
        .nth(1)
        .unwrap();
    let annex = format!(
        "use super::*;\n\nimpl Mailbox {{\n    pub(super) fn log_internal_system_notice_delivery{}",
        moved.trim_end().trim_end_matches('}').trim_end()
    );
    let annex = format!("{annex}\n}}\n");
    let without = DELIVERY_SRC
        .split("\n    fn log_internal_system_notice_delivery")
        .next()
        .unwrap();
    let mut base = Fixture::base();
    *base.get(DELIVERY) = format!("mod logs;\n{without}\n}}\n");
    base.add("delivery/logs.rs", &annex)
        .append(
            "delivery/logs.rs",
            "\n#[cfg(test)]\nfn example() { log::info!(\"[context-alert] delivered x\"); }\n",
        )
        .accepts("log moved to an annex, with a test example left behind");
}

/// The INFO's session, target and notice come from the real call in the entry.
fn provenance_controls() {
    let call = "Self::log_internal_system_notice_delivery(session_id, target, notice);";
    Fixture::base()
        .edit(
            DELIVERY,
            call,
            "let other = Uuid::new_v4();\n        \
             Self::log_internal_system_notice_delivery(other, target, notice);",
        )
        .rejects("another Uuid passed to the helper", "argument 1", DELIVERY);
    Fixture::base()
        .edit(
            DELIVERY,
            call,
            "Self::log_internal_system_notice_delivery(session_id, target, &spare);",
        )
        .rejects(
            "another notice passed to the helper",
            "argument 3",
            DELIVERY,
        );
    Fixture::base()
        .edit(
            DELIVERY,
            "match notice {",
            "let shadow = notice;\n        match spare {",
        )
        .rejects("fields of another notice", "argument 3", DELIVERY);
    let quiet = "\npub struct Quiet;\n\nimpl Quiet {\n    \
        fn log_internal_system_notice_delivery(session_id: Uuid,\n        \
        target: &InternalSystemTarget, notice: &InternalSystemNotice) {}\n}\n";
    Fixture::base()
        .append(DELIVERY, quiet)
        .edit(
            DELIVERY,
            call,
            "Quiet::log_internal_system_notice_delivery(session_id, target, notice);",
        )
        .rejects(
            "qualified call to a homonym",
            "delivery ContextAlert",
            DELIVERY,
        );
    Fixture::base()
        .append(DELIVERY, quiet)
        .accepts("an uncalled homonym helper of another type");
    for (name, value) in [("session_id", "Uuid::nil()"), ("target", "spare_target")] {
        Fixture::base()
            .edit(
                DELIVERY,
                call,
                &format!("let {name} = {value};\n        {call}"),
            )
            .rejects(
                "entry parameter shadowed",
                &format!("rebinds {name}"),
                DELIVERY,
            );
    }
    // The homonym lives outside the mailbox tree, as in a real misrouted call.
    Fixture::base()
        .edit(
            DELIVERY,
            call,
            &call.replace("Self::", "crate::otro::Mailbox::"),
        )
        .rejects(
            "module-qualified homonym",
            "delivery ContextAlert",
            DELIVERY,
        );
    // A name the body rebinds is resolved there, never by its text alone.
    let named = call.replace("Self::", "Mailbox::");
    for (why, bind) in [
        ("body import of a homonym", "use crate::otro::Mailbox;"),
        (
            "body alias of a homonym",
            "type Mailbox = crate::otro::Mailbox;",
        ),
        ("body import of an unknown module", "use otro::Mailbox;"),
    ] {
        Fixture::base()
            .edit(DELIVERY, call, &format!("{bind}\n        {named}"))
            .rejects(why, "unresolved call to Mailbox::", DELIVERY);
    }
    Fixture::base()
        .edit(DELIVERY, call, &named)
        .accepts("owner named by its declaration");
    Fixture::base()
        .edit(
            DELIVERY,
            call,
            &format!("use super::delivery::Mailbox;\n        {named}"),
        )
        .accepts("body import of the real owner");
}

/// Pattern guards and every successful `Ok` form.
fn guard_and_exit_controls() {
    let call = "        Self::log_internal_system_notice_delivery(session_id, target, notice);\n";
    Fixture::base()
        .edit(
            DELIVERY,
            call,
            &format!(
                concat!(
                    "        if matches!(notice, InternalSystemNotice::ContextAlert {{ .. }}\n",
                    "            | InternalSystemNotice::RemoteActivity {{ .. }} if ready()) {{\n",
                    "    {call}        }}\n"
                ),
                call = call
            ),
        )
        .rejects(
            "call behind a guarded matches!",
            "delivery ContextAlert",
            DELIVERY,
        );
    let tail = concat!(
        "        Self::log_internal_system_notice_delivery(session_id, target, notice);\n",
        "        Ok(())"
    );
    Fixture::base()
        .edit(DELIVERY, tail, &tail.replace("Ok(())", "Result::Ok(())"))
        .accepts("qualified Ok tail");
    Fixture::base()
        .edit(
            DELIVERY,
            ".await?;\n",
            concat!(
                ".await?;\n        if early() {\n",
                "            return std::result::Result::Ok(());\n        }\n"
            ),
        )
        .rejects(
            "qualified Ok return before the INFO",
            "delivery ContextAlert",
            DELIVERY,
        );
    Fixture::base()
        .edit(
            DELIVERY,
            ".await?;\n",
            concat!(
                ".await?;\n        match () {\n            () if {\n",
                "                if session_id.is_nil() {\n                    return Ok(());\n",
                "                }\n                true\n",
                "            } => {}\n            _ => {}\n",
                "        }\n"
            ),
        )
        .rejects(
            "Ok returned inside a match guard",
            "delivery ContextAlert",
            DELIVERY,
        );
    Fixture::base()
        .edit(DELIVERY, tail, &tail.replace("Ok(())", "finish()"))
        .rejects(
            "unclassified success value",
            "unresolved exit value",
            DELIVERY,
        );
}

/// Owner names resolve in the reference's own scopes first.
fn scope_controls() {
    let other_alias = "\nmod b {\n    struct Other;\n    type ClockAlias = Other;\n    \
        fn h() { let f = super::super::clocks::ClockAlias::mark_auto_closed; }\n}\n";
    Fixture::base()
        .append("clocks.rs", "\npub type ClockAlias = CoordinatorClocks;\n")
        .append(DELIVERY, other_alias)
        .rejects(
            "module path to an alias",
            T10,
            "delivery.rs::b::h (reference #1",
        );
    for (why, value) in [
        (
            "UFCS module path to an alias",
            "<super::super::clocks::ClockAlias>::mark_auto_closed",
        ),
        (
            "UFCS module path in a macro",
            "vec![<super::super::clocks::ClockAlias>::mark_auto_closed]",
        ),
    ] {
        Fixture::base()
            .append("clocks.rs", "\npub type ClockAlias = CoordinatorClocks;\n")
            .append(
                DELIVERY,
                &other_alias.replace("super::super::clocks::ClockAlias::mark_auto_closed", value),
            )
            .rejects(why, T10, "delivery.rs::b::h (reference #1");
    }
    let alias = "\npub type Alias = CoordinatorClocks;\n";
    let local = "\nmod local {\n    use crate::other::Plain as Alias;\n    \
        fn h() { let f = Alias::mark_auto_closed; }\n}\n";
    Fixture::base()
        .append("clocks.rs", alias)
        .append(DELIVERY, "\npub struct Plain;\n")
        .append(DELIVERY, local)
        .accepts("local rename shadows a CoordinatorClocks alias elsewhere");
    let imported = "\nmod local {\n    use super::super::clocks::Alias;\n    \
        fn h() { let f = Alias::mark_auto_closed; }\n}\n";
    Fixture::base()
        .append("clocks.rs", alias)
        .append(DELIVERY, imported)
        .rejects(
            "imported CoordinatorClocks alias",
            T10,
            "delivery.rs::local::h (reference #1",
        );
    Fixture::base()
        .append("clocks.rs", alias)
        .append(DELIVERY, "\npub type Alias = Plain;\npub struct Plain;\n")
        .append(DELIVERY, "\nfn g() { let f = Alias::mark_auto_closed; }\n")
        .accepts("same-module alias wins over a homonym elsewhere");
    Fixture::base()
        .append("clocks.rs", alias)
        .append(DELIVERY, "\npub type Alias = Plain;\npub struct Plain;\n")
        .append(
            "../mailbox.rs",
            "\nfn g() { let f = Alias::mark_auto_closed; }\n",
        )
        .rejects(
            "homonyms disagree for an import",
            "ambiguous owner",
            "src-tauri/src/phone/mailbox.rs",
        );
}

// ------------------------------------------------------------------ topology

#[test]
fn mailbox_sources_cover_moves() {
    let tree = discover(Fixture::base().write().path(), FACADE).expect("base tree");
    let roles: Vec<(String, sources::Role)> = tree
        .sources
        .iter()
        .map(|s| (s.path.clone(), s.role))
        .collect();
    let role = |file: &str| {
        roles
            .iter()
            .find(|(p, _)| p.ends_with(file))
            .map(|(_, r)| *r)
    };
    assert_eq!(role("phone/mailbox.rs"), Some(sources::Role::Prod));
    assert_eq!(role("mailbox/delivery.rs"), Some(sources::Role::Prod));
    assert_eq!(role("mailbox/tests.rs"), Some(sources::Role::Test));
    assert_eq!(role("tests/nested.rs"), Some(sources::Role::Test));
    let reads = tree
        .events
        .iter()
        .filter(|e| e.starts_with("MB_READ\t"))
        .count();
    assert_eq!(reads, roles.len(), "one read per physical source");

    let call = "\nfn late() { debug_assert!(Alias::mark_auto_closed(&mut g, &f, t)); }\n";
    let nested = Fixture::base()
        .edit(
            DELIVERY,
            "use crate::config",
            "mod annex;\nuse crate::config",
        )
        .add("delivery/annex.rs", "mod deeper;\n")
        .add("delivery/annex/deeper.rs", "pub fn quiet() {}\n");
    let flat_to_mod = |f: Fixture| f.rename("delivery/annex.rs", "delivery/annex/mod.rs");
    let with_call = |f: Fixture| {
        f.append("delivery/annex/deeper.rs", call)
            .append("clocks.rs", "\npub type Alias = CoordinatorClocks;\n")
    };
    Fixture {
        files: nested.files.clone(),
    }
    .accepts("new nested production annex");
    flat_to_mod(Fixture {
        files: nested.files.clone(),
    })
    .accepts("annex moved to mod.rs");
    with_call(Fixture {
        files: nested.files.clone(),
    })
    .rejects("restore in the new annex", T10, "delivery/annex/deeper.rs");
    with_call(flat_to_mod(Fixture {
        files: nested.files.clone(),
    }))
    .rejects("restore after the move", T10, "delivery/annex/deeper.rs");
    Fixture::base()
        .edit(
            "../mailbox.rs",
            "mod clocks;",
            "mod clocks;\nmod inline {\n    mod leaf;\n}",
        )
        .add(
            "inline/leaf.rs",
            "fn l(c: &mut X) { c.mark_auto_closed(&f, t); }\n",
        )
        .rejects("restore under an inline module", T10, "inline/leaf.rs");
    Fixture::base()
        .edit(
            "../mailbox.rs",
            "mod clocks;",
            "mod clocks;\nmod inline {\n    #[path = \"custom.rs\"]\n    mod leaf;\n}",
        )
        .add(
            "inline/custom.rs",
            "fn l(c: &mut X) { c.mark_auto_closed(&f, t); }\n",
        )
        .rejects("#[path] inside an inline module", T10, "inline/custom.rs");
    Fixture::base()
        .edit(
            "../mailbox.rs",
            "mod clocks;",
            "mod clocks;\nmod inline {\n    #[path = \"custom.rs\"]\n    mod leaf;\n}",
        )
        .add("inline/custom.rs", "pub fn l() {}\n")
        .accepts("valid #[path] inside an inline module");

    discovery_failures();
}

fn discovery_failure(fixture: Fixture, why: &str, cause: &str) {
    match fixture.run() {
        Ok(report) => panic!("{why}: accepted with {report:?}"),
        Err(errors) => assert!(
            errors
                .iter()
                .any(|e| e.starts_with("discovery: ") && e.contains(cause)),
            "{why}: want discovery {cause:?}, got:\n{}",
            errors.join("\n")
        ),
    }
}

fn discovery_failures() {
    let empty = tempfile::tempdir().expect("temp dir");
    let missing = discover(empty.path(), FACADE).err().expect("missing root");
    assert!(missing.contains("unreadable"), "{missing}");
    let only_facade = Fixture {
        files: vec![("../mailbox.rs".into(), "\n".into())],
    };
    let dir = only_facade.write();
    let err = discover(dir.path(), FACADE).err().expect("empty tree");
    assert!(err.contains("empty module tree"), "{err}");
    discovery_failure(
        Fixture::base().add("stray.rs", "fn s() {}\n"),
        "orphan",
        "orphan source",
    );
    discovery_failure(
        Fixture::base().remove("clocks.rs"),
        "missing module",
        "missing source",
    );
    discovery_failure(
        Fixture::base().add("clocks/mod.rs", "\n"),
        "ambiguous module",
        "ambiguous",
    );
    discovery_failure(
        Fixture::base().append("clocks.rs", "\nfn broken( {\n"),
        "malformed source",
        "malformed source",
    );
    discovery_failure(
        Fixture::base().append("clocks.rs", "\ninclude!(\"other.rs\");\n"),
        "include! inclusion",
        "include!",
    );
}
