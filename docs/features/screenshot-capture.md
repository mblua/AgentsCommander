# Screenshot capture

In-app screenshot capture lets you press a global hotkey, drag a rectangle over the frozen screen, and save a PNG inside the workgroup replica that owns the session you are working with, with the saved path already on your clipboard.

Use this feature when you want to hand a coding agent a picture of what you are looking at. It is not a terminal snapshot (that reads a backend terminal viewport without touching OS pixels) and not [window capture](window-capture.md) (that captures exactly one native window from the CLI or the API). Screenshot capture is Windows-only: other targets compile a stub that reports the feature as unsupported and never registers the hotkey.

## Before you start

You need:

- AgentsCommander running on Windows;
- a session selected in the app and displayable, because the screenshot belongs to that session; and
- that session's working directory inside a workgroup replica (an `__agent_*` directory), because the replica root is the destination.

A screenshot records whatever is on your monitors, including passwords, tokens, source code, and personal data. AgentsCommander saves the exact cropped pixels with no redaction, and it copies the file path to your clipboard.

## Capture a screenshot

1. Select the session the screenshot belongs to.
2. Press the capture hotkey (`Ctrl+Q` by default).
   AgentsCommander captures every monitor and opens one overlay window per monitor. Each overlay shows a frozen image of that monitor, not a live view, so the screen you are photographing cannot change under you.
3. Drag a rectangle over the area you want. A magnifier follows the pointer while you hover and while you drag, so you can place the edges precisely.
4. Release the pointer.
   AgentsCommander crops the frozen image, writes the PNG, copies its path to the clipboard, and closes every overlay.

To abandon the capture, press `Escape` or close the overlay. Nothing is written.

Two behaviors are deliberate and are not failures:

- Releasing on a selection that is a few pixels wide or tall discards that selection and leaves the overlay open, so you can drag again.
- Pressing the hotkey while a capture is already in flight does nothing. The second press is ignored so you cannot photograph your own overlays or start two captures at once.

## Where the file goes

AgentsCommander walks up from the active session's working directory to its `__agent_*` replica root and writes the file directly in that root:

```text
<workgroup-replica-root>\agentscommander-screenshot-<YYYYMMDD>-<HHMMSS>-<session-id-prefix>.png
```

The timestamp is local time. The last segment is the first 8 hexadecimal characters of the session id, so two sessions capturing in the same second still get distinct names.

The saved path is copied to the clipboard as text. Copying is best effort: if the clipboard is unavailable, AgentsCommander logs a warning and the capture still succeeds.

The app log records one line per successful capture:

```text
[screenshot] saved <path> for session '<session-name>'
```

If a write fails after the file is created, AgentsCommander deletes the partial file, closes the overlays, and reports the failure.

## Configure the hotkey

The hotkey lives in `settings.json`:

```json
{
  "screenshotCaptureHotkey": "Ctrl+Q"
}
```

The value must be one modifier plus one key, joined by `+`:

| Part | Accepted | Notes |
|---|---|---|
| Modifier | `Ctrl` or `Control` | Case-insensitive. No other modifier is supported. |
| Key | One ASCII letter or digit | Normalized to upper case. |

AgentsCommander validates the value when it reads settings and when you save them, and refuses an invalid value with an exact message:

| Message | Cause |
|---|---|
| `hotkey must not be empty` | The value is empty or only whitespace. |
| `hotkey '<value>' must be one modifier plus one key, e.g. Ctrl+Q` | The value is not exactly two parts separated by `+`. |
| `unsupported modifier '<other>'; only Ctrl is supported` | The modifier is anything other than `Ctrl` or `Control`. |
| `hotkey key '<key>' must be a single letter or digit` | The key is empty, longer than one character, or not alphanumeric. |

See the [settings reference](../reference/settings.md#window--ui) for the field entry. If the shortcut does not become active after you save, restart AgentsCommander.

## Check that the shortcut is active

When the hotkey is registered, the titlebar shows a chip with a camera glyph and the shortcut, for example `Ctrl + Q`. Its tooltip and accessible name are `Screenshot capture shortcut: <hotkey>`.

**An absent chip means the shortcut is not active.** The most common cause is another application already owning that combination at the OS level. Pick a different key in `screenshotCaptureHotkey` and restart.

Registration is also written to the app log:

```text
[screenshot] global hotkey registered 'Ctrl+Q'
[screenshot] global hotkey registration failed: <error>
```

## Availability right after startup

The shortcut becomes live when the app's event loop starts pumping, which is shortly after the window appears, not at process start. This is by design: the OS registers a global hotkey on the main thread, and forcing that work to complete earlier would block the main thread, which is exactly what starves the WebView and prevents dialogs such as the update prompt from rendering.

Presses inside that window are queued by the OS, not lost. They are serviced in a burst as soon as the event loop drains, so a press made while AgentsCommander is still restoring sessions still runs.

A press serviced before any session is selectable does not hang or crash. It fails visibly with `No active session is selected in AgentsCommander`, and you retry once the session list is up.

## Failures and recovery

Every failure before the overlays open raises and focuses the AgentsCommander window and requests your attention, so the message cannot be missed behind other applications.

| Message | Cause | Recovery |
|---|---|---|
| `No active session is selected in AgentsCommander` | No session is selected, or the selected session is not displayable. | Select a live session and press the hotkey again. |
| `The active session could not be found` | The selected session disappeared from the session list between the press and the capture. | Select an existing session and retry. |
| `Cannot resolve the active session directory: <error>` | The session's working directory cannot be canonicalized (it was deleted, renamed, or is unreachable). | Restore the directory, or point the session at a valid working directory. |
| `The active session is not inside a workgroup replica directory, so there is no place to save the screenshot` | The session's working directory has no `__agent_*` replica root above it, for example an ad-hoc shell outside a workgroup. | Run the capture from a session that lives inside a workgroup replica. |
| `No monitors were found to capture` | The monitor enumeration returned nothing. | Confirm a display is attached and active, then retry. |
| `saved file escaped the replica root` | The destination resolved outside the replica root between validation and write. | Do not link or redirect the replica root. Retry from a plain directory. |

AgentsCommander also refuses a replica root that is a link or reparse point, and validates the replica's identity before writing. A replica that fails those checks reports a failure instead of writing outside its owner.

## See also

- [Window capture](window-capture.md)
- [Terminal snapshots](terminal-snapshots.md)
- [Settings reference](../reference/settings.md#window--ui)
