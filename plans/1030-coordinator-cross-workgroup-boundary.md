# Plan: #1030 coordinator cross-workgroup delegation boundary

Author: architect (wg-12). Branch `fix/1030-coordinator-cross-workgroup-boundary`, base `main` @ `4acadfe5b22e67dff40cd20eda87b23eca4a7cbe`.
Status: READY_FOR_IMPLEMENTATION

Certified by architect at Full-path Step 7, round 3 (final), after dev-rust's Step 5 enrichment and two grinch review rounds. The verdict, the round-3 blocker resolutions, and the two accepted risks are recorded at the end of this plan; the reasoning there is binding, not commentary.

**Read this before implementing:** sections 5, 8 and 9 are the repo specification (dev-rust). Section 6.3 is the Agent Matrix rollout, which **no repo commit can perform** and which the user or Root executes in the stated order, gated by A4-pre, A7 and A8. Sections 2 and 10 are evidence; the two Grinch Review sections and the retractions inside 6.3 are the review record. Where a retraction quotes an earlier claim, the quoted claim is **wrong by construction** and the surrounding text says so.

---

## 1. Issue and objective

Issue #1030. `.ac/Context.coordinator.md` is project-scoped: one shared body appended to every coordinator session except Root (`session_context.rs:1844`). It says nothing about crossing workgroup boundaries, so the boundary is restated per-role and the restatements have drifted.

Per the user-approved `## Decision (2026-07-16)` section of the issue, code enforcement and prose are complementary, not alternatives: `can_communicate` denies at the moment of the **call** (after a full wasted turn, with no stated reason and no escape), while prose prevents at the moment of the **intent**. Objective: move the boundary into the shared coordinator body, remove the drifted per-role restatement, and align the routing doc with the code.

## 2. Evidence and current-state gap

All values below were measured at base `4acadfe5`, not inherited.

**E1. The default coordinator template carries no cross-workgroup rule.** `get_default_coordinator_template()` (`session_context.rs:2139-2160`) contains no occurrence of "workgroup".

**E2. Provenance of the outgoing default (v3).** The shipped accessor at `4acadfe5` was extracted mechanically (no hand-copy) into a standalone program, compiled with `rustc --edition 2021`, and its output written to disk:

- v3 length: **2296** bytes
- v3 sha256: **`9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84`**
- pure ASCII, LF only, no CR, no U+2014

The same pipeline was validated against the already-pinned v2 snapshot by running the accessor at `1dd0b58`: it reproduced **2403** / `92f3abfc108147b07f1c4a49e7062c0f4d0d9aae570b7e5195852c31bb8b0d02` exactly, matching both the const's doc comment and the `plans/1005-s4-checklist.md` record (which derived the same numbers by an independent python unescape). The method is therefore trustworthy for v3.

**E3. The live on-disk body is pristine v3 with trusted seed state.** `.ac/Context.coordinator.md` in this workspace measures **2296** bytes / `9f72fa83...`, byte-identical to E2, and contains **zero** occurrences of "workgroup" (confirming the issue's claim). `.ac/.agentscommander-context-templates.json` holds a trusted entry: `templateId: "coordinator"`, `currentVersion: 3`, `lastSeededSha256: "9f72fa83..."` (equal to the on-disk hash), all `ignored*` fields `null`.

**E4. Migration mechanism (the question the issue's Decision section left open; answered here).** `read_or_create_context_template` (`session_context.rs:1037-1061`) calls `seeded_context_templates::sync_project_context_template_for_read` **before** every read, which runs `sync_one_template(project_dir=None, ..., allow_create_missing=true, return_pending=false)` (`seeded_context_templates.rs:750-846`). Decision order, for the coordinator spec (`project_actionable: true`, `suppress_unknown_without_state: false`):

| On-disk state | Branch | Outcome |
|---|---|---|
| missing | `:763-775` | created with the current default |
| == current default | `:781-784` | no-op, marked seeded |
| trusted entry with `last_seeded_sha256` == file hash, and `is_known_generated(content)` | `:787-797` | **`auto_update_generated_template` overwrites with the new default** |
| no trusted entry, and `is_known_generated(content)` | `:800-805` | **auto-update** (a missing state file yields `trusted: true` with an empty map, so `has_valid_entry` is false: `load_state` `:483-490`) |
| user previously dismissed this exact (file, default) pair | `:815-821` | skipped permanently |
| anything else | `:823-845` | preserved, WARN logged, surfaced to the UI as a pending update |

Per E3 this workspace lands on the third row. **The change therefore needs no new migration code**: existing pristine bodies auto-update on the next coordinator session launch, conditional on the freeze in section 4.

**E5. The freeze is load-bearing.** The auto-update branch requires `is_known_generated_coordinator_template(content)` (`:363-367`), which is byte-equality against the current default plus the frozen snapshots. If the default changes and v3 is **not** frozen, every pristine v3 body on disk stops being recognized, falls to the last row of the E4 table, and is misreported to the user as a customized template. The rule would silently never reach existing workgroups. This is the whole migration question, and the answer is the #1005 S4 freeze pattern, not new code.

**E6. The mechanism, measured.** `can_communicate` (`teams.rs:814-848`) is the gate the CLI enforces **for non-Root targets** (`cli/send.rs:374`), and the same function computes the `reachable` flag in `list-peers-lean` (`list_peers.rs:651,686,780,911`) and backs `MailboxPoller::can_reach` (`mailbox.rs:6074`). **Correction (grinch finding 5, verified):** a Root Agent target never reaches it. `send.rs:356-369` is an earlier arm of the same `else if` chain and gates Root targets on `coordinator_to_root_target_allowed` instead, which is `verified_wg_coordinator_target(sender, paths).is_some()` (`send.rs:110-112`). So a verified workgroup coordinator replica may address the Root Agent directly, and that path is invisible to `can_communicate`. This does not affect any coordinator-to-coordinator claim below, but it does change the doc row (5.3). A temporary probe test (five assertions, run under `cargo test --lib`, then reverted; repo left clean) confirmed all five predictions:

| Case | Result |
|---|---|
| same team, different workgroup, member to member | **ALLOWED** |
| same team, different workgroup, coordinator to member | **ALLOWED** |
| different team, different workgroup, member to member | denied |
| different team, different workgroup, member to coordinator | denied |
| different team, different workgroup, coordinator to coordinator | **ALLOWED** |

**E7. Two premises in the issue are wrong, and section 4 depends on the correction.**

- The issue states "delegating to another workgroup's *members* is already impossible in code". **False for same-team replicas.** `extract_wg_team` (`teams.rs:629-638`) discards the workgroup number, so `is_in_team` is workgroup-blind and rule 1 of `can_communicate` admits `proj:wg-1-dev-team/dev-rust -> proj:wg-2-dev-team/dev-rust`. Cross-workgroup member contact is blocked only across *different* teams. This does not weaken the change; it strengthens it, because for same-team replicas the prose is the **only** thing enforcing "never its members". The user ruled this a **defect**, not a quirk: agents in different workgroups must not talk directly. Filed as **#1041**, out of scope here (section 3), and it governs how 5.3 may word the doc.
- `docs/agents/inter-agent-messaging.md:106` claims coordinators reach other coordinators "only via the Root Agent". Contradicted by E6: rule 3 admits direct coordinator-to-coordinator contact for any team, with no Root Agent in the path.

**E8. The mandated rule text contradicts an existing shared block.** `DEFAULT_DELEGATED_TASK_REPORTING` (`session_context.rs:2741-2743`) is rendered into every agent context, coordinators included: "When finishing a delegated task or getting blocked, reply to the coordinator or peer with a concrete artifact or message." It is unconditional. The rule as worded in the issue gates *reaching* another workgroup on role/user/Root authorization with no exemption for replies. A coordinator whose role is silent on cross-workgroup authority (which is every tech-lead once its restatement is removed) would receive both directives in the same materialized file: reply, and do not reply. See section 4.1.

**E9. Restatement inventory (issue scope item 5).** All 41 `_agent_*/Role.md` files in the Agent Matrix were scanned. Exactly **one** cross-workgroup messaging restatement exists: `_agent_tech-lead/Role.md:7`. `_agent_shipper/Role.md:48,53` mention "another workgroup" but govern killing processes and executables, not messaging; they are **out of scope** and must not be touched.

**E10. Roles that depend on the `your role` escape.** Every matrix coordinator is affected, because the body reaches all of them. The roster is deliberately not reproduced here (see "Matrix names" at the end of this plan); enumerate it from the matrix on disk, which is authoritative and current. **Use the 6.3 criterion command, not a bare coordinator list:** it joins each team configuration to its member count, and that count is what exposes an initiator (E10a). The attack case is real and present:

- `_agent_project-director/Role.md:5` "tracking progress across workgroups"; `:17` "Assign each task to the right tech-lead and workgroup"; `:21` "Monitor status across active workgroups". Cross-workgroup dispatch is its defining function.
- One further coordinator (the acceptance-testing one; `Role.md:9` reads "queue incoming test requests **from other tech-leads** ... report PASS/FAIL/BLOCKED results back to requesters") takes cross-workgroup work **inbound only**. Its requesters are by definition other workgroups' coordinators, so it depends on the **reply** path of E8. It needs no grant, because it initiates nothing across a workgroup (6.3, M1 criterion).

**E10a. Two more initiators, found only by topology (grinch round-1 finding 1, verified).** E10 as first written read role prose and concluded that project-director was the only initiator. That conclusion was **wrong**, and the method was why: **a role can be an initiator without ever using the word "workgroup".** Joining each team configuration to its member count returns two teams whose only member is the coordinator itself. A singleton coordinator has no same-workgroup peer, so **any** outbound peer duty it holds necessarily leaves the workgroup, by construction; both roles hold one (one is chartered to dispatch rounds to a contender class, the other's protocol has its holder always initiate contact with a counterpart). Both are stranded exactly as project-director is, and the reply carve-out saves neither, because both **initiate**.

So M1 covers **three** role files, not one, and the qualifying test is a property of the graph, not of the prose. The durable predicate, the enumeration command, and the tailoring rule are in 6.3; the two names are private and stay out of this plan by ruling.

**E13. The status-quo denominator, corrected (grinch round 2, verified independently).** Round 1 of this plan claimed "11 of 12 teams' coordinators" lack the boundary. That was **wrong**, and the error was mine: I counted team configurations as if each had a distinct coordinator. Re-measured by joining every `_team_*/config.json` to its coordinator and de-duplicating identities:

- **12** team configurations, **9** unique coordinator identities.
- Two coordinators own more than one team configuration; the one carrying the old restatement owns **three**.
- Exactly one coordinator identity's `Role.md` contains "outside your workgroup".
- Therefore: **9 of 12 team configurations**, and **8 of 9 unique coordinator identities**, carry no cross-workgroup boundary today.

The M2-early argument in 6.3 rests on this number, so it is stated here rather than inline. The correction does not reverse it: 8 of 9 is still an overwhelming majority, and the conclusion is unchanged. It is fixed because an argument made on measured grounds has to survive its own measurement being audited.

**E11. No other surface duplicates the body.** No file under `docs/` or `src/` quotes "You are the coordinator for your team", so the rule text has exactly one home. `ContextTemplateUpdate.currentDefaultVersion` already exists in `src/shared/types.ts:1027`, so the version bump needs no TypeScript change.

**E12. No test pins the current default's length or hash.** `token_accounting_report` (`session_context.rs:8264`) is `#[ignore]`d and asserts only non-emptiness. `coordinator_template_no_longer_carries_inline_self_maintenance` (`:6255`) asserts the template stays free of U+2014. All 34 `config::seeded_context_templates` tests pass at base.

## 3. Scope

**In scope**

1. Add the rule to `get_default_coordinator_template`.
2. Freeze v3, extend the recognizer, bump the coordinator template version, pin provenance.
3. Fix `docs/agents/inter-agent-messaging.md:106`, plus a one-line correction of the same table's cross-workgroup reach (E7).
4. Specify (not perform) the Agent Matrix rollout: a **tailored** cross-workgroup authority grant for each of the **three** coordinators the 6.3 criterion selects (one of them `_agent_project-director/Role.md`; the other two private, selected by criterion and gated by A7), and the removal of `_agent_tech-lead/Role.md:7`. All are user or Root actions, and all precede the deploy (6.3).

**Out of scope**

- Any change to `can_communicate`, `is_in_team`, or the routing topology. Code enforcement is unchanged by this issue; `#330` owns that.
- The same-team cross-workgroup member reach found in E7. Narrowing it is a mechanism change, not a prose change. Now filed as **#1041** and owned there; this plan only documents it as a known deviation (5.3).
- `_agent_shipper/Role.md` (E9), `docs/agent-matrix-conventions.md` (E11), any TypeScript, and any other role's authority text.
- Rewriting any matrix role **from this plan or from a repo commit**. `_agent_project-director/Role.md` needs a one-sentence authority grant, but the user applies it: see the mandatory rollout step in 6.3.

## 4. Decided solution

### 4.1 The rule text

One bullet is added to the "You must:" list. The issue's mandated sentence survives **verbatim**; a reply carve-out is appended after a semicolon:

> - To reach another workgroup, message its coordinator, never its members, and only when your role, the user, or the Root Agent authorizes it; replying to a coordinator who messaged you first is always authorized.

213 bytes, pure ASCII, no U+2014, LF only.

**Why the carve-out is added. Approved by the user on 2026-07-16 (tech-lead ruling 1); the text above is final.** The `when your role ... authorizes it` clause is preserved unchanged, so the primary attack case is answered exactly as the issue intends: a role whose defining function is cross-workgroup dispatch (E10: project-director) states its own authority, the shared rule defers to it, and a role that says nothing defaults to not authorized. But the issue's own analysis names a **second** half of that attack case: "because the rule gates *contact* with no exemption for replies, the receiving coordinator cannot reply either. Both sides deadlock." The mandated wording does not close it. Its three authorizers are role, user, and Root Agent; a peer coordinator who initiates contact is not among them. So a tech-lead, whose restatement this plan removes and whose role is then silent, would be forbidden from answering a project-director dispatch, while `DEFAULT_DELEGATED_TASK_REPORTING` (E8) simultaneously orders it to reply. That is a live contradiction inside one materialized context file, and the acceptance-testing coordinator's entire inbound workflow (E10) depends on the reply path. The carve-out is scoped to *replying to whoever messaged you first*. **What it is not (grinch finding 4, verified):** this is guidance, not a causal or security boundary, and the plan must not claim otherwise. `OutboxMessage` (`phone/types.rs:7-57`) carries id, from, to, body, mode, priority and timestamp, and **no** `reply_to`, thread, causal-parent, or expiry field: a grep of the whole file for those returns zero. The delivered notification adds the sender but no causal link (`cli/send.rs:436-468`). So nothing in the system distinguishes a reply from a fresh initiation, a relay to a third workgroup, or unrelated contact long after the fact, and after a context clear the agent may not retain the evidence of who messaged first. **Accepted operational interpretation, which is what the bullet means:** the exception covers a direct response to the same visible inbound sender; it does not cover a new delegation or a third-party relay, and where the inbound evidence is unavailable the exception is not established. Stronger semantics would need reply/thread state tracked as a separate enforcement change; #1030 does not implement one and does not claim one.

**Why it does not contradict the mechanism.** "message its coordinator, never its members" is stricter than `can_communicate`, never looser: E6 shows code permits same-team cross-workgroup member contact, and the prose forbids it. Prose narrowing a permitted channel is the intended relationship (code is the hard boundary, prose is the guidance and the escape). The rule never claims a channel the code denies.

### 4.2 Migration

**Decision: no new migration code. Freeze v3 and let the existing sync retro-update.** Per E4 and E3, the live pristine body auto-updates on the next coordinator session launch, and customized bodies are preserved and surfaced to the user (never silently overwritten; a user-accepted overwrite writes a backup first). Per E5 this is conditional on freezing v3 into the recognizer in the same commit, which is also the #1005 S4 precedent.

## 5. Affected surfaces

| File | Symbol | Change |
|---|---|---|
| `src-tauri/src/config/session_context.rs` | `get_default_coordinator_template` (`:2139`) | insert the 4.1 bullet |
| `src-tauri/src/config/seeded_context_templates.rs` | new const `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE` | frozen v3 bytes |
| same | `is_known_generated_coordinator_template` (`:363`) | add the new const |
| same | `project_specs()` coordinator spec (`:295-304`) | `current_version: 3` -> `4` |
| same | tests | provenance pin + migration tests (section 9) |
| `docs/agents/inter-agent-messaging.md` | routing table (`:104-108`) | fix the Coordinator row + reach note |
| `_agent_project-director/Role.md` (Agent Matrix, **not the repo**) | `## Communication Protocol` (after `:55`) | broad grant added by the user or Root, **before deploy** (6.3, M1) |
| Two further criterion-selected coordinator roles (Agent Matrix, **not the repo**; names private, selected by the 6.3 criterion, gated by A7) | beside each role's existing outbound duty | **tailored** grant added by the user or Root, **before deploy** (6.3, M1) |
| `_agent_tech-lead/Role.md` (Agent Matrix, **not the repo**) | line 7 | removed by the user or Root, **before deploy** (6.3, M2) |

### 5.1 `get_default_coordinator_template`

Insert one line between the existing `- Route each part of a request ...` and `- Sequence work, track progress ...` bullets, matching the surrounding `\n\` continuation style exactly:

```rust
     - To reach another workgroup, message its coordinator, never its members, and only when your role, the user, or the Root Agent authorizes it; replying to a coordinator who messaged you first is always authorized.\n\
```

Placement is fixed by this section, not the implementer's choice: the rule qualifies the routing/delegation bullet it follows, and the anchor bullet is unique in the body (10.1). Resulting default: 2509 bytes, sha256 `f6ef7894b9f0f606e945c282d769144e96487fcc01ab435c9aab8019bb3ce1f6` (independently re-derived by dev-rust, 10.1). Because the placement is fixed, those bytes are fully determined: a mismatch means the edit deviated from 5.1. **No unit test pins it** (the pattern pins frozen snapshots only, never the live default, and a future legitimate edit would have to churn it), **but it is binding at A4**, where it is the external value the deployed file is compared against. That is what stops A4 from being circular (grinch finding 6): comparing the live file to `get_default_coordinator_template()` proves only that the file matches whatever was implemented, correct or not.

### 5.2 The frozen snapshot

Add beside the existing snapshots, following `COORDINATOR_CONTEXT_TEMPLATE_BEFORE_TOKEN_MINIMIZATION` (`:73-109`) as the template for both the const and its doc comment. Its body is the v3 literal moved verbatim from `get_default_coordinator_template` at `4acadfe5` (copy the accessor's literal before editing it, so the two cannot diverge). The doc comment must record: frozen as the third legacy snapshot, never edit, and the E2 provenance (one-off run of the shipped accessor at `4acadfe5` printed len 2296, sha256 `9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84`), pinned against those externally captured values and never against the const itself.

Keep the string-literal-with-continuations form. Do **not** use `include_str!` (the `STANDALONE_GLOBAL_CONTEXT_BEFORE_CORE_CONCEPTS` comment at `:116-119` explains why: on a CRLF checkout it would silently stop recognizing the default).

### 5.3 The doc fix

Replace the Coordinator row at `:106`:

```
| Coordinator | Any team member; other coordinators only via the Root Agent. |
```

with:

```
| Coordinator | Any team member; any other coordinator directly, with no Root Agent relay; the Root Agent directly, from a verified workgroup coordinator replica. |
```

The Root Agent is an **authorizer**, not a relay for coordinator-to-coordinator traffic, but it is itself directly reachable (E6 correction), and the new rule names it as an authorizer, so a table that omits it hides the channel to the very authorizer the rule points at. The row must state both facts. Add the following below the table. It must be framed as a tracked defect, never as designed behavior (tech-lead ruling 3): documenting the deviation without the #1041 reference would teach the next reader that a bug is a feature. #1041's body already records the reciprocal obligation to delete this note when it lands.

```
**Known deviation, tracked in #1041:** "sharing a team" currently ignores the workgroup number, so two replicas of the same team in different workgroups can address each other directly, bypassing both coordinators. This is a defect, not intended behavior, and it contradicts the coordinator-only rule above. #1041 makes the same-team rule workgroup-aware; when it lands, this note is removed. Reaching a *different* team's workgroup is already coordinator-to-coordinator only.
```

Leave the Worker and Root Agent rows unchanged: E6 confirms "The team's coordinator + peers sharing a team" is accurate.

## 6. Required behavior, edge cases, failure behavior

**6.1 Required behavior**

- A new project seeds `.ac/Context.coordinator.md` with the rule (E4 row 1).
- An existing project with a pristine v1, v2, or v3 body has it overwritten with the new default on the next coordinator session launch (E4 rows 3 and 4), and every coordinator session in that project then carries the rule.
- A customized body is preserved byte-for-byte and reported as a pending update. Unchanged behavior.
- Root never receives the body (`session_context.rs:1844` guard). Unchanged.

**6.2 Edge cases**

| Case | Behavior |
|---|---|
| body customized, then user accepts the offered overwrite | previous bytes written to a backup first, then the new default (`overwrite_creates_backup_and_writes_default`) |
| body customized, then user dismisses | `mark_ignored` records the (file, default) pair; no further prompting for that pair |
| state file missing or corrupt | `trusted: true` with an empty map, or `trusted: false`; either way a pristine body still auto-updates via the `!has_valid_entry` branch (E4 row 4) |
| body is a symlink, reparse point, non-file, or invalid UTF-8 | untouched; `read_validated_snapshot` refuses it. Unchanged |
| file changes between snapshot and write | `auto_update_generated_template:729-735` re-reads, sees the hash moved, and preserves the file |
| pristine v1 or v2 body still on disk | still recognized; the two existing snapshot consts stay in the recognizer and must not be removed |

**6.3 The Agent Matrix changes (who, what, and in what order)**

Rewritten at Step 7 round 1 (grinch findings 1, 2, 3) and again at round 3 (grinch blockers 1, 2, 3, 4, 5). Three claims that governed earlier versions are retracted: that M1 is one file, that a delayed M2 is harmless, and that one shared grant paragraph widens nothing. All three were wrong, and the first two point the same way, which is that **every matrix action belongs before deploy**.

Matrix files live outside the repo, under `C:\Users\maria\0_repos\AgentsCommander_ac\.ac\`. **No repo commit can perform any of them, and neither architect nor dev-rust may write there.** All are user or Root Agent actions, gated by A7 and A8.

**Retraction 1: M2 delayed is contradictory, not redundant (grinch finding 2, verified).**

The old text claimed an asymmetry: a grant is safe early, a restatement is safe late "because it is redundant with the shared rule, not contradictory". The second half is false, and the carve-out is what makes it false:

| Situation | `_agent_tech-lead/Role.md:7` (old) | 4.1 shared rule (new) |
|---|---|---|
| initiating cross-workgroup contact | forbidden unless the user or Root directs | forbidden unless role, user or Root authorizes (near-redundant) |
| **replying to a coordinator who messaged first** | **forbidden** unless the user or Root directs | **"always authorized"** |

The two disagree on the reply path, flatly. A tech-lead materialized after the v4 body but before M2 holds both, and a routine cross-workgroup dispatch then puts it in the deadlock this issue exists to remove. That is not hypothetical in this rollout: M1 exists precisely to let a cross-workgroup initiator dispatch, and the coordinator session that triggers A4 may itself be a tech-lead.

The claim was true when written and stopped being true when the carve-out was approved. I updated 4.1 and did not propagate the consequence here. The fix is to drop the asymmetry, not to patch around it.

**Resolution: M2 moves before deploy, alongside M1. I dissent from the proposed launch hold.**

Grinch's fix (hold every tech-lead launch, restart and clear across the A4-to-M2 interval, drive A4 from the granted initiator, release after M2) does prevent the contradiction. Two objections decide against it:

1. **Its failure mode is the thing it prevents.** The hold is enforced by a human across every workgroup that has a tech-lead, and any missed launch, restart or context clear inside the interval produces exactly the contradictory context it exists to avoid. A control whose only failure mode is its own hazard is weak, and its failures are silent after the fact.
2. **The window it protects does not need to exist.** It exists only because M2 was placed after deploy. Move M2 before deploy and the interval is empty, with no operator burden at all.

Moving M2 early opens the opposite window: between M2 and deploy, a **newly launched** tech-lead has no boundary statement at all. (A session already running is unaffected: its context was materialized at launch.) That window is acceptable, and the evidence is decisive: **it is the state almost every other coordinator is in right now.** E1 measures zero occurrences of "workgroup" in the shipped coordinator body, and E9 found exactly one restatement in the entire matrix, so **9 of 12 team configurations, and 8 of 9 unique coordinator identities, run today with no cross-workgroup boundary anywhere in their context** (E13). M2-early places tech-lead, briefly, in the configuration that is currently production for everyone else. It is a guidance gap, not an enforcement gap: `can_communicate` is untouched throughout.

So the trade is a novel contradictory state that reliably deadlocks a routine dispatch, against a brief absence of guidance that is indistinguishable from today's production state for most coordinators. The second is plainly smaller, and it costs no launch hold.

**Retraction 2: M1 is not one file (grinch finding 1, verified).**

My "one narrowing you get for free" read role prose and cleared every coordinator whose role never mentions workgroups. That standard is unsound, because **topology can make a role an initiator without the role ever saying "workgroup"**. A coordinator whose team has exactly one member (itself) has no same-workgroup peer, so *any* outbound peer or dispatch duty in its role necessarily crosses a workgroup, by construction.

Joining every team config's coordinator to its member count (method below; output deliberately not published) returns **two singleton-coordinator teams**, and both roles carry an explicit outbound duty: one is chartered to dispatch rounds to parties outside its own team, and the other's protocol states that its holder always initiates contact with its counterpart. Under this section's own standard, that a defining duty is not an authorization, both are stranded exactly as project-director is, and the carve-out helps neither, because both **initiate**.

The criterion, not a roster, defines M1:

> **An M1 grant is required when a role initiates an AgentsCommander peer message to a destination outside the sender's own workgroup**, including to another replica of the *same* team in a different workgroup. Contact with the user, with the Root Agent, or with any non-agent external system does **not** qualify: none of those is another workgroup. A role that merely *describes* such a duty does not thereby authorize it, which is the standard applied to project-director and must be applied uniformly.

**Rewritten at round 3 (grinch round-2 blocker 2).** The previous predicate was expressed in *team* terms while the rule is *workgroup*-scoped, which made it both incomplete and overbroad: clause (b) ("beyond its own team") would miss a multi-member coordinator that messages a same-team replica in another workgroup, which is exactly the topology E7 measures and #1041 will close; and clause (a) ("any outbound contact") would grant authority to a singleton whose only outbound duty is to the user or to an external system. Both were real defects in a criterion whose whole purpose is to be durable.

Singleton team membership is **evidence**, not the test: a coordinator whose team's member list contains only itself has no same-workgroup peer, so any outbound *peer* duty in its role necessarily leaves the workgroup. It still has to be an inter-agent peer destination to qualify.

Enumerate candidates from the matrix root. Publish the method, never its output:

```
python -c "import json,glob; [print(len(json.load(open(c,encoding='utf-8-sig')).get('agents') or []), json.load(open(c,encoding='utf-8-sig')).get('coordinator'), c) for c in sorted(glob.glob('_team_*/config.json'))]"
```

Every coordinator whose team shows 1 member is an initiator if its role states any outbound duty; then read each remaining coordinator's `Role.md` for outbound duties directed beyond its own team.

**Today the criterion selects three coordinators.** One is `_agent_project-director` (named by tech-lead ruling). The other two are the singletons; their names are private and are deliberately not recorded in this public plan (see "Matrix names"), and the command above names them locally to whoever runs the rollout. A7 gates all three by count, not by name.

**No workflow is broken irreparably, and this needs no ruling.** The "never its members" clause cannot forbid anything the code already denies, and for a *different* team's members the code denies today (E6: rule 3 requires both ends to be coordinators). So any cross-workgroup workflow that runs today is necessarily addressing one of: a coordinator, which the grant restores; an agent inside the sender's own workgroup, which rule 2 allows and which is not "another workgroup", so the rule never applies; or a same-team replica across workgroups, which is exactly the #1041 deviation this rule intends to forbid and #1041 will enforce. Every branch lands on intended behavior.

**The grant text: tailored per role, never a shared paragraph (grinch round-2 blocker 4, upheld with evidence).**

Round 2's single generic paragraph authorized every holder to contact **any** workgroup's coordinator to "dispatch work, follow up, or collect status". For project-director that matches its role. For the two singletons it does not: their duties are narrow and destination-specific (one is chartered to run a defined round protocol against contenders; the other's protocol names a single counterpart holding the same role). Handing either the generic paragraph would let a narrow specialist cite it to initiate unrelated dispatch or status collection against **any** coordinator, which its role never authorized. That is a **capability expansion**, and the claim that the grant "widens nothing" was false for two of the three files. It is also self-defeating: this change exists to state authority precisely, so smuggling a broad grant in through its own rollout step is the exact defect it is meant to remove.

Each grant is therefore **written against the role it lands in**, under these rules:

1. **Invariant clauses, identical in every grant** (they narrow, never widen):
   - the authority is explicit and needs no prior user approval for the duty it names, which is what defeats the shared rule's not-authorized default;
   - reach a workgroup **only through its coordinator, never its members**.
2. **Tailored clause:** name only the destination class and the actions the role **already** carries. Do not generalize the destination ("any coordinator") and do not add verbs the role does not already have. If a role's outbound duty is one protocol against one counterpart class, the grant says exactly that.
3. **No new duties.** A grant authorizes an existing duty; it never creates one.

For `_agent_project-director` the broad form is correct, because its role's duty *is* general cross-workgroup dispatch, and it is the one file this public plan may name. Exact text, in `## Communication Protocol`, immediately after the `**With tech-leads:**` paragraph at `:55`:

> **Cross-workgroup authority:** You are authorized to contact any workgroup's coordinator directly, without asking the user first, to dispatch work, follow up, or collect status. Reach a workgroup only through its coordinator, never its members.

Evidence that it needs one: the full 66-line role was read; it describes cross-workgroup dispatch as its defining function (`:5`, `:17`, `:21`, `:55`) and contains **no authorization language anywhere** (a grep for authoriz / permitted / allowed / may contact / directly returns only `:42`, about committing to `main`). It is a coordinator, so it receives the shared body (`grep -l '_agent_project-director' _team_*/config.json`).

For the other two the user writes the tailored text against the private role, applying rules 1 to 3, and places it beside the outbound duty it authorizes. The plan cannot carry that text without publishing the roles (tech-lead ruling), and it does not need to: the rules above are the specification, and **A7 verifies whatever paragraph is actually written**, per file, rather than requiring one paragraph to match everywhere. All grants U+2014-free.

**M2: remove `_agent_tech-lead/Role.md:7`**, which reads "Never communicate with agents outside your workgroup unless explicitly directed by the user or Root Agent."

**Full rollout order** (reordered at round 3: grinch blocker 5)

1. **A4-pre**: classify the live body **before touching anything**. It decides which branch the rest of the rollout takes, and whether removing M2 is safe at all. Round 2 placed M2 first, which let the operator remove the boundary and only then discover the body was customized.
2. **M1**: apply the tailored grant to every coordinator the criterion selects. Gate: **A7**.
3. **M2**: remove `_agent_tech-lead/Role.md:7`. Gate: **A8-pre**. Blocked until A4-pre has selected a branch.
4. The commit lands (section 8); the binary is built and deployed.
5. A coordinator session starts; **A4-post** verifies the branch's expected bytes.
6. **A8-post**: a freshly materialized tech-lead context carries the new rule exactly once and the old sentence zero times.

Steps 2 and 3 must both precede step 4, and step 3 should be applied close to it so the M2 window stays short.

**Rollback, corrected (grinch blocker 1, partially upheld).** Round 2 called this "a one-line restore". That was **wrong**, and the correction matters: re-adding the canonical line does **not** repair a context that was already materialized. Guidance is copied into a managed context at session create/restart (`commands/session.rs:1405-1468`, `:2658-2683`, `:2820-2838`); a remote `/clear` does not rematerialize it (`phone/mailbox.rs:3393-3442`). So:

- restoring `_agent_tech-lead/Role.md:7` fixes every **future** launch, immediately;
- a context materialized inside the window keeps its state until that session is restarted, and a restart cures it.

The grants need no rollback: they state authority their holders already exercise.

**Accepted risk: the window survivor. The matrix-wide context scan is rejected as disproportionate (grinch blocker 1).**

Grinch is right on the mechanism: a tech-lead launched between M2 and deploy has neither boundary, and that context **persists** for the life of the session, because deploying the binary does not rewrite an already-materialized file. Its remedy is an invariant scan over *every* materialized tech-lead context matrix-wide, rematerializing or restarting each one that holds "both" or "neither".

The remedy is rejected, and the reason is that **the survivor's state is not a novel harm; it is the status quo, persisting for one session**:

| | the 8 of 9 coordinators today (E13) | a window survivor |
|---|---|---|
| boundary in its materialized context | none | none |
| enforcement (`can_communicate`) | unchanged | unchanged |
| acquires the rule when | its next post-deploy launch | its next post-deploy launch |

The two states are identical, and the second is strictly rarer. A control that scans and rematerializes every managed context across every workgroup, to cure a state that 8 of 9 coordinator identities are in **right now**, unremediated and accepted, costs more than the harm it prevents. By its own logic it would have to run today, against those 8, and nobody proposes that. The one asymmetry, that a survivor *lost* a boundary it previously had while the 8 never had one, is not behavioural: an agent reads the context it was given, and does not recall what a prior session's context said.

**What is accepted, precisely.** A tech-lead session launched in the M2-to-deploy window operates with no cross-workgroup boundary statement until it is next restarted.

**What it costs if it fires.** That session may initiate cross-workgroup contact without role authority. It cannot reach another workgroup's *members* of a different team (E6 denies it in code), and it cannot deadlock, because the failure mode of a missing rule is permissiveness, not contradiction. Enforcement is untouched throughout.

**What makes it cheap to avoid anyway, at no operator burden:** the window is bounded by the operator's own step 3-to-4 gap, and any restart cures a survivor. If a survivor is suspected, restarting that session is the whole cure and needs no scan.

This is a judgement about proportion, recorded as such rather than closed by weakening a gate: A8-post still hard-stops on a **contradictory** context, which is the state that actually deadlocks. Only the matrix-wide sweep for the *permissive* state is declined.

**Standing rule, which outlives this plan:** a coordinator that initiates a peer message outside its own workgroup needs an M1-style grant in its own `Role.md` before it next runs, because a silent role defaults to not-authorized under the new rule. Re-run the criterion when adding a coordinator or a team. A roster copied into this file would go stale and would publish it. This plan changes no role file itself.

**6.4 Failure behavior**

Unchanged and already correct: `sync_project_context_template_for_read` returns `Err` on an unreadable or non-regular file, and session context generation fails with a path-specific error rather than silently dropping the customization. State persistence is best-effort (`persist_state_best_effort`): a read-only `.ac` degrades to re-evaluating the same decision next launch, never to a wrong overwrite.

## 7. Compatibility and security impact

- **Compatibility.** No IPC, type, or schema change (E11); `STATE_SCHEMA_VERSION` stays 1. The `current_version` bump to 4 is metadata: it is persisted into the state entry and surfaced as `currentDefaultVersion`, and it does **not** drive the update decision, which is sha256 plus `is_known_generated`. The bump follows the convention pinned for the global at `:1593-1596`. Older builds reading a v4 body see an unrecognized-but-valid template and preserve it.
- **User edits.** No customized body is ever overwritten without an explicit user action, and that action backs up first.
- **Token cost.** The coordinator body grows 2296 -> 2509 bytes (+213, about +53 tokens) for coordinator sessions only. This is a deliberate cost against the #1005 minimization program, accepted by the issue's Decision section as the price of preventing failed cross-workgroup attempts.
- **Security.** No change to the enforced boundary: `can_communicate` remains the hard gate and is untouched. The prose is strictly narrower than the code (E7), so it cannot widen reach. **The reply carve-out is guidance, not a security property, and this plan makes no safety claim for it** (grinch finding 4, verified: the message model has no `reply_to`, thread, causal, or expiry field, `phone/types.rs:7-57`). Nothing mechanically distinguishes a reply from an initiation, so a prior inbound message can in principle be treated as standing authorization, and an unauthorized initiator can manufacture the condition that authorizes a response. The intended reading is recorded in 4.1: a direct response to the same visible inbound sender, not a new delegation or relay. The whole rule is guidance inside an editable project file and is not a security control; anything that must be enforced belongs in `can_communicate` (#330, #1041).

## 8. Implementation order

**One commit, both Rust files together.** This is a correctness requirement, not tidiness.

1. Copy the v3 literal verbatim out of `get_default_coordinator_template` into the new frozen const (5.2); add it to `is_known_generated_coordinator_template`; bump the spec to `current_version: 4`.
2. Insert the 4.1 bullet into `get_default_coordinator_template` (5.1).
3. Add the tests in section 9.
4. Fix the doc (5.3).

**Never land step 2 before step 1.** If the default changes while the recognizer still lacks v3, every pristine v3 body is classified as customized and surfaced as a pending update (measured: 10.2 P1). `mark_observed` alone is recoverable. A user who answers that spurious prompt with "keep custom" triggers `mark_ignored`, which records the (v3, v4) pair, and from there the outcome **splits by population** (measured: 10.2 P2 and P3). The one-commit sequencing closes both halves, so the requirement below is unchanged, but the reason is not the one this section originally gave:

- **No trusted state entry** (E4 row 4): the skip is **unrecoverable by code**. `mark_observed` reaches `entry_mut` (`:823`), which creates the entry with `last_seeded_sha256: None`. That permanently disqualifies the row-3 auto-update, which requires `last_seeded_sha256` to equal the file hash, and it simultaneously makes `has_valid_entry` true, which permanently disqualifies row 4. The dismissal branch at `:815-821` is then the only reachable match, and a later commit adding v3 to the recognizer does **not** undo it. The dismissal is keyed on the (v3, v4) pair, so it holds for as long as v4 is the shipped default; escaping it needs a further default change, or the user deleting the file or the state entry by hand.
- **Trusted entry whose `last_seeded_sha256` equals the on-disk v3 hash** (E4 row 3, which is what E3 measures in this workspace): the skip is **recoverable**. Neither `mark_observed` (`:253-263`) nor `mark_ignored` (`:265-280`) clears `last_seeded_sha256`, and the row-3 auto-update at `:787-797` is evaluated **before** the dismissal branch at `:815-821`, so a later freeze commit re-enters auto-update and overrides the dismissal.

Only the coordinator is exposed to the unrecoverable variant, and its own spec is why: `suppress_unknown_without_state: false` (`:303`) is what lets a stateless unknown fall through to `mark_observed` and acquire the poisoned entry. The global's `true` (`:293`) short-circuits at `:807-813` before any entry exists, so a stateless global body can always be rescued by a later freeze. The coordinator has no such protection. That is what makes the one-commit rule non-negotiable for this template specifically, and it is a stronger reason than the one originally stated, not a weaker one.

5. Verify (section 9).

**Section 8 is the order of the repo commit only.** It does not place the matrix work: 6.3 does, and 6.3 puts A4-pre, M1 and M2 **before** this commit is deployed. Round 2 of this plan ended this section with "hand off to the user or Root for the ordered matrix removal and audit", which read as though the matrix actions followed the commit. They do not, and that sentence is removed rather than patched. The repo commit and the matrix rollout are independent sequences joined at one point: **the deploy, which both A7 and A8-pre gate.**

## 9. Tests and acceptance criteria

New tests in `seeded_context_templates.rs`, mirroring the named precedents:

- **T1** `coordinator_pre_cross_workgroup_snapshot_is_byte_exact` (mirrors `coordinator_pre_token_minimization_snapshot_is_byte_exact:1485`): assert the frozen const's `.len() == 2296` and `hash_text(...) == "9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84"`. Pins E2's externally captured values, never the const against itself.
- **T2** `read_sync_updates_pristine_v3_coordinator_template` (mirrors `read_sync_updates_old_generated_coordinator_template:1631`): write the frozen v3 const to a temp `.ac/Context.coordinator.md` with no state file, run `sync_project_context_template_for_read`, assert the file now equals `get_default_coordinator_template()`. Covers E4 row 4.
- **T3** `read_sync_updates_seeded_v3_coordinator_and_bumps_version` (mirrors `read_sync_updates_pre_token_minimization_global_template:1541`, whose version assertion is at `:1593-1596`): write the frozen v3 const **and** a state file with `templateId: "coordinator"`, `currentVersion: 3`, `lastSeededSha256` = the v3 hash; sync; assert the file equals the current default and the persisted `templates.coordinator.currentVersion == 4` with `lastSeededSha256` = the new default hash. This is the branch the live workspace is in (E3), so it is the test that actually proves the migration.
- **T4** `coordinator_template_carries_cross_workgroup_rule` in `session_context.rs` (beside `:6255` / `:6274`). **Rewritten: grinch finding 6.** The original specified two disjoint `contains` fragments, which is exactly the failure this plan exists to prevent: a template that keeps the routing fragment and the reply fragment but **drops the entire `only when your role, the user, or the Root Agent authorizes it` clause between them** passes it, while violating the user-approved rule. The load-bearing qualifier is precisely the part a fragment test cannot see. T4 must therefore assert the **complete bullet as one exact string, occurring exactly once**:

  ```rust
  const RULE: &str = "- To reach another workgroup, message its coordinator, never its members, and only when your role, the user, or the Root Agent authorizes it; replying to a coordinator who messaged you first is always authorized.\n";
  let tpl = get_default_coordinator_template();
  assert_eq!(tpl.matches(RULE).count(), 1, "the exact approved bullet must appear exactly once");
  assert!(!tpl.contains('\u{2014}'), "coordinator template must stay em-dash-free");
  assert!(!get_default_agent_template().contains(RULE), "the rule is coordinator-only");
  ```

  **Corrected at round 3 (grinch blocker 6).** Round 2 claimed `matches(RULE).count() == 1` rejects "a duplicated or a contradictory second variant". It rejects an exact **duplicate**, but **not** a contradictory variant: a template holding the approved bullet once **plus** a second, differently worded `- To reach another workgroup, ...` bullet still counts exactly one match and passes. A4's fixed hash would eventually catch it, but only at the live-deploy gate, which is far too late for a defect the unit suite is supposed to hold. T4 therefore adds a looser uniqueness assertion on the anchor:

  ```rust
  assert_eq!(
      tpl.matches("- To reach another workgroup,").count(), 1,
      "exactly one cross-workgroup bullet may exist; a second variant contradicts the approved rule"
  );
  ```

  `RULE` is the 4.1 text verbatim including its leading `- ` and trailing `\n`; it is the same 213 bytes 5.1 inserts, so T4 and 5.1 cannot drift apart. The pair is what makes T4 non-vacuous: the exact assertion pins *the approved wording including the authorizer clause*, and the anchor assertion pins *that no rival wording exists*.

**T2 and T3 must not be vacuous, and as specified they are.** Both end in `assert_eq!(file, get_default_coordinator_template())` after a sync. If step 2 of section 8 is skipped, so the const is frozen but the template is never edited, then the frozen v3 **is** the current default: `sync_one_template` returns at the row-2 no-op (`:781-784`) and the assertion holds while proving nothing. The named precedent guards exactly this, and T2 must inherit its two opening assertions (`read_sync_updates_pre_token_minimization_global_template:1552-1561`):

```rust
assert_ne!(
    COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE,
    get_default_coordinator_template(),
    "the v4 edit must actually change the template or the freeze is pointless"
);
assert!(
    is_known_generated_coordinator_template(
        COORDINATOR_CONTEXT_TEMPLATE_BEFORE_CROSS_WORKGROUP_RULE
    ),
    "the recognizer must accept the frozen v3 bytes"
);
```

The `assert_ne!` is the only thing in the suite that fails if the freeze lands without the template edit; the `assert!` is the only thing that fails if the const lands without being wired into `is_known_generated_coordinator_template` at `:363`. Neither is implied by T1, which only pins the const's own bytes.

Regression, must still pass unmodified: `read_sync_updates_old_generated_coordinator_template` (v1 still recognized), `custom_coordinator_is_preserved_and_reported`, `overwrite_creates_backup_and_writes_default`, `coordinator_pre_token_minimization_snapshot_is_byte_exact`, `coordinator_template_no_longer_carries_inline_self_maintenance`.

**Acceptance criteria (objective)**

- **A1** `cargo test --lib config::seeded_context_templates` green, 34 pre-existing tests plus T1..T3, none modified.
- **A2** `cargo test --lib config::session_context` green, including T4.
- **A3** Formatting, restated because the original criterion is not reachable (measured: 10.3). `seeded_context_templates.rs` **is** rustfmt-clean at base and must stay clean. `session_context.rs` is **not**: it carries **23** pre-existing rustfmt hunks at `4acadfe5`, so "fmt clean" cannot be an acceptance criterion for it. The criterion is therefore **no new hunk**: run `rustfmt --check --edition 2021 <file>` per file and compare the hunk list against the baseline list in 10.3. Do **not** run `cargo fmt` across the crate; it would rewrite those 23 unrelated regions in this file alone and breach A6. `cargo clippy` introduces no new warning in either file.
- **A4** Live migration, measured, not asserted by eye. **Strengthened (grinch finding 6): the original compared the live file to "the new default", which is circular** because it holds whenever the file matches whatever was implemented, right or wrong. A4 compares against the **externally predicted** value from 5.1, derived before implementation and independently re-derived by dev-rust (10.1). **Split into two branches at round 3 (grinch blocker 5): round 2 offered "merge the rule by hand" while also demanding the file re-measure as pristine v3 and then as the fixed v4 hash, which a hand-merged body can never satisfy.** A4-pre runs **first in the rollout**, before M1 and M2, because it decides which branch applies and whether removing M2 is safe at all.
  - **A4-pre (before any matrix action).** Measure `.ac/Context.coordinator.md` and its state entry. Exactly one branch is then selected and recorded:
    - **Branch (a), pristine:** the file is 2296 bytes / `9f72fa83...` with state `currentVersion: 3` (E3, still true as of this plan). Proceed on the fixed-hash path.
    - **Branch (b), customized or dismissed:** anything else. Choose one, explicitly, and record it: **(b1)** restore the pristine v3 body, backing up the customization first, and fall back to branch (a); or **(b2)** keep the customization and merge the complete 4.1 bullet into it by hand, exactly once. Branch (b2) is a **deliberate opt-out of the auto-update path**: the seeded-state and fixed-default-hash assertions below do **not** apply to it and must not be forced, because a customized body is by definition not the default. What (b2) must still satisfy is the boundary requirement itself, which is the last two checks.
  - **A4-post, branch (a) only:** the file's sha256 is exactly `f6ef7894b9f0f606e945c282d769144e96487fcc01ab435c9aab8019bb3ce1f6`, and `.agentscommander-context-templates.json` shows `currentVersion: 4` with `lastSeededSha256` equal to that hash.
  - **A4-post, both branches:** the complete 4.1 bullet occurs **exactly once** in the body, and the looser anchor `"- To reach another workgroup,"` also occurs **exactly once** (blocker 6: the exact-string count alone accepts a second, differently worded variant alongside the approved one).
  - **A4-post, both branches, the check that actually matters:** a **freshly materialized** coordinator context carries the complete bullet exactly once, and the loose anchor exactly once. The template on disk is an input; what reaches the agent is the artifact this issue is about, and only this covers the append path at `session_context.rs:1844-1855`.
- **A5** (strengthened: grinch finding 6, the negative check was vacuous). `docs/agents/inter-agent-messaging.md` contains no occurrence of "only via the Root Agent" **and** contains the exact Coordinator row from 5.3, byte for byte, exactly once; the Worker and Root Agent rows are byte-unchanged from base `4acadfe5`; and the added note references **#1041** and labels the behavior a deviation or defect, not intended behavior. The old criterion passed after replacing the row with any text at all.
- **A6** No file outside `src-tauri/src/config/{session_context,seeded_context_templates}.rs`, `docs/agents/inter-agent-messaging.md`, and this plan is modified. In particular `teams.rs` is untouched.
- **A7 (matrix gate, blocks deploy; grinch finding 3, rewritten at round 3 for blocker 3).** A1-A6 are repo checks: every one passes with M1 skipped, applied to the wrong file, or applied to only some of the coordinators, and the result is that a cross-workgroup initiator is silently disabled the moment its next context materializes. A7 closes that objectively. Round 2's version gated on a **row count**, which was unsound: the enumeration emits one row per team configuration, and two coordinators own more than one (E13), so three rows can be three grants on the wrong set, or the same coordinator counted three times.
  1. **Build a set of unique canonical `Role.md` paths**, not a count. Start from the 6.3 enumeration, map each qualifying coordinator to its canonical role path, and de-duplicate. Then apply the 6.3 predicate to **every remaining** coordinator and record the audit result explicitly, so "no further candidates" is a finding rather than an omission. **Require set size 3 only after that identity check**, and only for today's matrix; if the matrix changed, resolve the delta before deploy, not after.
  2. **Capture the pre-action bytes of each path.** After the grant, the file must differ from its captured bytes by **exactly the inserted paragraph and nothing else** (compare mechanically; do not eyeball). This mirrors what A8-pre already requires of the tech-lead role, and it is what stops a grant from replacing or damaging unrelated role instructions while still "containing the paragraph once".
  3. For **each** path, the tailored grant paragraph written for that role (6.3) occurs **exactly once** in the canonical file. Zero means skipped; more than one means a duplicated or conflicting grant. The paragraph is per-file by construction, so this is checked against the text actually written, not against one shared string.
  4. For **each** path, a **freshly materialized** role context carries that same paragraph exactly once. Step 3 proves the canonical file; only this proves what the agent receives, and a replica materialized earlier will not have it.
- **A8 (matrix gate, two halves; grinch finding 3).**
  - **A8-pre, blocks deploy:** `_agent_tech-lead/Role.md` contains the M2 sentence **zero** times, and the file is otherwise byte-unchanged (only that line removed).
  - **A8-post, after A4:** a **freshly materialized** tech-lead context contains the complete 4.1 bullet exactly once and the old M2 sentence zero times. If the old sentence is still present, a stale canonical file or a cached context is in play, and tech-lead is in the contradictory state Retraction 1 describes: **hard stop and resolve before releasing tech-lead work.**
  - If A4 fails, execute the 6.3 rollback (re-add the M2 line verbatim) rather than leaving tech-lead with neither boundary.

---

## 10. Dev-rust enrichment (Step 5)

Added by dev-rust. Everything here was measured at base `4acadfe5` and re-derived rather than inherited from section 2. No certification verdict is recorded here; that is architect's at Step 7.

### 10.1 Provenance, re-derived independently

**E2's numbers are confirmed.** Re-derived from scratch by two methods that share no code, both driven off a mechanical extraction of the accessor from `git show 4acadfe5:src-tauri/src/config/session_context.rs` (nothing hand-copied):

- **M1**, the same shape as E2: slice the accessor out, append a `main` that calls `std::fs::write(out, get_default_coordinator_template())`, compile with `rustc --edition 2021 -O` (rustc 1.93.1), hash the file. `std::fs::write` rather than `print!` keeps it byte-exact on Windows.
- **M2**, independent of rustc: parse the Rust string literal in Python, implementing the escape set including the line-continuation escape, and hash the result.

| Version | Method | len | sha256 |
|---|---|---|---|
| v3 @ `4acadfe5` | M1 | 2296 | `9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84` |
| v3 @ `4acadfe5` | M2 | 2296 | `9f72fa83ac2fafc73565f975a2bec936a09d0e6a410b1ee1a4a13952e694ec84` |
| v2 @ `1dd0b58` | M1 | 2403 | `92f3abfc108147b07f1c4a49e7062c0f4d0d9aae570b7e5195852c31bb8b0d02` |
| v2 @ `1dd0b58` | M2 | 2403 | `92f3abfc108147b07f1c4a49e7062c0f4d0d9aae570b7e5195852c31bb8b0d02` |

M1 and M2 outputs are byte-identical under `cmp` for both versions, and both reproduce the v2 values this repo already pins independently at `seeded_context_templates.rs:79-81`. v3 is pure ASCII, LF only, no CR, no U+2014. `git show` was checked for eol contamination first: `core.autocrlf` is `true` here, but the blob came out with zero CR bytes, so the extraction is the raw blob.

E3 re-measured on disk, unchanged: 2296 / `9f72fa83...`, zero occurrences of "workgroup", state entry `currentVersion: 3` with `lastSeededSha256` equal to the file hash and every `ignored*` field null. E1, E6, E7, E9, E11 and E12 spot-checked and accurate as written, including the 41-file role scan and the base test counts (34 in `config::seeded_context_templates`, 132 passing plus 2 ignored in `config::session_context`).

**5.1's prediction is confirmed by construction, not by arithmetic.** Inserting the 4.1 bullet into the *derived* v3 bytes at 5.1's placement yields exactly **2509** / `f6ef7894b9f0f606e945c282d769144e96487fcc01ab435c9aab8019bb3ce1f6`. The anchor bullet is unique in the body, so the placement is unambiguous. One clarification for the implementer: the "213 bytes" in 4.1 is the bullet plus its trailing LF; the sentence alone is 212 bytes.

### 10.2 The freeze hazard: confirmed, mechanism corrected in section 8

E5 is real and section 8's sequencing does close it, so **the decided solution stands and needs no change**. What was wrong is the stated reason, which section 8 now carries corrected.

Measured with a temporary probe: four tests driving `sync_one_template` through a synthetic `SeededContextTemplateSpec` whose `current_content` is a stand-in v4 and whose `is_known_generated` either includes or omits the stand-in v3, mirroring `sync_project_context_template_for_read:962-974` and the tail of `dismiss_context_template_update:1304-1305`. Run under `cargo test --lib`, all four green, then reverted; the repo was left clean.

| Probe | Setup | Result |
|---|---|---|
| P0 | freeze applied, pristine v3, both populations | auto-updates to v4, `currentVersion` persisted as 4 |
| P1 | freeze **missing**, pristine v3, both populations | **stranded at v3**: E5 confirmed |
| P2 | freeze missing, then dismissal, then a later freeze commit, **trusted entry** (E3 / E4 row 3) | **recovers**: the file becomes v4 |
| P3 | freeze missing, then dismissal, then a later freeze commit, **no state entry** (E4 row 4) | **stays v3**: unrecoverable |

P2 is the counterexample to section 8's original wording, and P3 is the case that actually justifies it. P0 additionally confirms T3 is implementable exactly as specified: the version bump is persisted through the auto-update path in both populations.

The probe is faithful to production on the point that matters. `scan_project_context_template_updates:938-960`, which is what surfaces the prompt, does not use `compute_pending_update`; it calls the same `sync_one_template` with `return_pending: true`, and `mark_observed` at `:823` runs before the `return_pending` split. So the state mutations the probe exercises are the ones the real prompt path performs.

**Checked and cleared, so the next reviewer need not re-spend it:** `compute_pending_update:848-887` is a second copy of the decision order and it is missing the row-3 branch, which looks at first like a divergence that would fire a spurious prompt at the pristine-v3 trusted-entry state. It cannot: the only callers are `validate_expected_hashes` (the accept/dismiss precondition check), which runs only against an update a scan already produced, and the scan itself goes through `sync_one_template`. Pre-existing either way, and out of scope for #1030.

### 10.3 Notes for the implementer

- **The `\n\` continuation makes indentation non-load-bearing.** Rust's string-continuation escape eats the newline and the next line's leading whitespace, and every line of this literal ends in `\n\`. So moving the v3 literal verbatim from inside the accessor (`:2139`) to a module-level const produces byte-identical output regardless of indent. The existing v2 const at `:73-109` already uses the same 4-then-5 space shape as the accessor, so a verbatim copy is also a clean textual match. Both derivations above confirm this: M2 models the escape explicitly and agrees with rustc byte for byte.
- **`rustfmt` will not touch the literal.** `format_strings` is `false` by default, and the accessor region `:2139-2160` is rustfmt-stable at base. The 23 pre-existing hunks in `session_context.rs` are at lines 1888, 2016, 3543, 3886, 4452, 4531, 4545, 4619, 4632, 4677, 4726, 6296, 6305, 6332, 6354, 7992, 8071, 8174, 8239, 8265, 8277, 8293, 8326. None is in the accessor, and none is in T4's insertion site near `:6255-6292`, though 6296 is adjacent. This list is the A3 baseline.
- **The coordinator has exactly one recognizer, unlike the global.** `is_known_generated_coordinator_template` (`:363-367`) is the only function that byte-compares the coordinator default, and `project_specs():301` is its only use. The global has two (`is_known_generated_global_template` and `is_known_generated_standalone_global_template`), which is why #1005 S6's test comment warns that missing either strands installs silently. That warning does not generalize here: the freeze is a single-site change and there is no second recognizer to forget.
- **The version bump is inert with respect to the migration.** `entry_mut` (`:227-232`) writes `spec.current_version` on every `mark_*`, so the bump lands through whichever branch fires, and the update decision is sha256 plus the recognizer. Forgetting the bump would leave `currentDefaultVersion` stale in the UI and would not block the auto-update. It is still required by 5.2; it is simply not the thing that makes migration work, and it must not be mistaken for a guard.
- **No test pins the live default's length or hash** (E12 verified), so the template edit breaks nothing by construction. `token_accounting_report:8264` is `#[ignore]`d and only prints `.len()`.

---

## Rulings applied (tech-lead, 2026-07-16)

Both items previously open are now decided. No open questions remain.

1. **Reply carve-out: APPROVED by the user.** The 4.1 sentence is final and must not be reworded. Rationale retained in 4.1 because it is the reason the wording departs from the issue body.
2. **`_agent_project-director` promoted from audit to mandatory rollout step M1** (6.3), ordered **before** deploy. My independent re-read of the full role file supports the ruling: the role contains no authorization language at all, so it defaults to not-authorized the moment the body lands, and the carve-out cannot save an initiator.
3. **The 5.3 doc note is framed as a defect tracked in #1041**, never as intended behavior. #1041's body carries the reciprocal obligation to delete the note when the mechanism is fixed.

Unchanged by the rulings, and re-confirmed: the one-commit freeze-plus-template sequencing with the `mark_ignored` permanent-skip hazard called out for dev-rust (section 8); `_agent_shipper/Role.md:48,53` untouched; `docs/agent-matrix-conventions.md` untouched.

## Matrix names in this plan

`plans/` is tracked and this repository is public, so anything written here is published on landing and a later deletion does not un-publish it. Private Agent Matrix contents (agent names, team names, internal project names) are therefore not enumerated here. Where the matrix is evidence, this plan gives the command to enumerate it from disk instead; the audit stays fully actionable and the list stays current by construction.

Three names are retained deliberately, none silently:

| Name | Why retained | Already public? |
|---|---|---|
| `_agent_project-director` | Tech-lead ruling: M1 is a concrete required action on a specific file and cannot be expressed as a method. Generic name. | No. This plan is the first tracked file to carry it, and the ruling was made with that stated. |
| `_agent_tech-lead` | M2 is a concrete required action on a specific file, the same rationale as M1. Also already named in the public issue #1030's Decision section, so the plan discloses nothing new. | Yes: 39 tracked files. |
| `_agent_shipper` | E9 out-of-scope ruling: naming the one false positive is what makes "exactly one restatement exists" verifiable. | Yes: 10 tracked files. |

Removed and replaced with an enumeration method: the E10 and 6.3 coordinator rosters, the acceptance-testing coordinator's name, and the team-config filename that evidenced project-director's coordinator status.

**Also unpublished, and load-bearing: the two criterion-selected M1 targets** found in E10a. Both are private (one competitive, one a private benchmark), and by tech-lead ruling they stay out of this plan. That costs nothing operationally: 6.3's criterion selects them mechanically from the matrix on disk, A7 builds the identity set locally and gates on it, and their tailored grant text is written against the private role by whoever runs the rollout. This plan specifies the rule those grants must satisfy, not their text.

The certification verdict is at the end of this plan.

## Grinch Review

Added at Full-path Step 6. This is adversarial enrichment, not a certification verdict.

### Corrected section 8 verdict

The P2/P3 correction is mechanically sound, subject to one precision note. With a trusted v3 entry, `last_seeded_sha256` survives both `mark_observed` and `mark_ignored`, and the row-3 generated-template update is evaluated before the ignored-pair branch; P2 therefore recovers when a later build recognizes v3. Starting without an entry, the first unknown-template scan creates an entry whose `last_seeded_sha256` is `None`; after dismissal, that entry blocks both row 3 and row 4 and the ignored-pair branch wins; P3 therefore remains on v3 while the same default and valid state entry remain in place. This supports the one-commit requirement and does not force a change to the freeze-plus-template solution.

The phrase "unrecoverable by code" is slightly too absolute. Invalid JSON is loaded as a trusted empty map (`load_state:520-533`), and an unsupported schema is loaded without a valid entry (`:537-554`); after either external state change, a later freeze can take row 4 and recover. Section 8 should call P3 a stable fixed point **while the dismissed state entry remains valid and v4 remains the default**, rather than imply that no code path can ever recover it.

1. **What:** The one-file M1 conclusion is not supported by a topology-aware coordinator audit. E10 enumerates coordinator roles and reads their prose, but 6.3 does not join those roles to team membership before deciding whether an outbound peer workflow stays inside the workgroup. Running that joined audit exposes current singleton-coordinator workflows that the role-only conclusion does not account for. The private names are deliberately not recorded here.

   **Why:** For a coordinator with no same-workgroup peer, a role-mandated outbound peer workflow necessarily initiates cross-workgroup contact. The reply carve-out cannot help an initiator. Under 6.3's own standard that a defining duty without explicit cross-workgroup authorization is insufficient, such a coordinator is stranded exactly like the M1 case, so "No other matrix coordinator needs a grant" can be false even though a role never says "workgroup".

   **Fix:** Before certification, re-run the audit by joining every team config's coordinator and member count, then inspect every outbound contact/dispatch duty under one consistent definition of "the role authorizes it." Use a local command such as the following; publish the method, never its output. Any additional initiator needs an M1-style pre-deploy grant, or a concrete explanation of the existing sentence that already grants cross-workgroup initiation. Because adding another matrix name is ruled out, ask before naming any resulting file in this public plan.

   ```powershell
   Get-ChildItem -Directory -Filter '_team_*' | ForEach-Object {
       $configPath = Join-Path $_.FullName 'config.json'
       if (Test-Path -LiteralPath $configPath) {
           $config = Get-Content -Raw -LiteralPath $configPath | ConvertFrom-Json
           [pscustomobject]@{
               Config = $configPath
               Coordinator = $config.coordinator
               Members = @($config.agents).Count
           }
       }
   }
   ```

2. **What:** M2 is not harmless when delayed, and the stated rollout order creates a contradictory-context window. The old tech-lead sentence forbids every outside-workgroup message unless the user or Root Agent explicitly directs it; the new shared sentence says a reply to the initiating coordinator is always authorized. Those are not redundant instructions.

   **Why:** A tech-lead context materialized after A4 updates the shared body but before M2 is removed contains both rules, while the coordinator body also says to keep the base role. A tech-lead receiving a cross-workgroup dispatch can therefore refuse the required reply or deadlock on which instruction controls. If the unnamed coordinator in rollout step 4 is a tech-lead, the rollout creates the contradiction immediately; otherwise any tech-lead start, restart, or context clear in the A4-to-M2 interval does it.

   **Fix:** Make the interval a launch hold: after the v4 body can be written, no tech-lead session may start, restart, or clear until M2 is complete. Use the already-M1-granted `_agent_project-director` session to trigger and verify A4, remove M2 immediately after the exact-body check, and only then release tech-lead sessions. If A4 fails, keep M2 and stop the rollout. Replace the claims that M2 is merely redundant and can be left indefinitely.

3. **What:** None of A1-A6 gates the two mandatory matrix actions. The repo tests and doc checks can all pass if M1 is skipped, applied to the wrong file, or only partly applied, and they can pass if M2 is skipped. A4 also assumes the E3 body remains pristine until deployment but gives no failure branch for a later customization or dismissal.

   **Why:** Skipped M1 disables the project-director's defining outbound workflow as soon as its next v4 context is materialized. Removing M2 after a customized body is preserved leaves tech-leads with neither the old boundary nor the new one. A user can therefore satisfy the commit's acceptance criteria and still deploy the broken operational state that 6.3 calls mandatory.

   **Fix:** Add explicit operator release gates: before deploy, verify the exact M1 grant occurs once in the canonical role and in a freshly materialized role context; before M2, require the fixed v4 hash, the complete final bullet exactly once, version 4 state, and a freshly materialized coordinator context carrying the bullet; after M2, require the old sentence to occur zero times in a fresh tech-lead context while the shared rule still occurs once. A mismatch or customized/ignored body is a hard stop: leave M2 in place until the user accepts or manually merges the new rule and the checks pass.

4. **What:** Sections 4.1 and 7 overclaim that the reply carve-out cannot be chained into unauthorized dispatch. The message model has an ID, sender, recipient, body, and timestamp, but no `reply_to`, thread, causal parent, expiry, or message-purpose field (`phone/types.rs:7-57`); the delivered notification body carries only a path, and its wrapper adds the sender but no causal link (`cli/send.rs:436-468`).

   **Why:** The system cannot distinguish a direct answer from a new request, a relay to a third workgroup, or unrelated contact long after an earlier inbound message. After a context clear, the agent may also lack the history needed to decide who "messaged you first." Thus a prior inbound message can be treated as indefinite authorization, and an unauthorized initiator can create the very condition that authorizes the recipient's response. This is guidance, not a causal or security boundary.

   **Fix:** Keep the user-approved bullet verbatim, but remove the categorical non-chainability/security claims. Record the accepted operational interpretation: the exception covers a direct response to the same visible inbound sender, not a new delegation or third-party relay; if the inbound evidence is unavailable after context loss, the exception is not established. If stronger semantics are required, track reply/thread state as a separate enforcement change rather than claiming #1030 implements it.

5. **What:** The proposed Coordinator documentation row is still incomplete. `send.rs:356-369` special-cases direct coordinator-to-Root-Agent delivery before the `can_communicate` check at `:370-380`; E6's statement that `can_communicate` is the CLI gate therefore has a Root-target exception.

   **Why:** The table is an allowed-recipient table. Saying only "Any team member; any other coordinator directly" omits a real direct recipient and hides the channel a coordinator can use to reach an authorizer. "No Root Agent relay" is correct for coordinator-to-coordinator traffic, but it does not mean the Root Agent is unreachable.

   **Fix:** Include direct Root Agent contact in the Coordinator row while retaining that other coordinators need no Root relay. Correct E6's gate description and make A5 assert the complete required row, not only the absence of the old phrase.

6. **What:** T4 and A5 retain vacuous success paths. T4 searches for two disjoint fragments, so an implementation that drops the entire authorizer clause between them still passes. It also passes with duplicated or contradictory variants. A5 passes after replacing the old row with any text at all, provided the #1041 note remains. A4's "equals the new default" comparison is circular if the implemented default is wrong.

   **Why:** For example, a template containing the first routing fragment and the reply fragment but omitting "only when your role, the user, or the Root Agent authorizes it" satisfies T4 while violating the user-approved rule. A wrong doc row and wrong live default can likewise satisfy the negative/circular checks.

   **Fix:** Make T4 assert the complete final bullet as one exact string and assert its occurrence count is one. Make A5 assert the exact final Coordinator row (including finding 5) and the unchanged Worker/Root rows. Make A4 compare the live file to the externally predicted fixed sha256 `f6ef7894b9f0f606e945c282d769144e96487fcc01ab435c9aab8019bb3ce1f6`, assert the complete bullet once, and inspect the materialized coordinator context rather than only the source template and its self-consistent hash.

---

## Step 7 consensus: architect verdict

Round 1 verdict was `NEEDS_ANOTHER_ROUND`, on the grounds that I would not self-certify an unreviewed rewrite of the reasoning that had already failed twice. Grinch reviewed it (round 2), conceded the ordering dissent with code, and raised six more. All six are resolved below. Round 3 is final.

# READY_FOR_IMPLEMENTATION

This restates the plan's `Status:` field at line 4, which is the contract's anchor. The two must never disagree.

## Round 3 blocker resolutions

| # | Blocker | Resolution |
|---|---|---|
| 1 | Window survivor persists; rollback incomplete | **Split.** The rollback overclaim is **upheld and fixed** (6.3: re-adding the line cures future launches only; a survivor is cured by restart). The matrix-wide XOR scan is **rejected as disproportionate**, with the risk accepted and costed in 6.3. Evidence below. |
| 2 | M1 predicate team-scoped, not workgroup-scoped | **Upheld.** Predicate rewritten to the destination class: an AgentsCommander **peer** message to a destination outside the sender's **workgroup**, same-team replicas included; user, Root and external contacts excluded. Singleton membership is demoted to evidence, not the test. |
| 3 | A7 gates a row count, not identity | **Upheld.** A7 now builds a de-duplicated set of unique canonical role paths, requires size 3 only after an explicit zero-additional-candidate audit, and requires each file to differ from its captured pre-action bytes by exactly the inserted paragraph. |
| 4 | The shared grant is a capability expansion | **Upheld with evidence.** Grants are now tailored per role under three rules, with the coordinator-only and never-members clauses invariant. Evidence below. |
| 5 | A4-pre misplaced; custom-body branch incoherent | **Upheld.** A4-pre is now step 1 of the rollout, ahead of M1 and M2. A4 has two explicit branches; (b2) is a recorded opt-out from the fixed-hash and seeded-state assertions, which a hand-merged body can never satisfy, while still having to prove the bullet in the body and in a fresh context. |
| 6 | T4 does not reject a contradictory variant | **Upheld.** T4 adds `matches("- To reach another workgroup,").count() == 1` beside the exact-bullet assertion; A4/A8 apply the same anchor check to materialized contexts. The claim that the exact count rejected variants was simply false. |

Also corrected: the **9 of 12 / 8 of 9** denominator (E13), and the affected-surfaces table, which still said the tech-lead line went after deploy and listed one M1 file instead of three.

## Evidence on blocker 4 (capability expansion)

Upheld; I do not dissent. The generic paragraph authorized "contact **any** workgroup's coordinator ... to dispatch work, follow up, or collect status". Read against the two singleton roles, that is strictly wider than the duty that made each qualify: one is chartered to run a defined round protocol against a contender class, the other's protocol names a single counterpart holding the same role. Neither role authorizes general dispatch or status collection against arbitrary coordinators. Granting it would let a narrow specialist cite the paragraph for work its role never contemplated, and 6.3's claim that the grant "widens nothing" was false for two of three files.

The point is sharper than a scoping slip: this change exists to make authority explicit and precisely bounded, so a rollout step that hands out broad authority to narrow roles reintroduces, by hand, the exact defect the plan removes. Tailoring costs nothing that matters, because the grant only ever has to defeat the not-authorized default **for the duty the role already has**.

## Evidence on blocker 1 (window survivor)

The mechanism is conceded: guidance is materialized at session create/restart (`commands/session.rs:1405-1468`, `:2658-2683`, `:2820-2838`), remote `/clear` does not rematerialize (`phone/mailbox.rs:3393-3442`), so a context launched in the M2-to-deploy window keeps neither boundary until that session restarts, and deploying does not repair it. My "one-line restore" rollback was wrong and is fixed.

The remedy is rejected because **the survivor's state is not novel**. Set the survivor beside the coordinators already running today (E13):

| | 8 of 9 coordinator identities, today | a window survivor |
|---|---|---|
| boundary in materialized context | none | none |
| `can_communicate` enforcement | unchanged | unchanged |
| acquires the rule at | next post-deploy launch | next post-deploy launch |

Identical, and the survivor is strictly rarer. A scan-and-rematerialize sweep over every managed context in every workgroup, to cure a state that 8 of 9 coordinator identities are in right now, unremediated and accepted, costs more than the harm it prevents; by its own logic it would have to run today against those 8, which nobody proposes. The one asymmetry, that a survivor lost a boundary it once had, is not behavioural: an agent reads the context it is given and does not recall a prior session's.

The accepted risk, its cost, and its free mitigation (restart cures a survivor) are recorded in 6.3 rather than left implicit. Note what is **not** relaxed: A8-post still hard-stops on a **contradictory** context. Only the sweep for the **permissive** state is declined, and permissiveness is the failure mode the 8 already live with.

## On proportionality

Asked, and answered honestly: **the plan is not the risk, but it is close to the ceiling of what this change can carry.**

The size splits in two, and only one half is burden. Roughly 380 of the lines are **record**: evidence, two dev-rust derivations, two grinch rounds, retractions. That is audit trail. It is why four wrong claims (the 11-of-12 count, the one-file M1, the redundant-M2 asymmetry, the non-chainability property) were caught before rollout rather than during it, and it costs the operator nothing.

The **instruction** the operator actually executes is 6.3's six-step order plus A1-A8: roughly 70 lines, for a rollout that touches three private role files, one deploy, and a template migration with a permanent-skip failure mode. That is proportionate. A one-bullet repo change with a three-file manual matrix tail is not a one-bullet change.

I have declined the two controls whose cost exceeded their harm (the post-A4 launch hold in round 1, the matrix-wide context sweep now) and said so both times with the reason. What remains is objective and cheap. If a further round added another control of that weight, I would expect to decline that too, and at that point the honest reading would be that the operational tail, not the plan, had outgrown the change.

## Standing, unchanged

The 4.1 sentence is user-approved and final, byte for byte; it is recorded as guidance under a narrow direct-response reading, never as a security property. Freeze-plus-template and sections 4, 5, 8 are stable and cleared by both reviewers. The two private role names stay out of this plan by tech-lead ruling: 6.3 selects them by criterion and A7 gates them by identity set. #1041 and #1008 remain out of scope.

**Certified for implementation.** dev-rust implements sections 5, 8 and 9; the user or Root executes 6.3 in the stated order, gated by A4-pre, A7 and A8.
