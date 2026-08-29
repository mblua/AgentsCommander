# Host platform rules (`{{HOST_PLATFORM_RULES}}`)

For operators running AgentsCommander. What the per-platform `{{HOST_PLATFORM_RULES}}` block is, where it renders, and how to customize it by editing a plain file — without code changes or a release.

## What it is

`{{HOST_PLATFORM_RULES}}` is the 8th mandatory placeholder of the global default agent template (template version 5), rendered immediately after `{{CLI_CONTEXT}}` in every materialized agent context. Its content is selected by the session's **execution platform**:

| Host OS | Rendered block |
|---|---|
| Windows | The Git Bash CLI-routing rule (single source of truth; the `{{INTER_AGENT_MESSAGING}}` section only carries a pointer to it) |
| Linux / macOS | A minimal note that no platform-specific shell routing rules apply |
| Container (transport api) session | Nothing — no block, no file read |

The block is mandatory: a preserved personalized template that lacks the token receives the current block through the append fallback, exactly like `{{AGENT_REPOS}}`.

## Where it renders

- **Global template** (`Context.AgentsCommander.md` / `AGENTS.md` / `CLAUDE.md` materializations): between `## CLI executable` and `## Session credentials`.
- **Root Agent runtime prologue**: as its 10th code-owned block, between the CLI context and session credentials blocks. The root role template (`Context.root-agent.md`) is never rendered through the placeholder machinery; the root gets the block from the prologue.
- **Coordinator sessions**: through the global render — no coordinator-template change is needed and none exists (the block must not be added to `Context.coordinator.md`, or it would duplicate).
- **Container sessions**: never.

## The platform files

Each platform's block content is configurable by editing a per-platform file in the **project** `.ac` root:

| File | Platform |
|---|---|
| `.ac/Context.platform.windows.md` | Windows host sessions |
| `.ac/Context.platform.linux.md` | Linux host sessions |
| `.ac/Context.platform.macos.md` | macOS host sessions |

The file content **is** the rendered block, including its `## Host Platform Rules` heading — whatever you write in the file replaces the whole section verbatim.

The files are **project-level**: they are seeded absent-only into project `.ac` roots (never into the app-config directory), so they can be committed to an infrastructure-as-code repository and reviewed like any other file.

## How the lifecycle works

- **Seeding**: on project creation/registration the full project template set is seeded; for an already-known project, the platform files are seeded absent-only by the render path on the first materialization after open when a platform file is missing (same `sync_one_template` lifecycle, state entries `platform.*` v1 with `lastSeededSha256`). The startup scan never creates templates (unchanged). A pre-existing file is never overwritten; an unowned pre-existing file is preserved silently.
- **Editing**: edits are preserved. The seeded-template state (`.agentscommander-context-templates.json`) records the file in the observed posture (`lastObservedSha256`), and if a future app default differs, the file is offered a pending update instead of being silently overwritten.
- **Deleting or emptying** a platform file: the render falls back to the embedded default for that platform with a `WARN` line in `app.log`; the next project open re-seeds a deleted file (absent-only).
- **Versioning**: platform files are versioned in the seeded-template state like the global/coordinator templates (`platform.windows`, `platform.linux`, `platform.macos`, version 1). When a future release changes a platform default, the previous default is frozen as a snapshot and recognized, so seeded files auto-update while edited files stay preserved with the pending-update offer.

## Applying a change

1. Edit `.ac/Context.platform.windows.md` (or `.linux.md` / `.macos.md`) in the project.
2. Respawn the session — the new text is rendered on the next materialization. **No rebuild, no release.**

The render reads the file on every materialization; there is no cache.

## Notes

- The byte budget that binds the default template applies to the **embedded defaults**; an owner-authored longer file is a deliberate choice and is read without a size cap.
- Deleting the file between project opens renders the embedded default (identical content to a freshly seeded file) until the next open re-seeds.
- The Windows block is the single source of truth for the Git Bash routing rule; do not re-add that recipe to the messaging section or any other template.
