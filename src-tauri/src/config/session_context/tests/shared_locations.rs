use super::*;

/// #1795 `T4`. Pins the pure three-check resolver of plan 4.2 against the
/// authoritative four-check `wg_replica_layout_from_agent_dir` on a layout where
/// the authority check can succeed, so reusing only the three pure name checks
/// cannot drift into a different answer than the validator gives.
#[test]
fn replica_shared_roots_matches_wg_replica_layout_on_disk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ac_root, room_root, replica_root, _matrix_root) = make_valid_replica(temp.path());
    let agent_root = display_path(&replica_root);

    let (resolved_ac_root, resolved_room_root) = replica_shared_roots(&agent_root)
        .expect("an authoritative room replica must resolve its shared roots");
    let layout = crate::config::ac_root::wg_replica_layout_from_agent_dir(&replica_root)
        .expect("the authoritative validator must not error")
        .expect("the authoritative validator must accept this layout");

    assert_eq!(
        canonical_or_original(&resolved_ac_root),
        canonical_or_original(&layout.ac_root),
        "the resolver's `.ac` root must equal the authoritative validator's"
    );
    assert_eq!(
        canonical_or_original(&resolved_room_root),
        canonical_or_original(&layout.wg_dir),
        "the resolver's room root must equal the authoritative validator's"
    );
    assert_eq!(
        canonical_or_original(&resolved_ac_root),
        canonical_or_original(&ac_root),
        "the resolver's `.ac` root must be the fixture's `.ac` root"
    );
    assert_eq!(
        canonical_or_original(&resolved_room_root),
        canonical_or_original(&room_root),
        "the resolver's room root must be the fixture's room root"
    );

    for (agent_root, why) in [
        (
            "C:/fake/.ac/room-7-dev-team/notanagent",
            "check 1: the leaf is not an `__agent_` replica",
        ),
        (
            "C:/fake/.ac/room-7-dev-team/__agent_dev/sub/nested",
            "check 1: the leaf is `nested`, not an `__agent_` replica",
        ),
        (
            "C:/fake/.ac/room-7-dev-team/sub/__agent_x",
            "check 2: the direct parent `sub` carries no room prefix",
        ),
        (
            "C:/fake/room-7-dev-team/__agent_dev",
            "check 3: the room's parent is not named `.ac`",
        ),
    ] {
        assert!(
            replica_shared_roots(agent_root).is_none(),
            "{agent_root} must not resolve ({why})"
        );
    }
}

/// #1795 `T8`. An integration test through the real bootstrap call chain
/// (`create_default_context_templates` ->
/// `ensure_project_context_templates_with_clock`), not a direct call to
/// `create_project_shared_dirs`: deleting the production call site must not
/// leave this green.
///
/// The `project-shared` assertion is written FIRST on purpose. Control `C4`
/// deletes the whole creation call, so all four assertions would fail together
/// and the panic lands on whichever is written first; 8.3 names `project-shared`
/// as `C4`'s kill site.
#[test]
fn bootstrap_creates_project_shared_dirs() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_root = temp.path().join(".ac");

    create_default_context_templates(&ac_root).expect("bootstrap the project `.ac` root");

    assert!(
        ac_root.join("project-shared").is_dir(),
        "bootstrap must create `project-shared` under the project `.ac` root"
    );
    assert!(
        ac_root.join("plans").is_dir(),
        "bootstrap must create `plans` under the project `.ac` root"
    );
    assert!(
        ac_root.join("tools").is_dir(),
        "bootstrap must create `tools` under the project `.ac` root"
    );
    assert!(
        ac_root.join("errors").is_dir(),
        "bootstrap must create `errors` under the project `.ac` root"
    );
}

/// #1795 `T6b`. The backstop creates for an authoritative replica and nothing
/// for a Root Agent or a direct-Matrix agent.
#[test]
fn ensure_replica_shared_locations_creates_for_replica_and_nothing_for_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ac_root, room_root, replica_root, _matrix_root) = make_valid_replica(temp.path());
    let replica = display_path(&replica_root);

    ensure_replica_shared_locations(&replica, &replica);

    for name in ["plans", "tools", "errors", "project-shared"] {
        assert!(
            ac_root.join(name).is_dir(),
            "the backstop must create `{name}` under the project `.ac` root"
        );
    }
    assert!(
        room_root.join("room-shared").is_dir(),
        "the backstop must create `room-shared` under the room root"
    );

    let root_agent = temp.path().join("root-side").join("ac-root-agent");
    std::fs::create_dir_all(&root_agent).expect("root agent dir");
    let root_agent = display_path(&root_agent);
    ensure_replica_shared_locations(&root_agent, &root_agent);

    let direct_matrix = temp.path().join("matrix-side").join("_agent_dev-rust");
    std::fs::create_dir_all(&direct_matrix).expect("direct matrix dir");
    let direct_matrix = display_path(&direct_matrix);
    ensure_replica_shared_locations(&direct_matrix, &direct_matrix);

    for parent in ["root-side", "matrix-side"] {
        let side = temp.path().join(parent);
        assert!(
            !side.join("room-shared").exists(),
            "nothing may be created beside a {parent} agent"
        );
        for name in ["plans", "tools", "errors", "project-shared"] {
            assert!(
                !side.join(name).exists(),
                "no `{name}` may be created beside a {parent} agent"
            );
        }
    }
}

/// #1795 `T10`. Pins the two-name Root guard against a Root Agent whose
/// directory is an alias onto a replica path: the raw basename is
/// `ac-root-agent` and the canonical basename is `__agent_<name>`. Guarding on
/// one name only would create directories on that Root Agent's behalf. The
/// canonical argument is a layout the resolver ACCEPTS, which is what keeps
/// control `C8` alive: with a canonical root the resolver rejects anyway,
/// deleting the guard would change nothing. No junction and no privilege are
/// needed, because the guard is a pure function of the two strings.
///
/// The `room-shared` absence is asserted FIRST: `C8` flips both creations at
/// once, so the panic lands on whichever absence is written first, and 8.3 names
/// `room-shared` as `C8`'s kill site.
#[test]
fn root_agent_alias_creates_no_shared_locations() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ac_root, room_root, replica_root, _matrix_root) = make_valid_replica(temp.path());
    let replica = display_path(&replica_root);
    let root_alias = display_path(&temp.path().join("alias").join("ac-root-agent"));

    for (raw, canonical, order) in [
        (
            root_alias.as_str(),
            replica.as_str(),
            "raw Root, canonical replica",
        ),
        (
            replica.as_str(),
            root_alias.as_str(),
            "raw replica, canonical Root",
        ),
    ] {
        ensure_replica_shared_locations(raw, canonical);

        assert!(
            !room_root.join("room-shared").exists(),
            "no `room-shared` may be created for a Root Agent alias ({order})"
        );
        for name in ["plans", "tools", "errors", "project-shared"] {
            assert!(
                !ac_root.join(name).exists(),
                "no `{name}` may be created for a Root Agent alias ({order})"
            );
        }
    }
}

/// #1795 `T1`. Renders through the production resolver from a template written
/// to a real temp disk, and asserts on paths derived from that same disk. This
/// is also the proof for row 1 of plan 3.7: the template is ALREADY
/// MATERIALIZED before the resolve, so it exercises the replace chain, not a
/// synthetic default. No literal fixture path appears in any assertion.
#[test]
fn golden_rule_room_replica_renders_entries_five_and_six_on_disk() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ac_root, room_root, replica_root, matrix_root) = make_valid_replica(temp.path());
    std::fs::write(
        ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME),
        get_default_agent_template(),
    )
    .expect("materialize the default template on disk");

    let agent_root = display_path(&replica_root);
    let skills = render_skills_section(&discover_skill_index(Some(&matrix_root)));
    let out = resolve_agent_context_with_activation(
        &agent_root,
        Some(&matrix_root),
        &skills,
        &replica_root,
        None,
        None,
        None,
    )
    .expect("resolve the materialized template");

    assert_eq!(
        out.matches("5. **Project shared locations").count(),
        1,
        "entry five must render exactly once"
    );

    // AC-9(1): exact join equality, computed in the test from the canonicalized
    // tempdir. The four names are LITERALS here, never read from
    // `PROJECT_SHARED_DIRS`, so control `C5` can kill this test.
    let plans = display_path(&ac_root.join("plans"));
    assert!(out.contains(&plans), "entry five must name `{plans}`");
    let tools = display_path(&ac_root.join("tools"));
    assert!(out.contains(&tools), "entry five must name `{tools}`");
    let errors = display_path(&ac_root.join("errors"));
    assert!(out.contains(&errors), "entry five must name `{errors}`");
    let project_shared = display_path(&ac_root.join("project-shared"));
    assert!(
        out.contains(&project_shared),
        "entry five must name `{project_shared}`"
    );

    assert_eq!(
        out.matches("6. **Room shared locations").count(),
        1,
        "entry six must render exactly once"
    );
    let room_shared = display_path(&room_root.join("room-shared"));
    assert!(
        out.contains(&room_shared),
        "entry six must name `{room_shared}`"
    );
    let task = display_path(&room_root.join("TASK.md"));
    assert!(out.contains(&task), "entry six must name `{task}`");

    assert!(
        out.contains("Reading `TASK.md` is granted; writing it is NOT."),
        "entry six must state the read/write split"
    );
    assert!(
        out.contains("task-set-title"),
        "entry six must name the verb"
    );
    assert!(
        out.contains("task-append-body"),
        "entry six must name the verb"
    );
    assert!(
        out.contains("task-set-body"),
        "entry six must name the verb"
    );
    for literal in [
        "task-get --token",
        "task-status-set --token",
        "--expected-revision",
        "--request-id",
        "complete status",
        "missing status is null",
        "Direct filesystem access",
        "partial backups",
        "TASK-status.jsonl, history, backups, partial backups, lock, stages, TASK-clean.pending.json, TASK-status.* and TASK.md.lock",
        "Get returns complete task/latest status; missing status is null",
        "--text replaces complete status: remaining tickets, follow-ups, continuation",
        "Use returned revision/new request UUID",
        "Retry uncertain writes with identical ID/base revision/text: exact last-request retry replays receipt",
        "Overtaken retry/revision conflict: read/reconcile before new UUID",
        "No peer-memory/other-room access",
        "TASK-clean.pending.json",
        "Direct filesystem access (reads/writes/deletes/renames) is forbidden",
        "Never auto-rebase",
        "configured-CLI exception governs all managed IO",
    ] {
        assert!(
            out.contains(literal),
            "missing status instruction: {literal}"
        );
    }
    assert!(out.contains(&display_path(&room_root.join("TASK-status.jsonl"))));
    assert!(
        out.contains("All filesystem access not authorized by entries 1-6 is OFF-LIMITS"),
        "the closure sentence must claim six entries"
    );
    assert!(
        !out.contains("entries 1-4"),
        "no four-entry range may survive for an authoritative replica"
    );
    assert_eq!(out.matches("## GOLDEN RULE").count(), 1);
    assert!(!out.contains("{{"));
    assert!(!out.contains("}}"));

    // AC-9(2): the ONE prefix relation this layout permits. `<ac>/plans` is a
    // sibling of the room and `<room>/room-shared` is a sibling of the agent
    // directory, so no prefix relation holds between them in either direction.
    let ac_prefix = display_path(&ac_root);
    for rendered in [
        &plans,
        &tools,
        &errors,
        &project_shared,
        &room_shared,
        &task,
        &agent_root,
    ] {
        assert!(
            rendered.starts_with(&ac_prefix),
            "`{rendered}` must start with `{ac_prefix}`"
        );
    }
}

/// #1795 `T2`. The Root Agent must render no room entries and no duplicated
/// project grant. Built under a `.ac` AND a room ancestor on purpose: the
/// `temp_root_agent_dir` helper builds `temp/<ROOT_AGENT_DIR_NAME>` with
/// neither, which is why round 1's control against it was dead.
///
/// The four absences are ONE loop so this test contributes exactly one
/// such range literal to the 3.9.1 counts.
#[test]
fn golden_rule_root_agent_renders_no_shared_location_entries() {
    let temp = tempfile::tempdir().expect("tempdir");
    let ac_dir = temp.path().join(".ac");
    let root = ac_dir.join("wg-1-demo").join("ac-root-agent");
    std::fs::create_dir_all(&root).expect("create root under a `.ac` and room ancestor");
    let root_str = path_string(&root);

    let out = default_context_as_root(&root_str, None, &no_skill_section());

    for forbidden in [
        "5. **Project shared locations",
        "6. **Room shared locations",
        "room-shared",
        "entries 1-6",
    ] {
        assert!(
            !out.contains(forbidden),
            "the Root Agent block must not contain `{forbidden}`"
        );
    }
    assert!(
        out.contains("3. **Every registered AgentsCommander project folder"),
        "the existing project-wide Root grant must still be present, not duplicated"
    );
    assert!(
        !root
            .parent()
            .expect("room root")
            .join("room-shared")
            .exists(),
        "no `room-shared` may be created beside the Root Agent"
    );
}

/// #1795 `T3`. A direct-Matrix agent is rejected at check 1: its basename is
/// `_agent_`, not `__agent_`. One loop, one such range literal.
#[test]
fn golden_rule_direct_matrix_agent_renders_no_shared_location_entries() {
    let out = default_context("C:/fake/.ac/_agent_dev-rust", None, &no_skill_section());

    for forbidden in [
        "5. **Project shared locations",
        "6. **Room shared locations",
        "room-shared",
        "entries 1-6",
    ] {
        assert!(
            !out.contains(forbidden),
            "the direct-Matrix block must not contain `{forbidden}`"
        );
    }
}

/// #1795 `T3b`, the fail-closed test. Four inputs, each of which ENTERS branch A
/// (so entry 4 renders and the test is not vacuous) but is not an authoritative
/// replica. The `matrix_root` column is the entry-3 argument, not a resolver
/// input: `replica_shared_roots` is a function of `agent_root` alone, so it
/// cannot block or unblock entries 5 and 6. It is passed so entry 3 renders,
/// which proves the block under test is the full replica block.
///
/// The assertion ORDER is load-bearing and prescribed by plan 8.1: the three
/// absences first, the range last. Controls `C3`, `C3b` and `C3c` flip an
/// input's entries and therefore die at assertion 2, told apart by the input
/// named at the start of the message; `C9` flips only the printed range and
/// therefore dies at assertion 5. Written as ONE loop, so this test contributes
/// exactly one literal of each range to the 3.9.1 counts.
#[test]
fn golden_rule_non_replica_under_a_room_keeps_the_four_entry_text() {
    for (agent_root, matrix_root, why) in [
        (
            "C:/fake/.ac/room-7-dev-team/notanagent",
            Some("C:/fake/.ac/_agent_dev"),
            "check 1: the leaf is not an `__agent_` replica",
        ),
        (
            "C:/fake/.ac/room-7-dev-team/__agent_dev/sub/nested",
            Some("C:/fake/.ac/_agent_dev"),
            "check 1: the leaf is `nested`",
        ),
        (
            "C:/fake/.ac/room-7-dev-team/sub/__agent_x",
            Some("C:/fake/.ac/_agent_dev"),
            "check 2: the direct parent `sub` carries no room prefix",
        ),
        (
            "C:/fake/room-7-dev-team/__agent_dev",
            Some("C:/fake/_agent_dev"),
            "check 3: the room's parent is not named `.ac`",
        ),
    ] {
        let values =
            default_context_dynamic_values(agent_root, matrix_root, &no_skill_section(), false);
        let out = render_write_restrictions_block(agent_root, &values);
        assert!(out.contains("FORBIDDEN: reads/writes outside entries 1-4, including other replicas, unlisted Matrix files"));
        assert!(out.contains("Other agents' memory* directories (live memory/, memory-archive/ or legacy memory_YYYYMMDD_hhmmss/), plans, skills and Role.md are private: never read/list/search/summarize, even if asked; message the owner."));
        assert!(out.contains("CLI exception: only user-explicit documented commands through AGENTSCOMMANDER_BINARY_PATH"));

        assert!(
            out.contains("4. **Messaging access:**"),
            "{agent_root}: must reach branch A, or this case is vacuous ({why})"
        );
        assert!(
            !out.contains("5. **Project shared locations"),
            "{agent_root}: must not render entry five ({why})"
        );
        assert!(
            !out.contains("6. **Room shared locations"),
            "{agent_root}: must not render entry six ({why})"
        );
        assert!(
            !out.contains("room-shared"),
            "{agent_root}: must not name a room-shared path ({why})"
        );
        assert!(
            out.contains("All filesystem access not authorized by entries 1-4 is OFF-LIMITS"),
            "{agent_root}: must keep the four-entry closure ({why})"
        );
        assert!(
            !out.contains("entries 1-6"),
            "{agent_root}: must not claim six entries ({why})"
        );
    }
}

/// #1795 `T7`. Pins row 2 of plan 3.7. A materialized template that has LOST
/// `{{WRITE_RESTRICTIONS}}` but KEPT `{{AGENT_ROOT}}` keeps its stale inline
/// Golden Rule block, because `coarse_section_dedup_safe` skips the fallback
/// append for that shape, so it never gains entries 5 and 6.
///
/// This is the EXPLICIT SCOPE REDUCTION ruled by the tech lead (plan 2.1): that
/// template shape is out of scope for #1795 and the issue's acceptance criterion
/// was amended to match. This test pins the reduced behavior. It is NOT a claim
/// that the outcome is desirable, and `coarse_section_dedup_safe` is unchanged.
#[test]
fn materialized_template_without_write_restrictions_token_keeps_its_inline_block() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ac_root, _room_root, replica_root, matrix_root) = make_valid_replica(temp.path());
    let template = concat!(
        "# AgentsCommander Context\n\n",
        "## GOLDEN RULE — Repository Access Restrictions\n\n",
        "**ABSOLUTE AND NON-NEGOTIABLE:** this stale inline block was baked by an older build.\n\n",
        "2. **Your own agent replica root and descendants:**\n   ```\n   {{AGENT_ROOT}}\n   ```\n",
    );
    std::fs::write(ac_root.join(GLOBAL_CONTEXT_TEMPLATE_FILENAME), template)
        .expect("materialize a token-less Golden Rule template");

    let agent_root = display_path(&replica_root);
    let skills = render_skills_section(&discover_skill_index(Some(&matrix_root)));
    let out = resolve_agent_context_with_activation(
        &agent_root,
        Some(&matrix_root),
        &skills,
        &replica_root,
        None,
        None,
        None,
    )
    .expect("resolve the token-less template");

    assert!(
        !out.contains("5. **Project shared locations"),
        "the reduced shape must not gain entry five"
    );
    assert_eq!(
        out.matches("## GOLDEN RULE").count(),
        1,
        "the inline Golden Rule must not be duplicated by the fallback append"
    );
}

/// #1795 `T9`. `T1` pre-canonicalizes its own fixture, so it never exercises
/// `ensure_session_context_with_config:157-159`. This one drives production
/// canonicalization: the replica root is spelled with a DOUBLED separator, which
/// `Path::components` skips and `std::fs::canonicalize` collapses on every
/// platform without needing any privilege.
#[test]
fn production_canonicalization_reaches_the_shared_location_entries() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ac_root, room_root, _replica_root, _matrix_root) = make_valid_replica(temp.path());

    let sep = std::path::MAIN_SEPARATOR;
    // `display_path`, NOT `path_string`. `make_valid_replica` returns
    // canonicalized paths, which on Windows carry the `\\?\` VERBATIM prefix, and
    // Windows does not normalize a verbatim path: it is handed to the filesystem
    // as-is, so `std::fs::canonicalize` FAILS on a verbatim path with a doubled
    // separator and `:157-159` silently falls back to the raw string. The test
    // would then pass while proving nothing. Stripping the prefix first gives a
    // normal path, which Windows does collapse, so production really does
    // canonicalize before it resolves.
    let spelling = format!("{}{sep}{sep}__agent_dev-rust", display_path(&room_root));
    assert!(
        spelling != display_path(&canonical_or_original(Path::new(&spelling))),
        "the fixture spelling must differ from its canonical form, or this test is vacuous"
    );

    let cached = ensure_session_context(&spelling).expect("ensure session context");
    let out = std::fs::read_to_string(&cached).expect("read the cached context");

    for expected in [
        display_path(&ac_root.join("plans")),
        display_path(&ac_root.join("tools")),
        display_path(&ac_root.join("errors")),
        display_path(&ac_root.join("project-shared")),
        display_path(&room_root.join("room-shared")),
        display_path(&room_root.join("TASK.md")),
    ] {
        assert!(
            out.contains(&expected),
            "the rendered path must be the canonical join `{expected}`"
        );
    }
    assert!(
        !out.contains(&format!("{sep}{sep}")),
        "no doubled separator may survive into the rendered block"
    );
}

/// #1795 `T11`. The session backstop is warn-and-continue: a shared directory
/// that cannot be created must not fail the session. A regular FILE is placed
/// where `plans` would go, so `create_dir_all` must fail.
#[test]
fn shared_location_creation_failure_does_not_fail_the_session() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ac_root, room_root, replica_root, _matrix_root) = make_valid_replica(temp.path());
    std::fs::write(ac_root.join("plans"), b"not a directory").expect("block `plans`");

    let cached = ensure_session_context(&display_path(&replica_root))
        .expect("a creation failure must not fail the session");
    assert!(
        Path::new(&cached).exists(),
        "the cached context file must still be written"
    );
    let out = std::fs::read_to_string(&cached).expect("read the cached context");
    assert!(
        out.contains("5. **Project shared locations"),
        "entry five must still render when its directory could not be created"
    );
    assert!(
        room_root.join("room-shared").is_dir(),
        "the first creation failure must not abort the second creation"
    );
}

/// #1795 `T13`. Proves the P4 WIRING, not the helper: a test that called
/// `ensure_replica_shared_locations` directly would stay green with the call
/// site deleted, which is what control `C7` probes. The four project
/// directories are asserted FIRST, because `C7` stops every creation at once and
/// 8.3 names that assertion as its kill site.
#[test]
fn session_start_creates_shared_locations_for_a_replica() {
    let temp = tempfile::tempdir().expect("tempdir");
    let (ac_root, room_root, replica_root, _matrix_root) = make_valid_replica(temp.path());
    for name in ["plans", "tools", "errors", "project-shared"] {
        assert!(
            !ac_root.join(name).exists(),
            "the fixture must start with no `{name}`"
        );
    }

    let cached =
        ensure_session_context(&display_path(&replica_root)).expect("ensure session context");

    for name in ["plans", "tools", "errors", "project-shared"] {
        assert!(
            ac_root.join(name).is_dir(),
            "a session start must create `{name}` under the project `.ac` root"
        );
    }
    assert!(
        room_root.join("room-shared").is_dir(),
        "a session start must create `room-shared` under the room root"
    );
    let out = std::fs::read_to_string(&cached).expect("read the cached context");
    assert!(
        out.contains("5. **Project shared locations"),
        "the cached content must carry entry five"
    );
}
