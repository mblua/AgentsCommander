use super::*;

fn source_range(source: &str, needle: &str, index: usize) -> Option<std::ops::Range<usize>> {
    let compact = normalized(source);
    let at = compact.match_indices(needle).nth(index)?.0;
    let offsets: Vec<usize> = source
        .char_indices()
        .filter(|(_, c)| !c.is_whitespace())
        .flat_map(|(i, c)| i..i + c.len_utf8())
        .collect();
    Some(offsets[at]..offsets[at + needle.len() - 1] + 1)
}

fn replace_source(sources: &mut SessionSources, from: &str, to: &str) {
    let hits: usize = sources
        .values()
        .map(|s| normalized(s).matches(from).count())
        .sum();
    assert_eq!(hits, 1, "unique mutation anchor: {from}");
    for source in sources.values_mut() {
        if let Some(range) = source_range(source, from, 0) {
            source.replace_range(range, to);
        }
    }
}

fn move_before(sources: &mut SessionSources, statement: &str, anchor: &str) {
    replace_source(sources, statement, "");
    replace_source(sources, anchor, &format!("{statement}{anchor}"));
}

fn split_source_map(original: &SessionSources) -> SessionSources {
    if original.contains_key(CORE_SOURCE) {
        return original.clone();
    }
    let mut map = original.clone();
    let session = map.get_mut(SESSION_SOURCE).unwrap();
    let start = session.find("async fn create_session_inner_core").unwrap();
    let end = block_end(session, start).unwrap();
    let whole = session[start..end].to_string();
    session.replace_range(start..end, "");
    let context = whole
        .find("        // #930 - resolve the host-credential")
        .unwrap();
    let backend = whole
        .find("        // Capture the effective arg vector BEFORE spawn")
        .unwrap();
    let publication = whole
        .find(concat!(
            "        if let Some(continuation) = owned_replacement.as_ref() {\n",
            "            let actual = mgr.get_pending_session"
        ))
        .unwrap();
    let finish = whole
        .find("\n    };\n    // This accepted core owns every launched effect")
        .unwrap();
    let core = format!(
        "{}context::prepare(&ctx, input).await{}",
        &whole[..context],
        &whole[finish..]
    );
    map.insert(CORE_SOURCE.into(), core);
    for (path, signature, body, next) in [
        (
            CONTEXT_SOURCE,
            "prepare",
            &whole[context..backend],
            "backend::spawn(ctx, input).await",
        ),
        (
            BACKEND_SOURCE,
            "spawn",
            &whole[backend..publication],
            "publication::complete(ctx, input).await",
        ),
        (
            PUBLICATION_SOURCE,
            "complete",
            &whole[publication..finish],
            "",
        ),
    ] {
        map.insert(
            path.into(),
            format!("async fn {signature}() {{\n{body}\n{next}\n}}"),
        );
    }
    map
}

fn rejected(mode: &str, name: &str, passes: bool) {
    assert!(!passes, "{mode}: {name}");
    println!("{mode}: {name}: rejected");
}

#[test]
fn source_guards_round4_controls() {
    let real = session_sources();
    let split = split_source_map(&real);
    for (mode, original) in [("original", real), ("split", split)] {
        assert!(all_source_predicates(&original), "positive {mode}");
        let probes: &[(&str, &str, &str, SourcePredicate)] = &[
            ("remove fence", FENCE, "", source_fence),
            ("remove mark", MARK, "letother_mark=", source_mark),
            (
                "rotation conjunction",
                ROTATION_GATE,
                "ifstart_fresh{",
                source_rotation,
            ),
            (
                "second fresh binding",
                FRESH,
                "letstart_fresh=skip_auto_resume;letstart_fresh=true;",
                source_fresh,
            ),
            ("fresh RHS", FRESH, "letstart_fresh=true;", source_fresh),
            (
                "remove note",
                "note_coordinator_create_on_app(app,cwd)",
                "None",
                source_budgets,
            ),
            (
                "extra rotation reference",
                FRESH,
                "letstart_fresh=skip_auto_resume;/*rotate_origin_memory_at_spawn*/",
                source_budgets,
            ),
            (
                "remove restart refusal",
                RESTART_REFUSAL,
                "",
                source_restart,
            ),
        ];
        for (name, from, to, predicate) in probes {
            let mut probe = original.clone();
            replace_source(&mut probe, from, to);
            rejected(mode, name, predicate(&probe));
        }
        for index in 0..2 {
            let mut probe = original.clone();
            let source = probe
                .values_mut()
                .find(|s| normalized(s).contains(ARCHIVE_GATE))
                .unwrap();
            let range = source_range(source, ARCHIVE_GATE, index).unwrap();
            source.replace_range(range, "");
            rejected(
                mode,
                &format!("remove activation gate {index}"),
                source_archive(&probe),
            );
        }
        for (name, statement, anchor, predicate) in [
            (
                "late fence",
                FENCE,
                UNFENCE,
                source_fence as SourcePredicate,
            ),
            ("early mark drop", DROP_MARK, SPAWN, source_mark),
            (
                "late restart refusal",
                RESTART_REFUSAL,
                "teardown_old_for_restart(transaction,uuid,old_was_dormant).await",
                source_restart,
            ),
        ] {
            let mut probe = original.clone();
            move_before(&mut probe, statement, anchor);
            rejected(mode, name, predicate(&probe));
        }
        let mut probe = original.clone();
        move_before(&mut probe, DROP_MARK, "ifletSome(effect)=backend_effect{");
        rejected(mode, "late effect completion", source_effect(&probe));
        let mut probe = original.clone();
        let path = if probe.contains_key(CORE_SOURCE) {
            CORE_SOURCE
        } else {
            SESSION_SOURCE
        };
        probe
            .get_mut(path)
            .unwrap()
            .push_str("\nasync fn create_session_inner_core() {}\n");
        rejected(mode, "duplicate entry", source_map_complete(&probe));
        if original.contains_key(CORE_SOURCE) {
            for path in [
                CORE_SOURCE,
                CREATION_SOURCE,
                CONTEXT_SOURCE,
                BACKEND_SOURCE,
                PUBLICATION_SOURCE,
            ] {
                let mut probe = original.clone();
                probe.remove(path);
                rejected(
                    mode,
                    &format!("missing {path}"),
                    source_map_complete(&probe),
                );
            }
            for call in [
                "context::prepare(",
                "backend::spawn(",
                "publication::complete(",
            ] {
                for replacement in ["", "other::redirect("] {
                    let mut probe = original.clone();
                    replace_source(&mut probe, call, replacement);
                    assert!(!source_chain(&probe), "{mode}: remove/redirect {call}");
                    println!("{mode}: {call} -> {replacement}: rejected");
                }
            }
            for &(_, call, _, _) in LINKS {
                for replacement in ["", "other::redirect("] {
                    let mut probe = original.clone();
                    replace_source(&mut probe, call, replacement);
                    rejected(
                        mode,
                        &format!("helper {call} -> {replacement}"),
                        source_map_complete(&probe),
                    );
                }
            }
            let mut probe = original.clone();
            let core = probe.get_mut(CORE_SOURCE).unwrap();
            let pending = source_range(core, PENDING, 0).unwrap().start;
            let tail = ".map_err(|error|error.to_string());";
            let range = source_range(&core[pending..], tail, 0).unwrap();
            let end = pending + range.end;
            let statement = core[pending..end].to_string();
            core.replace_range(pending..end, "");
            let call = "context::prepare(";
            let after = core.find(call).unwrap() + call.len();
            core.insert_str(after, &statement);
            rejected(mode, "late pending insertion", source_pending(&probe));
        }
    }
}
