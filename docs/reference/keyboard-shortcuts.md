# Keyboard shortcuts

For developers who want the complete list of key combinations AgentsCommander binds, and where each one works. There are three, and two of them only work while an AC window has focus.

## Window shortcuts

| Shortcut | What it does | Where it works |
|---|---|---|
| `Ctrl+Shift+W` | Closes the currently selected session. | Any AC window with focus. |
| `Ctrl+Shift+R` | Toggles voice capture on the selected session. Live sessions only. | Any AC window with focus. |

The Close Session menu item and both close-session tooltips display `Ctrl+Shift+W`. Each of those controls closes the session it belongs to, which is not always the selected one.

Both shortcuts act on **the current selection**, not on the window you are looking at. `Ctrl+Shift+R` does nothing when the selection is not a live session.

## The global screenshot hotkey

The screenshot capture hotkey is **the only OS-global shortcut AgentsCommander registers**. It fires whether or not an AC window has focus, which is the point: you press it while looking at the thing you want to capture.

It is configurable, and it is Windows-only. See [Configure the hotkey](../features/screenshot-capture.md#configure-the-hotkey) for the accepted key combinations, how to change it, and how to check that the registration succeeded.

## Scope

The two shortcuts in the table above are **document-level listeners**, registered on the page inside each AC window. They are active only while an AC window has focus.

They are not OS-global hotkeys. Pressing `Ctrl+Shift+W` in another application closes nothing in AC, and the combinations do not conflict with whatever those applications bind to the same keys.

Both are registered once even when a single page hosts more than one AC surface, so a browser build that shows the sidebar and the terminal together does not run either handler twice.

## See also

- [Screenshot capture](../features/screenshot-capture.md) - the global hotkey, its configuration and its failure modes
- [Voice-to-text](../integrations/voice.md) - what `Ctrl+Shift+R` starts and how to cancel a recording
- [Session auto-close](../features/session-auto-close.md) - the other way a session closes
