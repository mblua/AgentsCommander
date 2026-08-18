# Plan #1408: Document recommended RTK usage and per-agent-type command statistics

Author: architect, wg-17. Authored and certified in a single Lite pass on 2026-08-18 UTC.

Status: READY_FOR_IMPLEMENTATION

Issue: [mblua/AgentsCommander#1408](https://github.com/mblua/AgentsCommander/issues/1408), `docs: document recommended RTK usage and per-agent-type command statistics`.

This is a Lite documentation change. It adds one new markdown file and edits three passages of one existing markdown file. It touches no Rust, no TypeScript, no CSS, no configuration schema, no build script and no CI workflow. It adds no crate, no npm dependency, no module, no Tauri command, no IPC surface, no event and no migration. It adds zero module-to-module dependency arcs.

## 1. Frozen authority and fail-closed entry gate

The implementation working tree is `repo-AgentsCommander`, branch `docs/1408-rtk-usage-and-per-agent-stats`, targeting `main`.

After `git fetch origin main` on 2026-08-18 UTC, all of the following resolved exactly to `7b1e2852060c83152a29bbf4de71f5cb58c9e9e4`:

- committed `HEAD` of `docs/1408-rtk-usage-and-per-agent-stats`;
- `origin/main`; and
- `git merge-base HEAD origin/main`.

Root `.gitignore` line 11 ignores `/plans/`, so the implementation must force-add this exact plan file with `git add -f plans/1408-rtk-usage-and-per-agent-stats.md`. Do not remove or weaken the repository's `plans/` ignore rule.

Branch-name validation was checked against `scripts/validate-branch-name.mjs` line 15. The pattern accepts type `docs`. `docs/1408-rtk-usage-and-per-agent-stats` parses as type `docs`, number `1408`, slug `rtk-usage-and-per-agent-stats` (29 characters, under the 50-character `MAX_SLUG` cap), so the required `validate-branch-name` check will pass.

`git status --porcelain=v1 --untracked-files=all` at freeze time reported exactly one entry: `?? rtk-replica-history.db`. That stray file is a leftover RTK database in the repository root, unrelated to this change. **Do not commit it and do not add a `.gitignore` rule for it in this issue** (see section 8.3).

Immediately before implementation, fetch `origin/main` again. Stop for current-base review if `origin/main`, the local committed branch head, or their merge base no longer equals the frozen SHA above. Do not rebase, merge a moved base, or silently substitute a newer commit under this certification.

Every line number in this plan refers to the frozen SHA above. If a line number no longer matches the quoted text, stop and re-anchor on the quoted text, never on the number.

## 2. Objective and non-goals

Objective: give a reader who already runs RTK a single page that states the recommended `RTK_DB_PATH` value, explains why that value produces per-agent-type statistics, shows exactly how to read those statistics, and names the two ways the configuration fails.

Non-goals, binding on the implementer:

- **No change under `src/`, `src-tauri/`, `scripts/`, `.github/` or any non-markdown file.** The issue explicitly scopes this to documentation. In particular, do **not** add a placeholder hint or help button to the ENVIRONMENT editor in `src/sidebar/components/SettingsModal.tsx`.
- Do not rename the `## 5. Profile Path Placeholders` heading in `docs/agent-matrix-conventions.md`. Four documents link to its anchor `#5-profile-path-placeholders` (`docs/features/coding-agent-profiles.md:21`, `:120`, `:169` and `docs/features/config-seed.md:111`, `:177`), and `src/sidebar/components/SettingsModal.tsx:81` names the section by number. Renaming it breaks all of them.
- Do not add an RTK entry to `README.md`, `docs/troubleshooting.md`, `docs/glossary.md`, `docs/faq.md`, `CHANGELOG.md` or `docs/reference/settings.md`. Discoverability is handled by the single inbound link added in section 5.3.
- Do not create a `docs/features/rtk.md` companion page. RTK is not an AC feature.
- Do not add tests. There is no markdown linter, link checker or docs test in `.github/workflows/` (`bundle-validation.yml`, `cache-warm.yml`, `lockfile-check.yml`, `pr-regression-gates.yml`, `release.yml`, `validate-branch-name.yml`, `version-sync-check.yml`), and adding one is out of scope.
- Do not reword any part of `docs/agent-matrix-conventions.md` other than the three passages specified in section 5.
- Do not change the file name in the recommended value. It is `rtk-matrix-history.db`, not `rtk-replica-history.db`.

## 3. Verified current state

Verified by direct read at the frozen SHA, and by direct execution on the authoring workstation (rtk 0.42.4, Windows 11).

### 3.1 Repository facts

- `docs/integrations/` contains exactly `coding-agents.md`, `telegram.md` and `voice.md`. `docs/integrations/rtk.md` does not exist.
- `docs/integrations/telegram.md` and `docs/integrations/voice.md` both open with an H1, then a one-sentence "who reads this and why" line, and both close with a `## See also` list. That is the house shape this plan follows.
- `docs/style-guide.md` binds: lead with a concrete outcome, second person present tense active voice, one concept per H2, show the exact command **and** its expected first lines, name the exact error string, and avoid the banned word list (`revolutionary, unleash, supercharge, next-gen, AI-powered, game-changing, blazing-fast, seamless, magical, agentic`, plus `simply`, `just`, `easily`, `easy to use`).
- `docs/agent-matrix-conventions.md` section 5 is titled `## 5. Profile Path Placeholders`. Its blockquote reads "These tokens are used **inside** a profile cell's command or env." and its first body paragraph opens "Coding-agent profile command strings and `env` values may use a small set of `%...%` path placeholders...". Both self-limit the mechanism to profile cells.
- `docs/reference/settings.md:91` documents `envs` as `CodingAgentEnv[]`, "Environment rows applied at spawn", under the `### Coding agents` heading (anchor `#coding-agents`).

### 3.2 Backend facts that make the section 5 wording wrong

`src-tauri/src/config/agent_command.rs:839-851` gates placeholder-context construction on **three** separate value surfaces:

1. `command_tokens` (the effective launch command);
2. `agent.envs` rows with `enabled == true`; and
3. `profile_resolution.cell.env` values.

`collect_agent_env` (`agent_command.rs:367-391`) calls `expand_runtime_value` on each enabled `agent.envs` row (line 384). `collect_profile_env` (`agent_command.rs:393-419`) does the same for the profile cell (line 412). `agent.envs` is the Settings, Coding Agents, ENVIRONMENT list, which is **not** a profile cell. The section 5 text is therefore narrower than the implementation.

### 3.3 Placeholder and failure-mode facts

- `src-tauri/src/config/placeholders.rs:21-25` lists the recognized token set as exactly `%AC_REPLICA_ROOT%`, `%AC_WORKSPACE_ROOT%`, `%AC_MATRIX_ROOT%`.
- `placeholders.rs:31` defines `AC_MATRIX_ROOT_ERROR` as the literal string `%AC_MATRIX_ROOT% requires an AC workgroup replica launch root`.
- `reject_unexpanded_markers` (`placeholders.rs:202-214`) with `strict_path_value == false` returns `format!("{context}: unknown placeholder marker in value")` whenever `contains_percent_marker` (`placeholders.rs:226-248`) finds a `%WORD%` pair. For an agent env row the `context` is `format!("Agent '{}' env settings", agent.label)` (`agent_command.rs:373`). The full message for a `%LOCALAPPDATA%` value is therefore `Agent '<label>' env settings: unknown placeholder marker in value`.

### 3.4 RTK facts, verified by execution

Run against `D:\0_repos\AgentsCommander_iac\.ac\_agent_architect\rtk-matrix-history.db`, the database this authoring session itself writes to.

- `rtk --version` prints `rtk 0.42.4`.
- The default database, with `RTK_DB_PATH` unset, is `%LOCALAPPDATA%\rtk\history.db` (74 MB on this workstation). `rtk config` reports its config file separately at `%APPDATA%\rtk\config.toml`.
- `RTK_DB_PATH` is honored by `rtk gain`: with the variable set to the Agent Matrix database, `rtk gain` reported 14 commands, not the machine-wide 69k.
- The `commands` schema, read from `sqlite_master`, is exactly:
  `id INTEGER PRIMARY KEY, timestamp TEXT NOT NULL, original_cmd TEXT NOT NULL, rtk_cmd TEXT NOT NULL, input_tokens INTEGER NOT NULL, output_tokens INTEGER NOT NULL, saved_tokens INTEGER NOT NULL, savings_pct REAL NOT NULL, exec_time_ms INTEGER DEFAULT 0, project_path TEXT DEFAULT ''`.
  A second table `parse_failures (id, timestamp, raw_command, error_message, fallback_succeeded)` also exists.
- `project_path` values carry the Windows verbatim prefix, for example `\\?\D:\0_repos\AgentsCommander_iac\.ac\wg-17-dev-v5-team\repo-AgentsCommander`.
- `rtk gain -f json` prints **only** a `summary` object. Adding `-H` changes nothing in the JSON.
- **`rtk gain -f csv` prints zero bytes and exits 0.** Verified with `rtk gain -f csv | wc -c` (result `0`) and with `-f csv -H`. The page must say this rather than present csv as usable.
- Cross-agent aggregation over `_agent_*\rtk-matrix-history.db` with Python's stdlib `sqlite3` in read-only URI mode works and produced a two-row per-agent table. `sqlite3.exe` is **not** on this workstation's PATH, so the page prescribes the Python form, not a `sqlite3` CLI form.
- The per-agent database really does span replicas and repository checkouts: grouping this session's database by `project_path` returned three distinct roots (the replica, the `repo-AgentsCommander` checkout, and the workgroup root).

## 4. Change 1: add `docs/integrations/rtk.md`

Create the file with **exactly** the content between the four-backtick fence markers below. It is the deliverable; do not paraphrase it, do not reorder its sections, and do not add sections to it. The four-backtick fence is only this plan's wrapper: the file itself starts at `# RTK usage and per-agent statistics` and ends at the last `See also` bullet.

Mapping to the seven mandatory content points of the issue:

| Issue point | Section that covers it |
|---|---|
| 1. What RTK is, optional third-party, AC's relation | H1 intro block |
| 2. The recommended configuration, stated explicitly | `## Recommended configuration` |
| 3. Why that value (per agent type, absolute, survives purges) | `## Why this value` |
| 4. Reading the statistics, `rtk gain`, formats, schema, aggregation | `## Reading the statistics` and its three H3s |
| 5. The two failure modes including silent tracking loss | `## Failure modes` and its two H3s |
| 6. The `%AC_MATRIX_ROOT%` root-kind constraint | `## When %AC_MATRIX_ROOT% does not resolve` |
| 7. Cross-reference to conventions section 5 | first bullet of `## See also` |

````markdown
# RTK usage and per-agent statistics

For developers who already run RTK and want to know what each agent type actually executes. After this page, every session records its RTK history into its own agent's database, and one command tells you how much each agent type ran and saved.

[RTK](https://github.com/rtk-ai/rtk) (Rust Token Killer, Apache-2.0) is an optional third-party CLI proxy. It wraps common developer commands, compresses their output before it reaches a coding agent's context, and records every invocation in a SQLite history database.

AgentsCommander does not ship, install, update or require RTK, and no AC feature depends on it. What AC contributes is one existing mechanism: it expands `%AC_*%` path placeholders in a coding agent's environment values at spawn time, which is enough to give each agent type its own RTK database.

With `RTK_DB_PATH` unset, RTK writes every invocation on the machine into one database (`%LOCALAPPDATA%\rtk\history.db` on Windows). Its `commands` table has no agent identity column, so that shared database cannot answer "what does each agent type run?".

## Recommended configuration

Open **Settings → Coding Agents**, pick the coding agent your agents launch, and add this row under **ENVIRONMENT**:

```text
RTK_DB_PATH = %AC_MATRIX_ROOT%\rtk-matrix-history.db
```

Save, then start a new session for an agent that uses that coding agent. AC expands the placeholder at spawn, so the child process receives an absolute path such as `D:\myproject\.ac\_agent_tech-lead\rtk-matrix-history.db`.

RTK creates the file on the first wrapped command in that session. Verify it:

```bash
ls -l "<project>/.ac/_agent_<name>/rtk-matrix-history.db"
```

Exits 0 and prints one line once that agent has run its first `rtk ...` command.

Repeat the row for every coding agent whose sessions you want to measure. ENVIRONMENT rows belong to one coding agent, not to the whole installation.

## Why this value

| Property | What it buys you |
|---|---|
| `%AC_MATRIX_ROOT%` | Resolves to `<project>\.ac\_agent_<name>`, the agent's canonical Agent Matrix. Statistics aggregate **per agent type**, across every workgroup replica that agent ever ran in. |
| Absolute after expansion | RTK opens `RTK_DB_PATH` exactly as given. A relative value resolves against the session's current working directory, which changes as the agent moves between `repo-*` checkouts, so the history scatters into several files. |
| Outside `wg-*` | Workgroup directories are disposable. A database under the Agent Matrix survives a workgroup purge. |

The `project_path` column keeps the second dimension. Every row records the working directory the command ran in, so one agent's database still tells you which replica or repository checkout each command came from.

## Reading the statistics

Every command below reads the database named by `RTK_DB_PATH`. Run them inside a session of the agent you want to inspect, or set `RTK_DB_PATH` in your own shell first. Verified against rtk 0.42.4.

### One agent

```bash
rtk gain
```

First lines:

```text
RTK Token Savings (Global Scope)
════════════════════════════════════════════════════════════

Total commands:    14
```

| Flag | Effect |
|---|---|
| `-H` | Appends a **Recent Commands** list to the text report. |
| `-p` | Restricts the report to commands whose `project_path` is the current working directory. |
| `-d` / `-w` / `-m` | Daily, weekly or monthly breakdown. |
| `-F` | Prints the parse-failure log. |
| `-f json` | Prints one JSON object. |

`rtk gain -f json` prints the summary object and nothing else:

```json
{
  "summary": {
    "total_commands": 14,
    "total_input": 5804,
    "total_output": 5140,
    "total_saved": 664,
    "avg_savings_pct": 11.44038594073053,
    "total_time_ms": 231,
    "avg_time_ms": 16
  }
}
```

`-H`, `-d`, `-w` and `-m` add nothing to that JSON. `rtk gain -f csv` prints zero bytes and exits 0. Do not build a pipeline on either flag. For anything past the summary, read the database.

### The `commands` table

```sql
CREATE TABLE commands (
    id INTEGER PRIMARY KEY,
    timestamp TEXT NOT NULL,
    original_cmd TEXT NOT NULL,
    rtk_cmd TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    saved_tokens INTEGER NOT NULL,
    savings_pct REAL NOT NULL,
    exec_time_ms INTEGER DEFAULT 0,
    project_path TEXT DEFAULT ''
);
```

`timestamp` is RFC 3339 with an offset. On Windows, `project_path` carries the verbatim prefix `\\?\`, so strip that prefix before you compare paths.

The same file holds `parse_failures (id, timestamp, raw_command, error_message, fallback_succeeded)`, the log of commands that fell back to raw execution.

### All agents at once

Each agent has its own file, so aggregation is a loop over `_agent_*`. Save this as `rtk-per-agent.py` and run it with any Python 3:

```python
import glob, os, sqlite3

pattern = r"<project>\.ac\_agent_*\rtk-matrix-history.db"
rows = []
for db in glob.glob(pattern):
    agent = os.path.basename(os.path.dirname(db)).removeprefix("_agent_")
    con = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    count, saved = con.execute(
        "SELECT COUNT(*), COALESCE(SUM(saved_tokens), 0) FROM commands"
    ).fetchone()
    con.close()
    rows.append((agent, count, saved))

for agent, count, saved in sorted(rows, key=lambda row: -row[2]):
    print(f"{agent:<20}{count:>8}{saved:>12}")
```

Output:

```text
tech-lead                 15        1519
architect                 22        1210
```

To break one agent down by replica or repository checkout, swap the query for `SELECT project_path, COUNT(*), COALESCE(SUM(saved_tokens), 0) FROM commands GROUP BY project_path`.

Always open the databases read-only, as `?mode=ro` above does. A live session may be writing to one of them.

## Failure modes

### A relative prefix loses tracking silently

A leftover `.\` in front of the placeholder, as in `.\%AC_MATRIX_ROOT%\rtk-matrix-history.db`, expands into an invalid path. RTK reports:

```text
Failed to initialize tracking database ... (os error 123)
```

and then **runs the wrapped command anyway and exits 0**. Nothing else fails and no row is recorded, so the loss stays invisible until you read the statistics. If `rtk gain` reports far fewer commands than the agent ran, open the ENVIRONMENT row and confirm the value starts with `%AC_MATRIX_ROOT%` and nothing else.

### A Windows environment variable blocks the spawn

`%LOCALAPPDATA%`, `%USERPROFILE%` and every other non-AC `%WORD%` marker is rejected fail-closed. AC hands the value to the child process without a shell, so nothing would expand it, and the session refuses to start:

```text
Agent '<label>' env settings: unknown placeholder marker in value
```

Only `%AC_REPLICA_ROOT%`, `%AC_WORKSPACE_ROOT%` and `%AC_MATRIX_ROOT%` are recognized. Use one of those, or a literal absolute path.

## When `%AC_MATRIX_ROOT%` does not resolve

`%AC_MATRIX_ROOT%` resolves only for a workgroup replica launch root, that is a `__agent_*` directory under a `wg-*` workgroup. A session launched from a `repo-*` checkout, a bare `wg-*` directory, an `_agent_*` matrix directory or the root agent fails at launch with:

```text
%AC_MATRIX_ROOT% requires an AC workgroup replica launch root
```

For those roots use `%AC_WORKSPACE_ROOT%\rtk-history.db` instead, which gives one database per project with no per-agent split, or a literal absolute path. `%AC_WORKSPACE_ROOT%` resolves for any launch root inside a `.ac` workspace; the root agent resolves only `%AC_REPLICA_ROOT%`.

## See also

- [Agent Matrix conventions §5](../agent-matrix-conventions.md#5-profile-path-placeholders) - the three path placeholders, where each one resolves, and the fail-closed rules
- [Coding agents](coding-agents.md) - the coding-agent catalog and the ENVIRONMENT rows this page uses
- [Settings reference - coding agents](../reference/settings.md#coding-agents) - the `agents[].envs` shape behind the ENVIRONMENT panel
- [RTK upstream repository](https://github.com/rtk-ai/rtk)
````

## 5. Change 2: correct `docs/agent-matrix-conventions.md` section 5

**The decision the dispatch left open is settled: yes, section 5 must change.** Section 3.2 proves the backend expands three value surfaces, one of which is the coding agent's own ENVIRONMENT list, while the section's own text limits the mechanism to a profile cell. A reader who follows section 4 of this plan is doing something the reference page says is not supported. Leaving that contradiction in place would make the new page look wrong.

The change is a wording correction plus one inbound link, deliberately confined to three passages. The `## 5. Profile Path Placeholders` heading and its anchor stay as they are (see section 2 for the four inbound links that depend on the anchor).

### 5.1 Widen the blockquote

Replace this exact line:

```markdown
> These tokens are used **inside** a profile cell's command or env. For the profile matrix itself (the lettered A/B/C launch variants per coding agent), see [Coding Agent Profiles](features/coding-agent-profiles.md).
```

with:

```markdown
> These tokens are used **inside** a coding agent's own ENVIRONMENT rows, inside a profile cell's command or env, or inside the effective launch command. For the profile matrix itself (the lettered A/B/C launch variants per coding agent), see [Coding Agent Profiles](features/coding-agent-profiles.md).
```

### 5.2 Widen the first body sentence

In the paragraph that begins "Coding-agent profile command strings and `env` values may use a small set of", replace **only** its first sentence:

```markdown
Coding-agent profile command strings and `env` values may use a small set of `%...%` path placeholders that AgentsCommander expands to absolute paths **at launch**.
```

with these two sentences:

```markdown
Coding-agent command strings and `env` values may use a small set of `%...%` path placeholders that AgentsCommander expands to absolute paths **at launch**. Three value surfaces are expanded: the effective launch command tokens, the coding agent's own ENVIRONMENT rows (`agents[].envs` in `settings.json`, edited in Settings → Coding Agents), and a profile cell's `env` map.
```

Leave the rest of that paragraph, from "Only the three tokens below are recognized." onward, byte-for-byte unchanged.

### 5.3 Add the inbound link

At the end of the `### Usage examples` subsection, after the paragraph that ends "the sidebar's profile preview only mirrors these tokens for display.", append one blank line and this paragraph:

```markdown
For a worked end-to-end example that puts `%AC_MATRIX_ROOT%` in a coding agent's ENVIRONMENT row, see [RTK usage and per-agent statistics](integrations/rtk.md).
```

This is the only inbound link to the new page, and it is what makes the page reachable.

## 6. Verification

There is no automated docs gate in this repository (section 2), so verification is a manual checklist. Run it from the repository root before opening the pull request.

1. `git status --porcelain=v1` lists exactly two changed tracked paths: `docs/integrations/rtk.md` (added) and `docs/agent-matrix-conventions.md` (modified). The stray untracked `rtk-replica-history.db` must still be untracked and uncommitted.
2. `git diff --stat origin/main -- src src-tauri scripts .github` prints nothing. No code file changed.
3. `git diff origin/main -- docs/agent-matrix-conventions.md` shows exactly three hunks, matching sections 5.1, 5.2 and 5.3. The line `## 5. Profile Path Placeholders` is not in the diff.
4. Every relative link in the new page resolves to an existing file. `ls docs/agent-matrix-conventions.md docs/integrations/coding-agents.md docs/reference/settings.md` exits 0.
5. The new page contains none of the banned words from `docs/style-guide.md`.
   `grep -inE "revolutionary|unleash|supercharge|next-gen|AI-powered|game-changing|blazing-fast|seamless|magical|agentic|simply|just|easily|easy to use" docs/integrations/rtk.md`
   must print nothing and exit 1. The content in section 4 was checked against this exact regex and passes.
6. The recommended value names the correct file. `grep -c "rtk-matrix-history.db" docs/integrations/rtk.md` is at least 4, and `grep -c "rtk-replica-history.db" docs/integrations/rtk.md` is 0.
7. `node scripts/validate-branch-name.mjs --branch docs/1408-rtk-usage-and-per-agent-stats` exits 0.

No `cargo test` or `npm test` run is required by this change, and none should be offered in the pull request as evidence.

## 7. Dependency-cycle gate

Applied per the `verify-no-dependency-cycles` criterion.

- **Module arcs added:** zero. The change set is two markdown files. No `use`, `import`, `mod` or `require` statement is created, moved or deleted.
- **Module arcs removed:** zero.
- **SCC impact:** none. `cyclicSccs` is unchanged and every SCC member set is identical, because this diff contains no module graph edge that could change them.
- **Cross-boundary arcs:** zero, therefore zero crossing a previously-clean SCC boundary.
- **Role and layering hygiene:** no module gains an `AppHandle` or `tauri` dependency. No transport-taking function is introduced or relocated.
- **Detector acceptance criterion:** a Step-N detector run is not applicable, because the plan touches no module structure. Criterion 2 of section 6, `git diff --stat origin/main -- src src-tauri scripts .github` printing nothing, is the equivalent gate for a documentation-only change and is mandatory.

**Gate result: PASS.**

## 8. Decisions settled by this plan

### 8.1 Does `docs/agent-matrix-conventions.md` section 5 change? Yes.

Settled in section 5, with the exact three edits specified. The rationale is section 3.2: the backend expands the coding agent's own ENVIRONMENT rows, so the section's self-limitation to "a profile cell's command or env" is factually wrong and directly contradicts the new page.

### 8.2 What the page says about `--format csv`

The issue asks the page to cover "its `--format json|csv` output". Verified behavior at rtk 0.42.4 is that `-f csv` emits zero bytes and exits 0, and that `-f json` emits only the summary object regardless of `-H`, `-d`, `-w` or `-m`. `docs/style-guide.md` requires every snippet to run and requires being specific about failure, so the page states both limits and routes the reader to the database for anything past the summary. Do not soften this into "csv output is also available".

### 8.3 The stray `rtk-replica-history.db` in the repository root

Out of scope, deliberately. It is an untracked leftover from manual RTK testing, not a repository artifact. Adding a `.gitignore` rule would be a repository change beyond the documentation-only scope of #1408, and the file name is not even the recommended one. Leave it untracked and raise it separately if it recurs.

### 8.4 Aggregation tooling: Python, not the `sqlite3` CLI

`sqlite3.exe` is not on this workstation's PATH, while Python 3 with the stdlib `sqlite3` module is present and the prescribed script was executed successfully against two real agent databases. Prescribing a `sqlite3` one-liner would violate the style guide's "every snippet must run" rule on the reference environment.

### 8.5 Where the recommended row goes

Settings, Coding Agents, ENVIRONMENT, on each coding agent, not in a profile cell. A profile cell `env` entry would also expand, but profiles are per launch variant, so the recommendation would stop applying whenever a session switched profile. The page says the row belongs to one coding agent and does not offer the profile alternative.

## 9. Implementation order

1. Create `docs/integrations/rtk.md` with the content in section 4, verbatim.
2. Apply the three edits in section 5 to `docs/agent-matrix-conventions.md`.
3. Run the section 6 checklist.
4. Commit both documentation files and force-add this plan:
   `git add docs/integrations/rtk.md docs/agent-matrix-conventions.md && git add -f plans/1408-rtk-usage-and-per-agent-stats.md`.
5. Open the pull request against `main` from `docs/1408-rtk-usage-and-per-agent-stats`, referencing #1408.

## 10. Risks and their bounds

| Risk | Bound |
|---|---|
| RTK changes `rtk gain` output in a later version | The page pins "Verified against rtk 0.42.4" in `## Reading the statistics`. A future version needs a re-verification pass, not a redesign. |
| RTK adds a working `--format csv` | The statement is scoped to the pinned version and is a factual observation, not a claim about the tool's roadmap. |
| A reader copies the value into a profile cell instead of ENVIRONMENT | It still expands and still works; it only stops applying on a profile switch. Section 8.5 keeps the page from encouraging it. |
| A reader runs an agent from a `repo-*` root and the session refuses to start | The `## When %AC_MATRIX_ROOT% does not resolve` section names the exact error string and gives the `%AC_WORKSPACE_ROOT%` fallback. |
| The section 5 wording edit conflicts with a concurrent branch touching that file | The three hunks are small and anchored on quoted text. Re-anchor on the quoted text, never on line numbers. |
