# Keyboard shortcuts

For developers who want the complete list of key combinations AgentsCommander binds, and where each one works. There are four, and three of them only work while an AC window has focus.

## Window shortcuts

| Shortcut | What it does | Where it works |
|---|---|---|
| `Ctrl+Shift+W` | Closes the currently selected session. | Any AC window with focus. |
| `Ctrl+Shift+R` | Toggles voice capture on the selected session. Live sessions only. | Any AC window with focus. |
| `Ctrl+Shift+E` (default) | Toggles the compact sidebar. Configurable. | Any AC window with focus. |

The Close Session menu item and both close-session tooltips display `Ctrl+Shift+W`. Each of those controls closes the session it belongs to, which is not always the selected one.

Both shortcuts act on **the current selection**, not on the window you are looking at. `Ctrl+Shift+R` does nothing when the selection is not a live session.

## The compact sidebar shortcut

The compact sidebar toggle defaults to `Ctrl+Shift+E`. Change it in Settings: focus the **Compact sidebar hotkey** field and press the new combination. `Escape` in that field resets it to the default. The accepted range is `Ctrl+Shift+<A-Z>`, excluding `W`, `R`, `C` and `V`, which the shipped shortcuts and terminal copy/paste already use. Digits are never accepted.

It matches the **physical** key, not the character it types, so it works on Cyrillic, Greek, Dvorak and AZERTY layouts. The letter shown in Settings is the US legend of that key, which may differ from your keycap on a non-US layout. In a terminal the key is held back from the shell, so it never types into the session.

Honest limits:

- Windows or a third-party tool may intercept the combination before AC sees it.
- An IME composition in progress suppresses it. A held key does not repeat the toggle.
- Where the physical key reports one of the reserved letters on your layout (for example `Z` on AZERTY types `w`), the shipped `Ctrl+Shift+W/R/C/V` action wins and the sidebar does not toggle.

While the sidebar is compact, a voice recording or voice auto-execute countdown already in flight **keeps running**. Nothing is auto-cancelled and the sidebar does not auto-expand for it. The recording indicator and its two cancel buttons are hidden and unreachable while compact. The way back is one click on the full-row toggle or one press of the configured shortcut. `Ctrl+Shift+R` keeps working, because no shipped binding changes.

## The global screenshot hotkey

The screenshot capture hotkey is **the only OS-global shortcut AgentsCommander registers**. It fires whether or not an AC window has focus, which is the point: you press it while looking at the thing you want to capture.

It is configurable, and it is available on Windows, macOS (Control key, not Command) and Linux/X11 (not Wayland). See [Configure the hotkey](../features/screenshot-capture.md#configure-the-hotkey) for the accepted key combinations, how to change it, and how to check that the registration succeeded.

## Scope

The three shortcuts in the table above are **document-level listeners**, registered on the page inside each AC window. They are active only while an AC window has focus.

They are not OS-global hotkeys. Pressing `Ctrl+Shift+W` in another application closes nothing in AC, and the combinations do not conflict with whatever those applications bind to the same keys.

All three are registered once even when a single page hosts more than one AC surface, so a browser build that shows the sidebar and the terminal together does not run any handler twice.

## See also

- [Screenshot capture](../features/screenshot-capture.md) - the global hotkey, its configuration and its failure modes
- [Voice-to-text](../integrations/voice.md) - what `Ctrl+Shift+R` starts and how to cancel a recording
- [Session auto-close](../features/session-auto-close.md) - the other way a session closes
