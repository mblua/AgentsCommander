use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Mutex;

use super::*;

const TOKEN: &str = "1a2b3c4d";
const GLOBAL: &str = "agentscommander:screenshot-1a2b3c4d";
const DESCRIPTION: &str = "AgentsCommander screenshot 1a2b3c4d";
const ORIGINAL: &str = "-- user binds\nhl.bind(\"SUPER + A\", hl.dsp.exec(\"a\"))\n";
const NO_ERRORS: &str = "[\"\"]";

fn ids() -> BindIds<'static> {
    BindIds {
        token: TOKEN,
        global: GLOBAL,
        description: DESCRIPTION,
    }
}

type Reply = Result<String, String>;

#[derive(Default)]
struct FakeHyprctl {
    reload: Mutex<VecDeque<Reply>>,
    configerrors: Mutex<VecDeque<Reply>>,
    binds: Mutex<VecDeque<Reply>>,
    on_reload: Option<Box<dyn Fn() + Send + Sync>>,
    calls: Mutex<Vec<&'static str>>,
}

fn ok(s: &str) -> Reply {
    Ok(s.to_string())
}

impl FakeHyprctl {
    fn errors(self, replies: Vec<Reply>) -> Self {
        *self.configerrors.lock().unwrap() = replies.into();
        self
    }

    fn binds(self, replies: Vec<Reply>) -> Self {
        *self.binds.lock().unwrap() = replies.into();
        self
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }
}

impl Hyprctl for FakeHyprctl {
    async fn run(&self, args: &'static [&'static str]) -> Result<String, String> {
        self.calls.lock().unwrap().push(args[0]);
        let (queue, default) = match args[0] {
            "reload" => {
                if let Some(edit) = &self.on_reload {
                    edit();
                }
                (&self.reload, "ok")
            }
            "configerrors" => (&self.configerrors, NO_ERRORS),
            "binds" => (&self.binds, "[]"),
            other => panic!("unexpected hyprctl {other}"),
        };
        let next = queue.lock().unwrap().pop_front();
        next.unwrap_or_else(|| ok(default))
    }
}

fn bound_json(key: &str) -> Reply {
    Ok(format!(
        "[{{\"modmask\":4,\"key\":\"{key}\",\"description\":\"{DESCRIPTION}\",\
         \"dispatcher\":\"__lua\"}}]"
    ))
}

/// `<home>/hypr/{hyprland.lua, bindings.lua}`; returns (dir, home, bindings).
fn omarchy(main: Option<&str>, bindings: &str) -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let hypr = home.join("hypr");
    std::fs::create_dir(&hypr).unwrap();
    if let Some(main) = main {
        std::fs::write(hypr.join("hyprland.lua"), main).unwrap();
    }
    let path = hypr.join("bindings.lua");
    std::fs::write(&path, bindings).unwrap();
    (dir, home, path)
}

const MAIN: &str = "require(\"hypr.bindings\")\n";

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn with_block(key: char) -> String {
    splice(ORIGINAL, TOKEN, Some(&block(key, &ids()))).unwrap()
}

async fn run(ctl: &FakeHyprctl, home: &Path, enabled: bool) -> BindOutcome {
    apply(ctl, true, Some(home), enabled, 'Q', &ids()).await
}

#[test]
fn block_and_lines_exact_text() {
    let lua = "hl.bind(\"CTRL + Q\", hl.dsp.global(\"agentscommander:screenshot-1a2b3c4d\"), \
               { description = \"AgentsCommander screenshot 1a2b3c4d\" })";
    assert_eq!(lua_line('q', &ids()), lua);
    assert_eq!(
        conf_line('Q', &ids()),
        "bindd = CTRL, Q, AgentsCommander screenshot 1a2b3c4d, global, \
         agentscommander:screenshot-1a2b3c4d"
    );
    let expected = format!(
        "-- >>> AgentsCommander screenshot shortcut 1a2b3c4d (managed by AgentsCommander) >>>\n\
         {lua}\n-- <<< AgentsCommander screenshot shortcut 1a2b3c4d <<<\n"
    );
    assert_eq!(block('Q', &ids()), expected);
    let source = include_str!("../hyprland_bind.rs");
    assert!(source.lines().all(|l| l.chars().count() <= 100));
}

#[test]
fn splice_append_replace_remove() {
    let b = block('Q', &ids());
    assert_eq!(splice("", TOKEN, Some(&b)).unwrap(), b);
    assert_eq!(splice("x", TOKEN, Some(&b)).unwrap(), format!("x\n\n{b}"));
    assert_eq!(splice("x\n", TOKEN, Some(&b)).unwrap(), format!("x\n\n{b}"));
    let w = block('W', &ids());
    let replaced = splice(&with_block('Q'), TOKEN, Some(&w)).unwrap();
    assert_eq!(replaced, format!("{ORIGINAL}\n{w}"));
    for c in ["", "x\n", ORIGINAL] {
        let added = splice(c, TOKEN, Some(&b)).unwrap();
        assert_eq!(splice(&added, TOKEN, None).unwrap(), c);
    }
    assert_eq!(splice(ORIGINAL, TOKEN, None).unwrap(), ORIGINAL);
}

#[test]
fn splice_rejects_damaged_markers() {
    let (begin, end) = (begin_marker(TOKEN), end_marker(TOKEN));
    for damaged in [
        format!("{begin}\n{begin}\n{end}\n"),
        format!("{end}\n{begin}\n"),
        format!("{begin}\n"),
    ] {
        let err = splice(&damaged, TOKEN, None).unwrap_err();
        assert!(err.contains("damaged"), "{err}");
    }
}

#[test]
fn splice_leaves_other_instances_alone() {
    let other_ids = BindIds {
        token: "bbbb2222",
        global: "agentscommander:screenshot-bbbb2222",
        description: "AgentsCommander screenshot bbbb2222",
    };
    let other = block('Q', &other_ids);
    let mine = BindIds {
        token: "aaaa1111",
        ..ids()
    };
    let start = format!("{ORIGINAL}\n{other}");
    let added = splice(&start, "aaaa1111", Some(&block('Q', &mine))).unwrap();
    assert!(added.starts_with(&start), "{added}");
    assert_eq!(splice(&added, "aaaa1111", None).unwrap(), start);
}

#[test]
fn parse_config_errors_cases() {
    assert_eq!(parse_config_errors("[\"\"]").unwrap(), Vec::<String>::new());
    assert_eq!(
        parse_config_errors("[\"a\",\"  \",\"b\"]").unwrap(),
        ["a", "b"]
    );
    assert!(parse_config_errors("oops").is_err());
}

#[test]
fn parse_bound_cases() {
    let row = |modmask: u64, key: &str, desc: &str| {
        format!("[{{\"modmask\":{modmask},\"key\":\"{key}\",\"description\":\"{desc}\"}}]")
    };
    assert!(parse_bound(&row(4, "Q", DESCRIPTION), 'Q', DESCRIPTION).unwrap());
    assert!(!parse_bound(&row(4, "Q", "other"), 'Q', DESCRIPTION).unwrap());
    assert!(!parse_bound(&row(5, "Q", DESCRIPTION), 'Q', DESCRIPTION).unwrap());
    assert!(!parse_bound(&row(4, "W", DESCRIPTION), 'Q', DESCRIPTION).unwrap());
    assert!(parse_bound(&row(4, "q", DESCRIPTION), 'Q', DESCRIPTION).unwrap());
    assert!(parse_bound("{", 'Q', DESCRIPTION).is_err());
}

#[tokio::test]
async fn not_hyprland_runs_nothing() {
    let ctl = FakeHyprctl::default();
    let out = apply(&ctl, false, None, true, 'Q', &ids()).await;
    assert_eq!(out, BindOutcome::NotHyprland);
    assert!(ctl.calls().is_empty());
}

#[tokio::test]
async fn on_and_bound_applies() {
    let (_d, home, path) = omarchy(Some(MAIN), ORIGINAL);
    let ctl = FakeHyprctl::default().binds(vec![bound_json("Q")]);
    assert_eq!(run(&ctl, &home, true).await, BindOutcome::Applied);
    assert_eq!(read(&path), with_block('Q'));
}

fn failed(out: BindOutcome) -> String {
    match out {
        BindOutcome::Failed(m) => m,
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn new_config_error_restores() {
    let (_d, home, path) = omarchy(Some(MAIN), ORIGINAL);
    let ctl = FakeHyprctl::default().errors(vec![ok(NO_ERRORS), ok("[\"line 9: bad\"]")]);
    let m = failed(run(&ctl, &home, true).await);
    assert!(m.contains("restored") && m.contains("line 9: bad"), "{m}");
    assert_eq!(read(&path), ORIGINAL);
}

#[tokio::test]
async fn only_old_errors_keep_the_block() {
    let (_d, home, path) = omarchy(Some(MAIN), ORIGINAL);
    let old = ok("[\"old\"]");
    let ctl = FakeHyprctl::default()
        .errors(vec![old.clone(), old])
        .binds(vec![bound_json("Q")]);
    assert_eq!(run(&ctl, &home, true).await, BindOutcome::Applied);
    assert_eq!(read(&path), with_block('Q'));
}

#[tokio::test]
async fn after_errors_failure_restores() {
    let (_d, home, path) = omarchy(Some(MAIN), ORIGINAL);
    let ctl = FakeHyprctl::default().errors(vec![ok(NO_ERRORS), Err("gone".into())]);
    let m = failed(run(&ctl, &home, true).await);
    assert!(m.contains("gone") && m.contains("restored"), "{m}");
    assert_eq!(read(&path), ORIGINAL);
}

#[tokio::test]
async fn reload_failure_still_checks_errors() {
    let (_d, home, path) = omarchy(Some(MAIN), ORIGINAL);
    let ctl = FakeHyprctl::default().binds(vec![bound_json("Q")]);
    *ctl.reload.lock().unwrap() = vec![Err("no socket".to_string())].into();
    assert_eq!(run(&ctl, &home, true).await, BindOutcome::Applied);
    assert_eq!(
        ctl.calls(),
        ["configerrors", "reload", "configerrors", "binds"]
    );
    assert_eq!(read(&path), with_block('Q'));
}

#[tokio::test]
async fn external_edit_during_reload_is_kept() {
    let (_d, home, path) = omarchy(Some(MAIN), ORIGINAL);
    let edited = path.clone();
    let ctl = FakeHyprctl {
        on_reload: Some(Box::new(move || {
            std::fs::write(&edited, "external\n").unwrap()
        })),
        ..FakeHyprctl::default()
    }
    .errors(vec![ok(NO_ERRORS), ok("[\"new\"]")]);
    let m = failed(run(&ctl, &home, true).await);
    assert!(m.contains("restoring bindings.lua failed"), "{m}");
    assert_eq!(read(&path), "external\n");
}

#[tokio::test]
async fn written_but_not_loaded_is_undone() {
    let (_d, home, path) = omarchy(Some(MAIN), ORIGINAL);
    let ctl = FakeHyprctl::default();
    let m = failed(run(&ctl, &home, true).await);
    assert!(m.contains("did not load"), "{m}");
    assert_eq!(read(&path), ORIGINAL);
}

#[tokio::test]
async fn block_present_but_not_loaded() {
    let (_d, home, path) = omarchy(Some(MAIN), &with_block('Q'));
    let ctl = FakeHyprctl::default();
    assert_eq!(run(&ctl, &home, true).await, BindOutcome::NotLoaded);
    assert_eq!(read(&path), with_block('Q'));
    assert_eq!(ctl.calls(), ["binds"]);
}

#[tokio::test]
async fn off_removes_the_block() {
    let (_d, home, path) = omarchy(Some(MAIN), &with_block('Q'));
    let ctl = FakeHyprctl::default();
    let out = run(&ctl, &home, false).await;
    assert_eq!(out, BindOutcome::Manual { reason: None });
    assert_eq!(read(&path), ORIGINAL);
}

#[tokio::test]
async fn off_with_user_line_is_user_bound() {
    let (_d, home, path) = omarchy(Some(MAIN), ORIGINAL);
    let ctl = FakeHyprctl::default().binds(vec![bound_json("Q")]);
    assert_eq!(run(&ctl, &home, false).await, BindOutcome::UserBound);
    assert_eq!(read(&path), ORIGINAL);
}

#[tokio::test]
async fn other_layouts_are_manual_or_user_bound() {
    let (_d, home, path) = omarchy(None, ORIGINAL);
    let ctl = FakeHyprctl::default();
    let BindOutcome::Manual { reason: Some(r) } = run(&ctl, &home, true).await else {
        panic!("expected Manual with a reason")
    };
    assert!(r.contains("Omarchy"), "{r}");
    assert_eq!(read(&path), ORIGINAL);
    let ctl = FakeHyprctl::default().binds(vec![bound_json("Q")]);
    assert_eq!(run(&ctl, &home, true).await, BindOutcome::UserBound);
    let (_d, home, path) = omarchy(Some("-- require(\"hypr.bindings\")\n"), ORIGINAL);
    assert!(omarchy_bindings_path(&home).is_none());
    let out = run(&FakeHyprctl::default(), &home, true).await;
    assert!(
        matches!(out, BindOutcome::Manual { reason: Some(_) }),
        "{out:?}"
    );
    assert_eq!(read(&path), ORIGINAL);
    let out = apply(&FakeHyprctl::default(), true, None, true, 'Q', &ids()).await;
    let reason = Some("HOME is not set".to_string());
    assert_eq!(out, BindOutcome::Manual { reason });
}

#[tokio::test]
async fn binds_failure_is_reported() {
    let (_d, home, _path) = omarchy(Some(MAIN), &with_block('Q'));
    let ctl = FakeHyprctl::default().binds(vec![Err("boom".into())]);
    let m = failed(run(&ctl, &home, true).await);
    assert!(
        m.contains("could not read the Hyprland key bindings"),
        "{m}"
    );
}

#[test]
fn status_texts() {
    let lua = lua_line('Q', &ids());
    let status = |o: BindOutcome| status_result(&o, 'Q', &ids());
    for o in [
        BindOutcome::NotHyprland,
        BindOutcome::Applied,
        BindOutcome::UserBound,
    ] {
        assert_eq!(status(o), Ok(()));
    }
    let not_loaded = status(BindOutcome::NotLoaded).unwrap_err();
    assert!(not_loaded.contains("configerrors") && !not_loaded.contains("hl.bind("));
    let manual = status(BindOutcome::Manual { reason: None }).unwrap_err();
    assert!(
        manual.contains(&lua) && manual.contains("Settings > Hotkeys"),
        "{manual}"
    );
    assert!(manual.contains(&conf_line('Q', &ids())), "{manual}");
    let reason = Some("why".to_string());
    let manual = status(BindOutcome::Manual { reason }).unwrap_err();
    assert!(manual.contains("why") && manual.contains(&lua), "{manual}");
    assert!(!manual.contains("Settings > Hotkeys"), "{manual}");
    let failed = status(BindOutcome::Failed("Broke.".into())).unwrap_err();
    assert_eq!(
        failed,
        format!("Broke. Add this line to your Hyprland config instead: {lua}")
    );
}
