# Spec Board

For developers who keep a diagram next to the code and want a coding agent to edit it. After this page you can open a Spec Board on a Mermaid file, watch it render as you type, recover an earlier version from a snapshot, and hand the file to an agent without leaving the board.

The Spec Board is a separate AC window holding one Mermaid file: source on one side, the rendered diagram on the other. It saves to a real file in your repo, snapshots your edits as you go, and notices when something else changes the file underneath you.

## What a Spec Board is

A Spec Board is one document, backed by one file on disk, open in its own window. The document holds the file's text, the diagram parsed out of it, and the state AC needs to protect your work: whether it has unsaved changes, whether the file on disk has moved ahead of you, and its snapshot history.

The file is a Mermaid file. The board expects exactly one diagram in it, which is what lets the preview render without you choosing anything.

Because the board is a window rather than a panel, it survives independently of the sidebar: opening a board does not take over the main window, and closing the board does not touch your sessions.

## Turning it on

The Spec Board is **off by default**. Set `specBoardEnabled` to `true` in `settings.json` and the Spec Board button appears in the sidebar toolbar; that button is what opens the window.

`specBoardEnabled` is a manual-only field. Edit it while AC is closed, or reload settings before you use any Settings save path, otherwise the next in-memory save can clobber your change.

One caveat worth knowing: the key controls the toolbar entry point only. It is not an access-control or a security boundary, and the backend Spec Board commands stay callable whatever it is set to.

## The editing window

The toolbar sits across the top of the board:

| Button | What it does |
|---|---|
| `New` | Starts a new document. The board closes the previous one for you. |
| `Open` | Opens a file picker and loads the file you choose. |
| `Save` | Writes the current text to the document's file. With no path yet, it asks where to save. It is disabled while a saved document has no unsaved changes. |
| `Save As` | Always asks for a destination and saves there. |
| `Ask Agent` | Opens the panel described below. Disabled until the document has a path. |
| `-`, `Reset Zoom`, `+` | Zoom the preview out, back to 100 percent with the pan reset, and in. |

The preview renders the diagram from the source as you edit. You can also zoom it with the mouse wheel and pan it by dragging with the left button, which is usually faster than the toolbar buttons for a large diagram.

When the source does not parse, the preview **keeps the last diagram it rendered** and reports the parser's message instead of blanking. That is intentional: a half-typed edit should not make your diagram disappear.

## Snapshots

The board snapshots your work as you edit, without you asking. An edit schedules a snapshot 750 milliseconds later, and the snapshot is skipped if the content is identical to the last one taken, so holding a key down does not fill the history with duplicates.

Each snapshot records an id, a label, when it was created, what produced it, the full text, the diagram source, and a hash of the content. Snapshots live in a per-document snapshot directory, not inside the file you are editing.

Restoring a snapshot loads it into the open document and marks the document as having unsaved changes. It does **not** write to disk. Nothing is committed to your file until you save, so you can look at an older version and change your mind.

## Conflicts and unsaved work

Two dialogs protect the file, and each says exactly what it is protecting.

**Something else changed the file.** The board watches the file it has open. When the file changes underneath you, a banner appears reading `File changed externally.` with three buttons:

- `Apply External` takes what is now on disk and replaces what is in the board.
- `Keep Mine` keeps your text and leaves the file on disk alone until you save.
- `Save As` writes your text somewhere else, so neither version is lost.

This is the banner you get when a coding agent edits the file you asked it to edit. Reaching for `Apply External` is the normal answer there.

**You are closing with unsaved changes.** The board asks `You have unsaved changes. Do you want to save before closing?` with `Save`, `Discard` and `Cancel`. `Cancel` returns you to the board and closes nothing.

## Asking an agent about the spec

`Ask Agent` sends the file to a running session instead of pasting the diagram into a terminal by hand.

The panel opens with the heading `Ask Agent`, a session picker starting on `Select a session...`, and an instruction box with the placeholder `Instructions...`. Choose the session, describe the change, and press `Send`.

What AC sends is a prompt naming the file's absolute path, your request, and two standing instructions to the agent: keep exactly one Mermaid diagram in the file, and save the file when done. The board watches that file, so the diagram updates when the agent saves.

`Ask Agent` stays disabled until the document has a path. An agent cannot edit a file that does not exist yet, so save the board first.

## Settings

| Key | What it controls |
|---|---|
| `specBoardEnabled` | Whether the Spec Board button appears in the sidebar toolbar. `false` by default. It gates the entry point only, not the backend commands. |

See [Settings reference](../reference/settings.md#window--ui) for the group this key belongs to and for how manual-only fields behave.

## Troubleshooting

**"There is no Spec Board button in the toolbar."** `specBoardEnabled` is `false`, which is the default. Set it to `true` while AC is closed. If you edited it while AC was running, the next in-memory save may have written the old value back.

**"The preview stopped updating and shows an error."** The source no longer parses. The board reports the parser's message, or `Parse error` when the parser gives none, and keeps the previous diagram on screen so you can compare. Fix the source and the preview catches up on its own.

**"The preview is empty and says `Failed to load Mermaid parser.`"** The Mermaid parser did not load in that window. Close the board and open it again; nothing in your file causes this.

**"`Ask Agent` is greyed out."** The document has no path yet. Save it, then the button enables.

**"An operation failed with `Spec board document not found: <id>`."** The board is acting on a document the backend has already closed, which happens if the window was reopened behind the operation. Open the file again and retry.

## See also

- [App windows](app-windows.md) - the other windows AC opens beside the main one
- [Settings reference](../reference/settings.md#window--ui) - `specBoardEnabled` and its neighbours
- [Concepts](../concepts.md) - sessions, rooms, and the vocabulary this page uses
