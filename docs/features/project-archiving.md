# Project archiving

For developers whose sidebar has collected projects they are done with. After this page you can archive a project, understand exactly what that does and does not touch, get it back, and read the message that tells you why an archive was refused.

Archiving hides a project from the sidebar. It is a change to AC's registration list, not to your files: the project's folder, its `.ac` directory, its workgroups and its history stay exactly where they are.

## What archiving does

Archiving moves one path from AC's active project list to its archived list. That is the whole operation. `projectPaths` loses the entry and `archivedProjectPaths` gains it, each alongside its portable companion path, preserving array order.

**Nothing on disk is moved, renamed or deleted.** Archiving is not a delete, it does not free space, and it does not touch the project's contents. An archived project is described in the settings reference as "registered but hidden", which is exactly right: AC still knows about it, and stops showing it.

What changes for you:

- The project disappears from the sidebar and from the workgroup rail.
- It stops taking part in startup restoration and team discovery.
- No session can start inside it while it is archived. That is enforced, not a convention, and it is why the auto-unarchive flow below exists.

## Archiving a project

Archiving is started from the project's own row in the sidebar and applies to that one project.

AC checks for open sessions before it writes anything. If the project has any, the archive is refused and nothing changes; see the next section for what counts as open. If the check passes, AC writes the archive, then **checks a second time**. If a session became live between the two checks, AC unarchives the project again, tells the sidebar, restores the project catalog, and reports the same refusal message as if it had never started.

A project whose folder has vanished can still be archived. That is deliberate: a registration pointing at a folder you deleted is exactly the kind of entry you want out of the sidebar.

## What blocks an archive

One thing blocks an archive: **open sessions inside the project**. AC builds a single list of blocker names from two places and deduplicates it, so the same name never appears twice:

- pending spawns whose working directory is inside the project, and
- live sessions whose working directory is inside the project.

Both feed the same "open session(s)" count in the same message.

Two exclusions matter, because without them the count will not match what you see in the sidebar:

- **Root Agent sessions never block an archive.** Not by their Root Agent flag, and not by their directory name.
- **A running session with no terminal does not block.** Liveness here means a live PTY, so a session record that is listed but has no terminal attached is not a blocker. An exited session under the project does not block either.

The practical consequence: close the sessions the message names, then archive again. There is no force flag, and this page will not invent one.

## Unarchiving

Archived projects live in the **Archived projects** modal. Each row shows the project's folder name and its full path, and carries an `Unarchive` button, which reads `Unarchiving...` while it runs. Unarchiving puts the path back in the active list and the project back in the sidebar.

Two things a row can tell you before you restore it:

- `Folder missing` means the folder the registration points at is gone.
- `Workspace missing` means the folder is there but its workspace is not.

A row in either state gains a second button, `Remove from list`, which drops the registration entirely. **That button removes the project from AC, not from disk**, and it is offered only for rows AC could not validate.

When the list is empty the modal reads `No archived projects`. `Close` and the Escape key both close it.

## Auto-unarchive

An archived project cannot hold a running session. When a session starts inside one anyway, AC does not refuse it and does not leave the project archived: it restores the project and then tells you.

The dialog is titled `Project un-archived`, or `Projects un-archived` when more than one was restored, and it names each project and the session that caused it. For a single project it reads:

> `<folder name>` was restored to your project list. A session started inside it, and an archived project cannot hold a running session.

For several:

> These projects were restored to your project list because sessions started inside them, and archived projects cannot hold running sessions.

The only control is `Got it`, and Escape does not close the dialog. **Confirming it acknowledges the notice; it does not perform or undo anything.** The restore already happened before you were told. If you did not want the project back, archive it again after closing the session that pulled it in.

## Settings

| Key | What it controls |
|---|---|
| `archivedProjectPaths` | Absolute paths of archived projects: registered but hidden. `[]` by default. |
| `archivedProjectPathsRelativeToInstance` | The portable companion of each archived path, same length and order. |
| `projectPaths` | The active project list an archive removes an entry from. |

See [Settings reference](../reference/settings.md#projects) for the companion-path rules and how archiving moves the whole pair together.

## Troubleshooting

**"Cannot archive: 2 open session(s) in this project (dev-rust, tech-lead). Close them first."** Exactly what it says: those sessions are open under the project. Close them and archive again. The message names at most three:

```text
Cannot archive: 5 open session(s) in this project (dev-rust, tech-lead, architect, and 2 more). Close them first.
```

The count in front is the total, and `and <m> more` covers the ones past the first three.

**"The count is higher than the sessions I can see."** A pending spawn counts as an open session, even before its terminal exists. Wait for it to finish starting, or close it, and the count drops.

**"The count is lower than the sessions I can see."** Root Agent sessions are excluded, and so is any session without a live terminal. Both are deliberate.

**"The project got archived and I got `The project could not be restored automatically (<error>). Open Archived Projects to restore it.`"** This is the rare case: a session became live between AC's two checks, AC tried to roll the archive back, and the rollback itself failed. The project is archived and you did not want it archived. Open the **Archived projects** modal and press `Unarchive` on it. The blocker message that precedes that sentence tells you which sessions appeared.

**"I archived a project and its files are gone."** Archiving does not touch files. It edits two lists in `settings.json`. If files are missing, something else removed them; check the row in the modal for `Folder missing`, which means the registration now points at nothing.

## See also

- [Settings reference](../reference/settings.md#projects) - the project and archived-project path arrays
- [Sidebar guide](sidebar-guide.md) - the project panel the archived project disappears from
- [Portable instances](portable-instances.md#portable-project-paths) - the companion path format archiving preserves
