# Remote web UI

For developers who want AgentsCommander in a browser, on the machine itself or from another device on a network they trust. After this page you can start the embedded server, reach it from a second device, and understand exactly what you are exposing when you do.

AC ships an embedded HTTP and WebSocket server. Turn it on and it serves the AgentsCommander interface to a browser: the same sidebar, the same terminals, over a WebSocket transport instead of the desktop IPC. It is **off by default** and bound to `127.0.0.1`.

## What it does

The server is part of the running app, not a separate process. While it runs, a browser pointed at its address gets the live interface for the sessions that app is managing: the same rows, the same terminal output, the same input.

The listener is the same one described by the `webServer*` settings. It is **not** the [control-plane API](control-plane-api.md), which is a separate opt-in listener with its own port, its own bind address and its own tokens. The two do not configure each other, and turning one on does not turn the other on.

One consequence deserves stating before anything else: **terminal content can contain passwords, tokens, source code, prompts and personal data, and the web server performs no automatic redaction.** Anything a session prints is visible to anyone who can reach and authenticate to the listener.

## Turning it on

The globe button in the sidebar titlebar carries a status dot and opens the web server menu. The button's tooltip tells you the state at a glance:

| Tooltip | State |
|---|---|
| `Web server stopped` | Not listening. |
| `Web server running on port <n>` | Listening, and this AC instance owns the listener. |
| `Port <n> is in use` | Something is listening on that port, and it is not this instance. |
| `Web server status unavailable` | AC could not read the status. |

The menu repeats the state as `Running`, `Listening`, `Stopped`, `Port in use` or `Unknown`, shows the base URL it would serve (`http://<bind>:<port>`), and gives you the controls: a toggle that starts or stops the server and writes `webServerEnabled` for you, a restart, a port editor, and an action that opens the served page in your browser.

The port editor validates before saving. A value outside 1 to 65535 is refused with `Port must be 1 to 65535`, and saving a new port restarts a running server so the change takes effect.

**The bind address has no UI.** `webServerBind` is edited in the per-instance `settings.json` while AC is closed. That is deliberate: the bind address is what decides whether anything outside the machine can reach the listener.

## Connecting from another device

Everything you need is already in the settings reference, and it is worth reading before you widen the bind: see [Web Remote Access on a trusted LAN](../reference/settings.md#web-remote-access-on-a-trusted-lan) for the full procedure.

The shape of it:

1. Close AC and set `webServerBind` to the host's real private LAN IPv4 address, plus the port you chose, in the per-instance `settings.json`. A concrete private address is preferred over `0.0.0.0`, which listens on every interface.
2. Restart AC and start the server from the menu.
3. If the host firewall blocks the client, add an inbound rule for that TCP port only, scoped to the Private profile, the selected address and port, and the intended client address or subnet.
4. From the second device, browse to the host and port and authenticate.

Two things the reference is explicit about and this page will not soften. **A firewall rule permits reachability; it does not authenticate a user.** And external access is an opt-in that exposes live terminal content to every party that can reach and authenticate to the listener.

## What the browser UI can do

The served page is **the full AgentsCommander UI**: it renders the same sidebar and terminal components you use on the desktop, side by side with a draggable divider, running over a WebSocket transport rather than the Tauri one. It is the same interface, not a reduced view of your sessions. The session-selection event is pinned by a test to carry the same payload on both transports; other events are not covered by that guarantee, and terminal output travels as a binary frame rather than JSON.

What is not there:

- **No desktop titlebar.** The web view has no native window frame, so the sidebar left/right preset appears as a compact toggle anchored to the terminal pane instead.
- **Only the sidebar and the terminal.** The served page renders those two. AC's separate windows, the Resource Monitor, Watchers, the Spec Board and the Guide, are desktop windows and are not part of the page you are served.

## Authentication

The credential is the per-instance `web-token.txt` file, which AC writes next to the binary. It is **separate** from the CLI `master-token.txt` and separate again from control-plane API client tokens; the three are not interchangeable.

Treat that token, and any URL or browser state carrying it, as a password: use it only with a trusted client, and never commit it or paste it into tickets, chat, logs or screenshots.

Obtain and use it through the existing local Web Remote Access flow. **Do not invent a URL parameter or a token-rotation procedure**; the settings reference says this in as many words, and this page will not describe one it cannot point to.

For the wider picture of what an agent and a remote caller can reach, see [Security model](../security.md).

## Settings

| Key | What it controls |
|---|---|
| `webServerEnabled` | Whether the embedded HTTP and WebSocket server runs. `false` by default. |
| `webServerPort` | The listening port. The default is platform-specific per binary suffix. Editable from the web server menu. |
| `webServerBind` | The bind address. `"127.0.0.1"` by default. No UI: edit it in `settings.json` while AC is closed. |

See [Settings reference](../reference/settings.md#web-server-opt-in) for the field types and the LAN procedure.

## Troubleshooting

**"The menu says `Port in use` and the server will not start."** Another process, often an older AC instance, holds that port. The menu reports `Port is already in use` when a start attempt hits it. Pick a different port in the editor, or stop whatever owns the current one.

**"I stopped the server and the menu says `Port is still in use`."** The stop completed on AC's side but something is still listening on the port. That is the ambiguous case the status dot shows; check for another instance before reusing the port.

**"The menu says `Web server status unavailable`."** AC could not read the listener status at all, so it reports `Unknown` rather than guessing. Reopen the menu to retry.

**"The local listener works and the second device cannot reach it."** Re-check, in this order: the selected private address, the port, the network profile, the firewall remote scope, and that both devices are on the same trusted LAN. A listener bound to `127.0.0.1` is not reachable from any other device, by design.

**"I want to stop exposing this."** Turn the server off from the menu and remove the narrowly scoped firewall allowance you added. Leaving the listener off when you do not need it is the recommendation in the settings reference, not an extra precaution.

## See also

- [Security model](../security.md) - what a remote caller can reach and what is out of scope
- [Control-plane API](control-plane-api.md) - the other opt-in listener, for machine clients
- [Settings reference](../reference/settings.md#web-server-opt-in) - the `webServer*` keys and the LAN procedure
