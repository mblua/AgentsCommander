# App windows

For developers wondering which AgentsCommander window they are looking at and where the others come from. After this page you can name every window AC opens, know what each is for, and find the page that owns it.

AC is one application that opens several windows. One page routes them all: the window you get is decided by a `?window=` parameter, and the main window is what you get without one.

## The window map

| Window | What it is for | How to open it |
|---|---|---|
| Sidebar | Projects, rooms, replicas and sessions, plus the room rail | Part of the main window; also runs on its own in the browser build |
| Main | The default window: the sidebar beside a central pane | Starting AC |
| Terminal | The central pane's terminal, or a detached window locked to one session | Part of the main window; detaching moves one session into its own window |
| Guide | Hints and a tutorial | A button in the sidebar action bar |
| Watchers | Watcher match activity | A button in the terminal status bar, which opens it scoped to that session. See [Watchers](watchers.md) |
| Resource Monitor | Per-agent memory, CPU and process counts | Its own window, or attached to the main window's central pane. See [Resource monitor](resource-monitor.md) |
| Spec Board | One Mermaid file with a live preview | The Spec Board button in the sidebar toolbar, once `specBoardEnabled` is on. See [Spec Board](spec-board.md) |
| Screenshot overlay | The frozen-screen selection surface for a capture | The global screenshot hotkey. See [Screenshot capture](screenshot-capture.md) |

## Sidebar

The sidebar is the navigation surface: the room rail down one edge, and the project panel listing projects, rooms, replicas and sessions.

In the desktop app it lives inside the main window. In the browser build it is rendered beside the terminal in the served page, with a draggable divider between them and a toggle to move it to the other side.

Everything the sidebar shows is documented in [Sidebar guide](sidebar-guide.md).

## Main window and Home

The main window is the sidebar plus a central pane. The pane usually holds the terminal, and it can hold the Resource Monitor instead when you attach it.

Before a session is selected the pane shows **Home**, a markdown document rendered inside the app. The markdown is parsed and then sanitized before it is displayed, so a document cannot inject scripts into the app.

Home has three states and a control:

- A refresh button, tooltipped `Refresh`, disabled while a fetch is in flight.
- While loading with nothing to show yet: `Loading Home…`.
- When the fetch fails with nothing to show yet: `Could not load Home: <error>`, with a `Try again` button.

Both states are conditioned on there being no content yet, so refreshing an already-loaded Home leaves the current document on screen instead of blanking it.

## Terminal

The terminal is where a session's output goes, and it exists in two shapes: the central pane of the main window, and a detached window locked to one session. A detached window is marked as such in its titlebar.

Around the terminal itself:

- **The titlebar** carries the session name and its shell, and the detached marker when the window is detached.
- **The status bar** carries the session's launch command and its controls, including the microphone button. See [Voice-to-text](voice-to-text.md) for what the mic does and how to cancel a recording.
- **The last prompt** display shows the most recent command for the session.

**The room task** strip shows the title from the room's `TASK.md`, parsed from the file's frontmatter. You can edit the title in place: `Enter` saves it, `Escape` cancels. The control refuses an empty title with `Title cannot be empty.`, and it stops if the session changed underneath the edit, with `Session changed; cancel and retry.` Both the edit and the clean control are disabled when there is no session or the session is not inside a room.

**Cleaning the task** asks first. The confirmation is titled `Clean TASK?` and states what it will do: it resets the room `TASK.md`, replacing all frontmatter fields and body content with `title: 'Clean'` and the body `Ready to start a new topic`. If a `TASK.md` exists, a timestamped backup is saved alongside it before the reset. The buttons are `Cancel`, which has focus when the dialog opens, and `Clean`. `Escape` cancels, and `Enter` cleans only when `Clean` already has focus.

## Guide

The Guide is a small separate window with its own titlebar, holding two tabs: **Hints**, which opens first, and **Tutorial**.

The tutorial covers the same ground as [Quickstart](../quickstart.md). Read whichever you prefer; they are not different procedures.

## Other windows

**Watchers.** One window listing watcher matches across your agent sessions, with a scope selector and filters. See [Watchers](watchers.md).

**Resource Monitor.** Per-agent memory, CPU and process counts, plus the watchdog state. It runs either as its own window or attached to the main window's central pane, and it remembers which. See [Resource monitor](resource-monitor.md).

**Spec Board.** One Mermaid file with source on one side and a live diagram on the other. Off by default. See [Spec Board](spec-board.md).

**Screenshot overlay.** The full-screen surface you drag a rectangle on after pressing the capture hotkey. See [Screenshot capture](screenshot-capture.md).

## Troubleshooting

**"Home is stuck on `Loading Home…`."** The fetch has not returned. The message only appears while there is no content at all, so this is a first load, not a refresh. Press `Refresh` once it enables.

**"Home says `Could not load Home:` and an error."** The document could not be fetched. `Try again` re-runs the same fetch; the error text names the cause.

**"The task controls are greyed out."** Either no session is selected, or the selected session is not inside a room. `TASK.md` belongs to a room, so a plain shell has nothing to edit.

**"I cleaned the task and lost its content."** The reset is what `Clean` does, and the confirmation says so before you press it. A timestamped backup is written next to `TASK.md` when one existed; look there.

**"The Spec Board button is not in the toolbar."** `specBoardEnabled` is `false`, which is the default. See [Spec Board](spec-board.md).

## See also

- [Sidebar guide](sidebar-guide.md) - everything the sidebar window shows
- [Keyboard shortcuts](../reference/keyboard-shortcuts.md) - the two window shortcuts and the one global hotkey
- [Notifications and dialogs](notifications-and-dialogs.md) - the modals these windows raise
