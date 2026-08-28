# Screenshot capture

In-app screenshot capture lets you press a global hotkey, drag a rectangle over the frozen screen, and save a PNG inside the room replica that owns the session you are working with, with the saved path already on your clipboard.

Use this feature when you want to hand a coding agent a picture of what you are looking at. It is not a terminal snapshot (that reads a backend terminal viewport without touching OS pixels) and not [window capture](window-capture.md) (that captures exactly one native window from the CLI or the API). Screenshot capture is Windows-only: other targets compile a stub that reports the feature as unsupported and never registers the hotkey.

## Before you start

You need:

- AgentsCommander running on Windows;
- a session selected in the app and displayable, because the screenshot belongs to that session; and
- that session's working directory inside a room replica (an `__agent_*` directory), because the replica root is the destination.

A screenshot records whatever is on your monitors, including passwords, tokens, source code, and personal data. AgentsCommander saves the exact cropped pixels with no redaction, and it copies the file path to your clipboard.

## Capture a screenshot

1. Select the session the screenshot belongs to.
2. Press the capture hotkey (`Ctrl+Q` by default).
   AgentsCommander captures every monitor and opens one overlay window per monitor. Each overlay shows a frozen image of that monitor, not a live view, so the screen you are photographing cannot change under you.
3. Drag a rectangle over the area you want. A magnifier follows the pointer while you hover and while you drag, so you can place the edges precisely.
4. Release the pointer.
   AgentsCommander crops the frozen image, writes the PNG, copies its path to the clipboard, and closes every overlay. A success toast reads `Screenshot saved. Path copied: <path>` and clears itself after 30 seconds.

To abandon the capture, press `Escape` or close the overlay. Nothing is written.

Two behaviors are deliberate and are not failures:

- Releasing on a selection thinner than 2 pixels in either direction discards that selection and leaves the overlay open, so you can drag again. Both the width and the height must reach 2 pixels, so a 1 by 500 pixel drag is rejected.
- Pressing the hotkey while a capture is already in flight does nothing. The second press is ignored so you cannot photograph your own overlays or start two captures at once.

## Where the file goes

AgentsCommander walks up from the active session's working directory to its `__agent_*` replica root and writes the file directly in that root:

```text
<room-replica-root>\agentscommander-screenshot-<YYYYMMDD>-<HHMMSS>-<session-id-prefix>.png
```

The timestamp is local time. The last segment is the first 8 hexadecimal characters of the session id, so two sessions capturing in the same second still get distinct names.

The saved path is copied to the clipboard as text. Copying is best effort: if the clipboard is unavailable, AgentsCommander logs a warning and the capture still succeeds.

The app log records one line per successful capture:

```text
[screenshot] saved <path> for session '<session-name>'
```

If a write fails after the file is created, AgentsCommander deletes the partial file, closes the overlays, and reports the failure.

## Configure the hotkey

Change the shortcut from the Settings dialog. The dialog validates the field before it saves anything:

| Message | Cause |
|---|---|
| `Screenshot hotkey is required` | The field is empty. |
| `Screenshot hotkey must look like Ctrl+Q` | The value does not match the expected shape. |

The same value lives in `settings.json`:

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

AgentsCommander validates the value again when it reads settings and when you save them, and refuses an invalid value with an exact message:

| Message | Cause |
|---|---|
| `hotkey must not be empty` | The value is empty or only whitespace. |
| `hotkey '<value>' must be one modifier plus one key, e.g. Ctrl+Q` | The value is not exactly two parts separated by `+`. |
| `unsupported modifier '<other>'; only Ctrl is supported` | The modifier is anything other than `Ctrl` or `Control`. |
| `hotkey key '<key>' must be a single letter or digit` | The key is empty, longer than one character, or not alphanumeric. |

See the [settings reference](../reference/settings.md#window--ui) for the field entry.

### What happens when you save

AgentsCommander registers the new shortcut immediately after the save. You do not restart the app. Saving the same value twice succeeds and changes nothing, and when you save a different value AgentsCommander registers the new shortcut first and releases the old one afterwards, so you are never left with no shortcut at all.

The two ways a save can go wrong behave differently:

- **The syntax is invalid.** The save is rejected before anything is written to disk, and your previous hotkey stays registered and keeps working. You rarely reach this path, because the Settings dialog refuses the value first.
- **The syntax is valid but Windows refuses the combination**, usually because another application already owns it. The save succeeds and the new value is persisted; only the registration fails. AgentsCommander raises an error toast reading `Screenshot hotkey was saved but could not be registered: <error>`, and keeps the previously registered shortcut alive as a fallback. So the configured value is the new combination, the key that actually fires is still the old one, and the status for the configured value reports it as not registered. Pick another combination and save again.

## Check that the shortcut is active

When the hotkey is registered, the titlebar shows a chip with a camera glyph and the shortcut, for example `Ctrl + Q`. Its tooltip and accessible name are `Screenshot capture shortcut: <hotkey>`.

**An absent chip means the shortcut is not active.** The most common cause is another application already owning that combination at the OS level. At startup AgentsCommander checks the registration and raises an error toast reading `Screenshot hotkey <configured> is not active: <error>` when the configured shortcut failed to register, so a conflict detected at launch reaches you even if you never look at the titlebar. Pick a different combination in Settings and save; the new shortcut registers at once.

Registration is also written to the app log:

```text
[screenshot] global hotkey registered 'Ctrl+Q'
[screenshot] global hotkey registration failed: <error>
```

## Availability right after startup

The shortcut is active as soon as the app finishes starting, without waiting for session restore. It is not active at process start: Windows registers a global hotkey on the main thread, so the registration completes only once the app's event loop is running. This is by design. Forcing that work to complete earlier would block the main thread, which is exactly what starves the WebView and prevents dialogs such as the update prompt from rendering.

Presses inside that window are queued by the OS, not lost. They are serviced in a burst as soon as the event loop drains, so a press made while AgentsCommander is still restoring sessions still runs.

A press serviced before any session is selectable does not hang or crash. It fails visibly with `No active session is selected in AgentsCommander`, and you retry once the session list is up.

## Failures and recovery

AgentsCommander reports every failure as an error toast carrying the backend message verbatim. The toast has no timeout: it stays until you dismiss it, so a failure cannot scroll past unnoticed. A failure before the overlays open also raises and focuses the AgentsCommander window and requests your attention, so the message cannot be missed behind other applications.

| Message | Cause | Recovery |
|---|---|---|
| `No active session is selected in AgentsCommander` | No session is selected, or the selected session is not displayable. | Select a live session and press the hotkey again. |
| `The active session could not be found` | The selected session disappeared from the session list between the press and the capture. | Select an existing session and retry. |
| `Cannot resolve the active session directory: <error>` | The session's working directory cannot be canonicalized (it was deleted, renamed, or is unreachable). | Restore the directory, or point the session at a valid working directory. |
| `The active session is not inside a room replica directory, so there is no place to save the screenshot` | The session's working directory has no `__agent_*` replica root above it, for example an ad-hoc shell outside a room. | Run the capture from a session that lives inside a room replica. |
| `No monitors were found to capture` | The monitor enumeration returned nothing. | Confirm a display is attached and active, then retry. |
| `saved file escaped the replica root` | The destination resolved outside the replica root between validation and write. | Do not link or redirect the replica root. Retry from a plain directory. |

AgentsCommander also refuses a replica root that is a link or reparse point, and validates the replica's identity before writing. A replica that fails those checks reports a failure instead of writing outside its owner.

## See also

- [Window capture](window-capture.md)
- [Terminal snapshots](terminal-snapshots.md)
- [Settings reference](../reference/settings.md#window--ui)
