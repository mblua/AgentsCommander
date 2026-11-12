# Screenshot capture

In-app screenshot capture lets you press a global hotkey, drag a rectangle over the frozen screen, and save a PNG inside the room replica that owns the session you are working with, with the saved path already on your clipboard.

Use this feature when you want to hand a coding agent a picture of what you are looking at. It is not a terminal snapshot (that reads a backend terminal viewport without touching OS pixels) and not [window capture](window-capture.md) (that captures exactly one native window from the CLI or the API). Screenshot capture works on Windows, macOS, Linux under X11, and Linux under Wayland through xdg-desktop-portal. On Wayland the desktop's own area selector replaces the AgentsCommander overlay. Every other target compiles a stub that reports the feature as unsupported and never registers the hotkey.

## Before you start

You need:

- AgentsCommander running on Windows, on macOS, or on Linux in an X11 or Wayland session; on Wayland you also need your desktop's portal backend (on Hyprland, `xdg-desktop-portal-hyprland`); see [Linux: X11 and Wayland](#linux-x11-and-wayland) to check which session you are in;
- on macOS, the Screen Recording permission, which macOS asks for on the first capture; see [macOS: Screen Recording permission](#macos-screen-recording-permission);
- a session selected in the app and displayable, because the screenshot belongs to that session; and
- that session's working directory inside a room replica (an `__agent_*` directory), because the replica root is the destination.

A screenshot records whatever is on your monitors, including passwords, tokens, source code, and personal data. AgentsCommander saves the exact cropped pixels with no redaction, and it copies the file path to your clipboard.

## Capture a screenshot

1. Select the session the screenshot belongs to.
2. Press the capture hotkey (`Ctrl+Q` by default; on macOS, `Ctrl` is the Control key, not Command), or click the camera chip in the titlebar. Clicking the chip captures exactly like the key.
   AgentsCommander captures every monitor and opens one overlay window per monitor. Each overlay shows a frozen image of that monitor, not a live view, so the screen you are photographing cannot change under you.
3. Drag a rectangle over the area you want. A magnifier follows the pointer while you hover and while you drag, so you can place the edges precisely.
4. Release the pointer.
   AgentsCommander crops the frozen image, writes the PNG, copies its path to the clipboard, and closes every overlay. A success toast reads `Screenshot saved. Path copied: <path>` and clears itself after 30 seconds.

To abandon the capture, press `Escape` or close the overlay. Nothing is written.

Two behaviors are deliberate and are not failures:

- Releasing on a selection thinner than 2 pixels in either direction discards that selection and leaves the overlay open, so you can drag again. Both the width and the height must reach 2 pixels, so a 1 by 500 pixel drag is rejected.
- Pressing the hotkey while a capture is already in flight does nothing on Windows: the second press is ignored so you cannot photograph your own overlays or start two captures at once. On Linux X11 and macOS a second press cancels the capture instead; on Wayland a second press (or chip click) does nothing while the desktop's selector is open. See [Linux X11: two behaviors that differ from Windows](#linux-x11-two-behaviors-that-differ-from-windows) and [macOS: behaviors that differ from Windows](#macos-behaviors-that-differ-from-windows).

## Where the file goes

AgentsCommander walks up from the active session's working directory to its `__agent_*` replica root and writes the file directly in that root:

```text
Windows:  <room-replica-root>\agentscommander-screenshot-<YYYYMMDD>-<HHMMSS>-<session-id-prefix>.png
Linux:    <room-replica-root>/agentscommander-screenshot-<YYYYMMDD>-<HHMMSS>-<session-id-prefix>.png
macOS:    <room-replica-root>/agentscommander-screenshot-<YYYYMMDD>-<HHMMSS>-<session-id-prefix>.png
```

The timestamp is local time. The last segment is the first 8 hexadecimal characters of the session id, so two sessions capturing in the same second still get distinct names.

The saved path is copied to the clipboard as text. Copying is best effort: if the clipboard is unavailable, AgentsCommander logs a warning and the capture still succeeds.

The app log records one line per successful capture:

```text
[screenshot] saved <path> for session '<session-name>'
```

If a write fails after the file is created, AgentsCommander deletes the partial file, closes the overlays, and reports the failure.

## Linux: X11 and Wayland

Screenshot capture works in both session types, in different ways:

- **X11** uses the flow described above: frozen per-monitor overlays, the magnifier, and the global hotkey.
- **Wayland** goes through xdg-desktop-portal. It needs your desktop's portal backend, with the Screenshot and GlobalShortcuts portals (on Hyprland, `xdg-desktop-portal-hyprland`).

To check which session you are in, run:

```bash
echo $XDG_SESSION_TYPE
```

The command prints what your shell inherited: usually `x11` or `wayland`, but it can be empty or hold another value, and a graphical session can run without it. Treat the output as a hint, not the decision: the value is inherited and can be stale. AgentsCommander makes its own detection from more than this variable, and its verdict decides.

### Capture on Wayland

When you press the key or click the chip, the desktop's own area selector opens. There is no AgentsCommander overlay and no magnifier.

- Drag a rectangle to save. The PNG goes to the same replica folder as on other systems, the path is copied to the clipboard, and the same success toast appears.
- Press `Escape` to cancel. Nothing is written and no message appears.
- A second press, or a chip click, while the selector is open does nothing.
- If the selector never answers, restart AgentsCommander.
- Some portals also keep their own copy of the image (GNOME, for example, in `~/Pictures`). AgentsCommander never deletes it.

The path is copied to the clipboard. Whether it survives after AgentsCommander quits was not verified on Wayland.

### The shortcut on Wayland

AgentsCommander registers one portal shortcut per instance, with its own id `agentscommander:screenshot-<8 hex>`. Each AgentsCommander configuration directory has its own id. On KDE and GNOME the desktop asks you for the key the first time AgentsCommander registers the shortcut, and owns that key after that. Hyprland needs a key binding instead; see [Wayland: the key binding on Hyprland](#wayland-the-key-binding-on-hyprland).

## Linux X11: two behaviors that differ from Windows

**The copied path may not survive closing AgentsCommander.** On X11 the clipboard is usually served by the running application, so when AgentsCommander exits the path is normally gone. A clipboard manager can change that: if one is running (KDE's Klipper, for example), it can take over the clipboard before the app exits and keep the path available. Without one, expect to lose it. The PNG file is unaffected either way. Paste the path before you quit. This is how X11 works, not a defect.

**Pressing the hotkey a second time cancels the capture.** On Windows a second press while a capture is in flight is ignored. On Linux it closes the overlays and cancels the capture, because a window manager may decline to give the overlay keyboard focus, which would leave `Escape` undelivered. So on Linux you always have a way out: `Escape`, or the hotkey again.

## macOS: Screen Recording permission

macOS grants applications other windows' pixels only with the Screen Recording permission; without it a capture returns the wallpaper alone. AgentsCommander checks the permission before capturing, asks macOS for it on the first capture, and shows this message:

```text
Screenshot capture needs the Screen Recording permission. Allow AgentsCommander in System Settings > Privacy & Security > Screen Recording, then quit and reopen AgentsCommander.
```

Nothing is captured, no overlay opens, and nothing is written. To grant the permission:

1. Open System Settings > Privacy & Security > Screen Recording.
2. Turn on AgentsCommander in the list.
3. Quit AgentsCommander and open it again. The grant takes effect only after a restart.
4. Press the capture hotkey again.

Builds are unsigned, so an update can require the grant again: macOS can treat the new binary as a different app. If a capture fails again after an update, remove the AgentsCommander entry in System Settings > Privacy & Security > Screen Recording, grant it again on the next capture, quit, and reopen. Whether the grant survives an update was not verified on a real Mac.

If you start AgentsCommander from a terminal instead of Finder, macOS may ask for the terminal app instead of AgentsCommander. That was not verified on a real Mac.

## macOS: behaviors that differ from Windows

**Pressing the hotkey a second time cancels the capture.** As on Linux X11, a second press while a capture is in flight closes the overlays and cancels the capture. macOS does not guarantee keyboard focus for the overlay, so `Escape` may not reach it; the hotkey always does. Only Windows ignores the second press.

**The copied path survives quitting AgentsCommander.** macOS keeps clipboard contents after an application quits, so the path stays pasteable; on X11 it is usually lost. This was not verified on a real Mac.

**Two behaviors remain unverified on a real Mac:** whether the menu bar and the Dock draw above the overlay, and whether the overlay appears over an app that is fullscreen in its own Space.

## Wayland: the key binding on Hyprland

Hyprland binds the portal shortcut only through a key binding in its config. AgentsCommander can add it for you, or tell you the line to paste.

### Let AgentsCommander add it

Turn on **Add the screenshot shortcut to Hyprland for me** in Settings > Hotkeys (`screenshotHyprlandBind`, off by default). It works only with the Omarchy layout: a regular `~/.config/hypr/bindings.lua` loaded by `hyprland.lua`.

AgentsCommander then writes its own marked block in `bindings.lua`. For an instance with the token `1a2b3c4d` and the key `Ctrl+Q`:

```lua
-- >>> AgentsCommander screenshot shortcut 1a2b3c4d (managed by AgentsCommander) >>>
hl.bind("CTRL + Q", hl.dsp.global("agentscommander:screenshot-1a2b3c4d"), { description = "AgentsCommander screenshot 1a2b3c4d" })
-- <<< AgentsCommander screenshot shortcut 1a2b3c4d <<<
```

- When you change the key, AgentsCommander updates the block.
- When you turn the setting off, it removes the block.
- After each write it checks `hyprctl configerrors`. If Hyprland reports a new error, AgentsCommander restores the file.
- It touches only its own block. Blocks written by other AgentsCommander instances stay as they are.

### Paste the line yourself

When the setting is off, or your config is not the Omarchy layout, one sticky toast shows the line to paste, in both forms:

```text
hl.bind("CTRL + Q", hl.dsp.global("agentscommander:screenshot-1a2b3c4d"), { description = "AgentsCommander screenshot 1a2b3c4d" })
bindd = CTRL, Q, AgentsCommander screenshot 1a2b3c4d, global, agentscommander:screenshot-1a2b3c4d
```

The first line is for `bindings.lua`, the second for `hyprland.conf`. Copy the line from your own toast: the token is different for each instance. AgentsCommander finds a pasted line through `hyprctl binds` by its description, so keep the `description` as it is.

### The key belongs to the whole desktop

While the binding is in place, Hyprland takes that key for the whole desktop: other applications no longer receive it (for example `Ctrl+Q`). To give the key back, pick another key or turn the setting off.

## Configure the hotkey

Change the shortcut from the Settings dialog. The dialog validates the field before it saves anything:

| Message | Cause |
|---|---|
| `Screenshot hotkey is required` | The field is empty. |
| `Screenshot hotkey must look like Ctrl+Q` | The value does not match the expected shape. |

The same value lives in `settings.30.instance.no-git.json`:

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
- **The syntax is valid but the operating system refuses the combination**, usually because another application already owns it. The save succeeds and the new value is persisted; only the registration fails. AgentsCommander raises an error toast reading `Screenshot hotkey was saved but could not be registered: <error>`, and keeps the previously registered shortcut alive as a fallback. So the configured value is the new combination, the key that actually fires is still the old one, and the status for the configured value reports it as not registered. Pick another combination and save again.

On Wayland the save succeeds and the result arrives a moment later. The `Screenshot hotkey was saved but could not be registered: <error>` toast does not appear on Wayland. A failure shows `Screenshot hotkey <key> is not active: <reason>` instead, once per distinct reason. On Hyprland with **Add the screenshot shortcut to Hyprland for me** on, when the new block cannot be written, or Hyprland rejects it or does not load it, `bindings.lua` keeps (or gets back) the previous block. The previous key then keeps capturing until you fix the problem.

## Check that the shortcut is active

The titlebar always shows a camera chip. When the shortcut is active, the chip also shows the key, for example `Ctrl + Q`, and its tooltip and accessible name are `Take a screenshot (<hotkey>)`. Clicking the chip captures, like the key.

**A chip with only the camera means the shortcut is not active.** Its tooltip and accessible name are then `Take a screenshot`, and clicking it still captures. The most common cause is another application already owning that combination at the OS level. AgentsCommander raises an error toast reading `Screenshot hotkey <configured> is not active: <error>` once per distinct reason, not on every Settings save, so a conflict reaches you even if you never look at the titlebar. Pick a different combination in Settings and save; the key appears in the chip without a restart.

Registration is also written to the app log:

```text
[screenshot] global hotkey registered 'Ctrl+Q'
[screenshot] global hotkey registration failed: <error>
```

On Wayland, two more lines report the result:

```text
[screenshot] Wayland shortcut active '<key>'
[screenshot] Wayland shortcut not active: <error>
```

On Wayland, the `global hotkey registered` line only means the request to the portal started.

## Availability right after startup

The shortcut is active as soon as the app finishes starting, without waiting for session restore. It is not active at process start: the global hotkey is registered on the main thread, so the registration completes only once the app's event loop is running. This is by design. Forcing that work to complete earlier would block the main thread, which is exactly what starves the WebView and prevents dialogs such as the update prompt from rendering.

On Wayland the shortcut status arrives a moment after startup, once the portal answers.

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

### Wayland and Hyprland messages

These messages appear as the reason in `Screenshot hotkey <key> is not active: <reason>`.

| Message | Cause | Recovery |
|---|---|---|
| `Screenshot capture on Wayland needs xdg-desktop-portal with the Screenshot and GlobalShortcuts portals, and the screenshot shortcut could not be registered there. Install your desktop's portal backend (on Hyprland: xdg-desktop-portal-hyprland) and restart AgentsCommander, or log in with an Xorg session. Details: <error>` | The portal is unreachable, or it would not bind the shortcut. | Install or fix the portal backend, then restart AgentsCommander. A failed portal host registration is kept until AgentsCommander or the desktop portal restarts. |
| `The desktop portal stopped the screenshot shortcut. Save Settings or restart AgentsCommander to bind it again.` | The portal restarted or closed the session. | Save Settings once. That binds the key again, also after a portal restart. |
| `The AgentsCommander lines in bindings.lua are damaged; delete them and save Settings again.` | The block markers in `bindings.lua` are broken. | Delete the AgentsCommander lines and save Settings. |
| `bindings.lua changed while AgentsCommander was updating it; nothing was written.` | Something else edited the file during the update. | Save Settings again. |
| `Hyprland rejected the AgentsCommander binding (<error>); bindings.lua was restored.` | Hyprland reported a new config error after the write. | Fix the error shown, then save Settings again. |
| `Hyprland did not load the AgentsCommander binding from bindings.lua; the change was undone.` | The block was written but Hyprland did not load it. | Check your Hyprland config, then save Settings again. |
| ``bindings.lua has the AgentsCommander binding, but Hyprland has not loaded it. Fix the errors that `hyprctl configerrors` lists, then save Settings again.`` | The block is present but not loaded. | Fix what `hyprctl configerrors` lists, then save Settings. |

When the automatic binding fails, the toast also shows the line to paste instead.

## See also

- [Window capture](window-capture.md)
- [Terminal snapshots](terminal-snapshots.md)
- [Settings reference](../reference/settings.md#window--ui)
