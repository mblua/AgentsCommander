use super::*;

/// The files rustc compiles for `module`, and for every module below it when
/// `reach` says so, resolved by walking `mod` declarations down from the crate
/// root (see `support/module_sources.rs`).
pub(crate) fn files_of(module: &[&str], reach: Reach) -> Result<Vec<PathBuf>, String> {
    match reach {
        Reach::OwnFilesOnly => own_files_of(module),
        Reach::WithSubmodules => sources_of(module),
    }
}

/// What one module's files were observed to name.
///
/// `anchored`, `relative_up` and `own_module` are `(file, child)` pairs under
/// `crate::`, under `super::` and under `self::` respectively; `aliases` is the
/// files that rename a module group; `forbidden` is the files that name
/// `root_agent` under no anchor at all; `globs` is the number of
/// `super::`-anchored glob imports across the module, counted before the
/// observed sets are deduplicated.
pub(crate) struct Observation {
    pub(crate) anchored: Vec<(String, String)>,
    pub(crate) relative_up: Vec<(String, String)>,
    pub(crate) own_module: Vec<(String, String)>,
    pub(crate) aliases: Vec<String>,
    pub(crate) forbidden: Vec<String>,
    pub(crate) globs: usize,
}

/// Read every file of `module` and report what it names.
///
/// A file reached through the module tree is a file rustc compiles, so a `scrub`
/// failure on one of them is fatal here and says so: it is source the compiler
/// reads and this scan could not.
pub(crate) fn observe(module: &[&str], reach: Reach) -> Observation {
    let files = files_of(module, reach).unwrap_or_else(|reason| {
        panic!(
            "the module {module:?} could not be resolved from the module tree, so this scan \
             proves nothing: {reason}\n\
             \n\
             WHY THIS IS A FAILURE AND NOT A SKIP: this guard exists to prove that a specific \
             dependency is absent. If the module cannot be located, the guard has read nothing \
             and must say so rather than pass. Rename or move the module and this message names \
             the file whose `mod` declaration no longer resolves; update GUARDED_MODULE or \
             HOST_MODULE, or the declaration, to match."
        )
    });
    assert!(
        !files.is_empty(),
        "the module {module:?} resolved to no files at all; the scan proves nothing"
    );

    let mut anchored = Vec::new();
    let mut relative_up = Vec::new();
    let mut own_module = Vec::new();
    let mut aliases = Vec::new();
    let mut forbidden = Vec::new();
    let mut globs = 0usize;
    for path in &files {
        let relative = relative_of(path);
        let code = scrubbed(path, Literals::Drop).unwrap_or_else(|reason| {
            panic!(
                "{reason}\n\
                 \n\
                 This file is in the module tree, so rustc compiles it and this scan could not \
                 read it. That is a hard failure, not a skip."
            )
        });
        let body = normalized(&code);
        let name = |children: Result<Vec<String>, &'static str>| {
            children
                .unwrap_or_else(|reason| panic!("{relative}: {reason}"))
                .into_iter()
                .map(|child| (relative.clone(), child))
                .collect::<Vec<_>>()
        };
        anchored.extend(name(children_under(&body, CRATE_ANCHOR)));
        own_module.extend(name(children_under(&body, SELF_ANCHOR)));
        // Counted here, before the dedup below, because two globs in one file
        // deduplicate to one pair and the count is the only thing that sees the
        // difference. See GLOB_CHILD and entries 9, 14 and 15.
        let reached_up = name(children_under(&body, SUPER_ANCHOR));
        globs += reached_up
            .iter()
            .filter(|(_, child)| child.as_str() == GLOB_CHILD)
            .count();
        relative_up.extend(reached_up);
        if aliases_a_module_group(&body) {
            aliases.push(relative.clone());
        }
        if names_the_replaced_module(&body) {
            forbidden.push(relative.clone());
        }
    }
    anchored.sort();
    anchored.dedup();
    relative_up.sort();
    relative_up.dedup();
    own_module.sort();
    own_module.dedup();
    Observation {
        anchored,
        relative_up,
        own_module,
        aliases,
        forbidden,
        globs,
    }
}

pub(crate) fn expected(table: &[(&str, &str)]) -> Vec<(String, String)> {
    table
        .iter()
        .map(|(file, child)| ((*file).to_string(), (*child).to_string()))
        .collect()
}
