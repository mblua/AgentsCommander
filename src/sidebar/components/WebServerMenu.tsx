import { Component, For, Show, createEffect, createMemo, createSignal, onMount } from "solid-js";
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import type {
  AppSettings,
  WebServerInterfaceInfo,
  WebServerOwnedStatus,
} from "../../shared/types";

interface WebServerMenuProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const PORT_MIN = 1;
const PORT_MAX = 65535;
const RESTART_POLL_DELAY_MS = 100;
const RESTART_POLL_ATTEMPTS = 15;

const ALL_INTERFACES = "0.0.0.0";
const LOCALHOST = "127.0.0.1";
// #1453 - strict IPv4 shape (0-255 per octet), the same regex as the approved
// prototype. It rejects leading zeros exactly like Rust's parser, so a string
// that passes is already canonical and safe to cross-check against the
// canonical addresses reported by list_web_server_interfaces.
const IPV4_RE = /^(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/;

const sleep = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms));

const errorMessage = (err: unknown): string =>
  err instanceof Error ? err.message : String(err);

interface BindOptionProps {
  address: string;
  label: string;
  iface: string;
  note?: string;
  noteClass?: string;
  nested?: boolean;
  selected: boolean;
  disabled: boolean;
  onSelect: (address: string) => void;
}

/** #1453 - one selectable address row of the bind chooser. Presets, detected
 *  adapters and the revealed virtual adapters all share this markup. */
const BindOption: Component<BindOptionProps> = (props) => (
  <button
    class={`webserver-bind-option${props.selected ? " selected" : ""}${props.nested ? " nested" : ""}`}
    onClick={() => props.onSelect(props.address)}
    disabled={props.disabled}
    data-ac-testid="titlebar.webserver.addrOption"
    data-addr={props.address}
  >
    <span class="webserver-bind-radio">{props.selected ? "●" : "○"}</span>
    <span>
      <span class="webserver-bind-main">
        <span class="webserver-bind-addr">{props.label}</span>
        <span class="webserver-bind-iface" title={props.iface}>
          {props.iface}
        </span>
      </span>
      <Show when={props.note}>
        <span class={`webserver-bind-note${props.noteClass ? ` ${props.noteClass}` : ""}`}>
          {props.note}
        </span>
      </Show>
    </span>
  </button>
);

const WebServerMenu: Component<WebServerMenuProps> = (props) => {
  const [settings, setSettings] = createSignal<AppSettings | null>(null);
  const [status, setStatus] = createSignal<WebServerOwnedStatus | null>(null);
  const [statusUnavailable, setStatusUnavailable] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [editingPort, setEditingPort] = createSignal(false);
  const [portDraft, setPortDraft] = createSignal("");
  const [interfaces, setInterfaces] = createSignal<WebServerInterfaceInfo[] | null>(null);
  const [editingAddr, setEditingAddr] = createSignal(false);
  const [addrDraft, setAddrDraft] = createSignal("");
  // #1453 - user override of the collapsed virtual group; null = untouched.
  const [virtualExpandedOverride, setVirtualExpandedOverride] =
    createSignal<boolean | null>(null);

  const configuredPort = createMemo(() => settings()?.webServerPort ?? 0);
  const configuredBind = createMemo(() => settings()?.webServerBind || "127.0.0.1");
  const webServerEnabled = createMemo(() => settings()?.webServerEnabled ?? false);
  const baseUrl = createMemo(() => `http://${configuredBind()}:${configuredPort()}`);
  const running = createMemo(() => status()?.listening ?? false);
  const ownedRunning = createMemo(() => status()?.owned ?? false);
  const ownershipAmbiguous = createMemo(() =>
    statusUnavailable() || status()?.externalListening === true
  );
  const canOpenBrowser = createMemo(() =>
    webServerEnabled() && status()?.openAllowed === true
  );

  const bindFailure = createMemo(() => status()?.bindFailure ?? null);
  const physicalInterfaces = createMemo(() => (interfaces() ?? []).filter((i) => !i.isVirtual));
  const virtualInterfaces = createMemo(() => (interfaces() ?? []).filter((i) => i.isVirtual));
  // #1453 - if the stored bind IS one of the virtual addresses the group must
  // open expanded, or the chooser would hide the only selected row. A memo and
  // not a signal: the list resolves asynchronously and the chooser can open
  // before it arrives, so this has to re-evaluate when the list lands.
  const virtualExpanded = createMemo(
    () =>
      virtualExpandedOverride() ??
      virtualInterfaces().some((i) => i.address === configuredBind())
  );
  const bindIsPreset = createMemo(
    () => configuredBind() === ALL_INTERFACES || configuredBind() === LOCALHOST
  );
  // #1453 - an empty list is ABSENCE OF EVIDENCE, not evidence of absence. It
  // covers the failed fetch (null) and the successful fetch with zero rows
  // ([], a machine whose only addresses are loopback and link-local) alike.
  // Every memo that ASSERTS something about availability gates on this.
  const hasDetection = createMemo(() => (interfaces()?.length ?? 0) > 0);
  const storedUnavailable = createMemo(
    () =>
      hasDetection() &&
      !bindIsPreset() &&
      IPV4_RE.test(configuredBind()) &&
      !interfaces()!.some((i) => i.address === configuredBind())
  );
  // Decides whether a row is rendered; asserts nothing. Deliberately over
  // `interfaces() ?? []`, because with no list the Stored row is the only
  // representation of the persisted value and must still appear.
  const storedCustom = createMemo(
    () => !bindIsPreset() && !(interfaces() ?? []).some((i) => i.address === configuredBind())
  );
  const addrDraftValid = createMemo(() => IPV4_RE.test(addrDraft().trim()));
  const addrDraftUndetected = createMemo(() => {
    const draft = addrDraft().trim();
    return (
      addrDraftValid() &&
      hasDetection() &&
      draft !== ALL_INTERFACES &&
      draft !== LOCALHOST &&
      !interfaces()!.some((i) => i.address === draft)
    );
  });
  const showBindAlert = createMemo(
    () => bindFailure() !== null && !ownedRunning() && status()?.externalListening !== true
  );
  // #1453 - the IPV4_RE gate is mandatory. Without it every InvalidAddr failure
  // (whose bind, by construction, is not an address) would print the confident
  // "Address no longer on this machine" headline over a value that is not an
  // address at all.
  const alertAddressMissing = createMemo(() => {
    const failure = bindFailure();
    return (
      failure !== null &&
      hasDetection() &&
      IPV4_RE.test(failure.bind) &&
      failure.bind !== ALL_INTERFACES &&
      failure.bind !== LOCALHOST &&
      !interfaces()!.some((i) => i.address === failure.bind)
    );
  });

  const buttonTitle = createMemo(() => {
    if (ownedRunning()) return `Web server running on port ${configuredPort()}`;
    if (status()?.externalListening) return `Port ${configuredPort()} is in use`;
    if (statusUnavailable()) return "Web server status unavailable";
    if (bindFailure()) return "Web server bind failed";
    return "Web server stopped";
  });
  const buttonState = createMemo(() =>
    statusUnavailable()
      ? "unknown"
      : ownedRunning()
        ? "running"
        : status()?.externalListening
          ? "ambiguous"
          : bindFailure()
            ? "ambiguous"
            : "stopped"
  );
  const statusLabel = createMemo(() => {
    if (ownedRunning()) return "Running";
    if (status()?.externalListening) return "Port in use";
    if (statusUnavailable()) return "Unknown";
    if (bindFailure()) return "Stopped · bind failed";
    return running() ? "Listening" : "Stopped";
  });

  const parsePortDraft = (): number | null => {
    const text = portDraft().trim();
    if (!/^\d+$/.test(text)) return null;
    const value = Number(text);
    if (!Number.isInteger(value) || value < PORT_MIN || value > PORT_MAX) return null;
    return value;
  };

  const loadOwnedStatus = async (): Promise<WebServerOwnedStatus | null> => {
    try {
      const nextStatus = await SettingsAPI.getWebServerOwnedStatus();
      setStatusUnavailable(false);
      setStatus(nextStatus);
      return nextStatus;
    } catch {
      setStatusUnavailable(true);
      setStatus(null);
      return null;
    }
  };

  // #1453 - defensive only; production cannot reach the failing branch. This
  // menu renders solely in the desktop titlebar (Titlebar.tsx, under `isTauri`,
  // and the served page passes `embedded` so it gets no titlebar at all), so a
  // browser never calls this. The catch stays because the WS bridge routes no
  // web server commands, so any future non-Tauri render site would throw here.
  // It is NOT observable browser behaviour: there is no chooser in a browser to
  // degrade, and documenting it as one is exactly the false claim #1453 removed
  // from docs/features/remote-web-ui.md.
  const loadInterfaces = async () => {
    try {
      setInterfaces(await SettingsAPI.listWebServerInterfaces());
    } catch {
      setInterfaces(null);
    }
  };

  const refreshState = async () => {
    const [nextSettings] = await Promise.all([
      SettingsAPI.get(),
      loadOwnedStatus(),
      loadInterfaces(),
    ]);
    setSettings(nextSettings);
    setPortDraft(String(nextSettings.webServerPort));
  };

  const saveWebSettings = async (
    patch: Partial<Pick<AppSettings, "webServerEnabled" | "webServerPort" | "webServerBind">>
  ) => {
    const latest = await SettingsAPI.get();
    await SettingsAPI.saveDraft({ ...latest, ...patch });
    void settingsStore.refresh();
  };

  const waitForOwnedStatus = async (
    predicate: (nextStatus: WebServerOwnedStatus) => boolean
  ): Promise<WebServerOwnedStatus | null> => {
    for (let i = 0; i < RESTART_POLL_ATTEMPTS; i += 1) {
      const nextStatus = await loadOwnedStatus();
      if (nextStatus && predicate(nextStatus)) return nextStatus;
      await sleep(RESTART_POLL_DELAY_MS);
    }
    return status();
  };

  const startAndWaitForRunning = async (failureMessage: string): Promise<boolean> => {
    const started = await SettingsAPI.startWebServer();
    const observed = await waitForOwnedStatus((nextStatus) =>
      nextStatus.owned || nextStatus.externalListening
    );

    if (started && observed?.owned) return true;

    if (observed?.externalListening) {
      setError("Port is already in use");
    } else {
      setError(failureMessage);
    }
    return false;
  };

  const stopAndWaitForStopped = async (): Promise<boolean> => {
    await SettingsAPI.stopWebServer();
    const observed = await waitForOwnedStatus((nextStatus) =>
      !nextStatus.owned && !nextStatus.listening
    );
    if (observed && !observed.listening && !observed.owned) return true;

    setError("Port is still in use");
    return false;
  };

  const restartServer = async (failureMessage: string): Promise<boolean> => {
    if (!(await stopAndWaitForStopped())) return false;
    return startAndWaitForRunning(failureMessage);
  };

  const runExclusive = async (action: () => Promise<void>) => {
    if (busy()) return;
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (err) {
      setError(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleToggle = () => {
    void runExclusive(async () => {
      if (ownedRunning()) {
        const stopped = await stopAndWaitForStopped();
        await saveWebSettings({ webServerEnabled: false });
        await refreshState();
        if (!stopped) setError("Port is still in use");
        return;
      }

      // #1453 - persist the intent only once a start actually converged, so a
      // visibly failed click cannot leave the server enabled for every future
      // launch. Safe because start_web_server reads only bind and port, never
      // the enabled flag, so writing it afterwards cannot block the start.
      const started = await startAndWaitForRunning("Server did not start");
      if (started) await saveWebSettings({ webServerEnabled: true });
      await refreshState();
    });
  };

  // #1453 - the single path for presets, detected rows and manual entry.
  const applyBind = (nextBind: string) => {
    void runExclusive(async () => {
      const wasOwnedRunning = ownedRunning();
      const shouldStart = !wasOwnedRunning && webServerEnabled();
      await saveWebSettings({ webServerBind: nextBind });
      // Collapse ONLY on convergence. The helpers do not throw on failure: they
      // set error() and return false. Closing the chooser after a failure would
      // take away the very screen the user needs to pick another address.
      let converged = true;
      if (wasOwnedRunning) {
        converged = await restartServer("Server did not restart");
      } else if (shouldStart) {
        converged = await startAndWaitForRunning("Server did not start");
      }
      await refreshState();
      if (converged) {
        setEditingAddr(false);
        setAddrDraft("");
      }
    });
  };

  const handleRestart = () => {
    if (busy() || !ownedRunning()) return;
    void runExclusive(async () => {
      await restartServer("Server did not restart");
      await refreshState();
    });
  };

  const handleSavePort = () => {
    const nextPort = parsePortDraft();
    if (nextPort === null) {
      setError("Port must be 1 to 65535");
      return;
    }

    const wasOwnedRunning = ownedRunning();
    void runExclusive(async () => {
      await saveWebSettings({ webServerPort: nextPort });
      const saved = !wasOwnedRunning || (await restartServer("Server did not restart"));
      await refreshState();
      if (saved) setEditingPort(false);
    });
  };

  const handleOpenBrowser = () => {
    if (busy() || !canOpenBrowser()) return;
    void runExclusive(async () => {
      if (!canOpenBrowser()) return;
      await SettingsAPI.openWebRemote();
    });
  };

  onMount(() => {
    void refreshState();
  });

  createEffect(() => {
    if (!props.open) {
      setEditingPort(false);
      setEditingAddr(false);
      // null, not false: false would freeze the group collapsed even when the
      // stored bind is one of the virtual addresses.
      setVirtualExpandedOverride(null);
      setAddrDraft("");
    }
  });

  return (
    <div class="webserver-menu-wrapper">
      <button
        class={`titlebar-btn titlebar-btn-webserver ${props.open ? "open" : ""}`}
        onClick={(event) => {
          event.stopPropagation();
          const nextOpen = !props.open;
          props.onOpenChange(nextOpen);
          if (nextOpen) void refreshState();
        }}
        title={buttonTitle()}
        data-ac-testid="titlebar.webserver.button"
        data-ac-state={buttonState()}
      >
        <span class="webserver-menu-icon">&#x1F310;</span>
        <span class={`webserver-status-dot ${buttonState()}`} />
      </button>
      <Show when={props.open}>
        <div
          class="webserver-dropdown"
          onClick={(event) => event.stopPropagation()}
          // `on:keydown`, not `onKeyDown`: the delegated form would register a
          // document-level keydown listener, and Titlebar.zoom.test.tsx asserts
          // that no component in the mounted tree does that, because its
          // zoom-listener scrub depends on it. This binds the element directly.
          on:keydown={(event) => {
            // #1453 - this change adds ~10 focusable buttons plus a text input
            // to a popover that only closed on outside click, so keyboard users
            // would otherwise have no way out.
            if (event.key !== "Escape") return;
            event.stopPropagation();
            if (editingAddr()) setEditingAddr(false);
            else props.onOpenChange(false);
          }}
          data-ac-testid="titlebar.webserver.menu"
        >
          <div class="webserver-status-row">
            <span class={`webserver-status-dot inline ${buttonState()}`} />
            <span>{statusLabel()}</span>
          </div>
          <div class="webserver-url">{baseUrl()}</div>

          <Show when={showBindAlert()}>
            <div class="webserver-alert" data-ac-testid="titlebar.webserver.bindAlert">
              <div class="webserver-alert-title">
                <span>{"⚠"}</span>
                <span>
                  {alertAddressMissing()
                    ? "Address no longer on this machine"
                    : "Could not start the web server"}
                </span>
              </div>
              <div class="webserver-alert-body">
                <Show
                  when={alertAddressMissing()}
                  fallback={
                    <>
                      The server could not bind to{" "}
                      <b>
                        {bindFailure()!.bind}:{bindFailure()!.port}
                      </b>
                      . Pick a different address or port, then start again.
                    </>
                  }
                >
                  <b>{bindFailure()!.bind}</b> is not assigned to any adapter, so the server
                  cannot bind to it. Pick a current address to start.
                </Show>
              </div>
              <div class="webserver-alert-detail">{bindFailure()!.detail}</div>
              <div class="webserver-alert-actions">
                <button
                  class="webserver-inline-btn primary"
                  onClick={() => {
                    setEditingPort(false);
                    setEditingAddr(true);
                  }}
                  disabled={busy()}
                  data-ac-testid="titlebar.webserver.bindAlertAction"
                >
                  Change address
                </button>
              </div>
            </div>
          </Show>

          <div class="layout-section-label">Bind</div>
          <div class="webserver-bind-row">
            <span class="webserver-port-label">Addr</span>
            <span
              class={`webserver-bind-value${alertAddressMissing() ? " invalid" : ""}`}
              title={configuredBind()}
            >
              {configuredBind()}
            </span>
            <button
              class={`webserver-inline-btn${editingAddr() ? " primary" : ""}`}
              onClick={() => {
                const nextEditing = !editingAddr();
                // Mutual exclusion with the PORT editor: the same idiom the
                // titlebar already uses for its two dropdowns, and it bounds
                // the worst-case height of the popover.
                if (nextEditing) setEditingPort(false);
                setEditingAddr(nextEditing);
              }}
              disabled={busy()}
              data-ac-testid="titlebar.webserver.editAddr"
            >
              {editingAddr() ? "Close" : "Edit"}
            </button>
          </div>

          <Show when={editingAddr()}>
            <div class="webserver-bind-panel" data-ac-testid="titlebar.webserver.bindPanel">
              <div class="webserver-bind-group">Presets</div>
              <BindOption
                address={LOCALHOST}
                label="Localhost only"
                iface={LOCALHOST}
                note="This machine only."
                selected={configuredBind() === LOCALHOST}
                disabled={busy()}
                onSelect={applyBind}
              />
              <BindOption
                address={ALL_INTERFACES}
                label="All interfaces"
                iface={ALL_INTERFACES}
                note="Any device on your network can reach this server. Survives a DHCP address change."
                noteClass="bad"
                selected={configuredBind() === ALL_INTERFACES}
                disabled={busy()}
                onSelect={applyBind}
              />

              <Show when={physicalInterfaces().length > 0}>
                <div class="webserver-bind-group">Detected on this machine</div>
                <For each={physicalInterfaces()}>
                  {(iface) => (
                    <BindOption
                      address={iface.address}
                      label={iface.address}
                      iface={iface.interfaceName}
                      selected={configuredBind() === iface.address}
                      disabled={busy()}
                      onSelect={applyBind}
                    />
                  )}
                </For>
              </Show>

              <Show when={virtualInterfaces().length > 0}>
                <button
                  class="webserver-bind-group-toggle"
                  onClick={() => setVirtualExpandedOverride(!virtualExpanded())}
                  data-ac-testid="titlebar.webserver.virtualToggle"
                >
                  <span class="webserver-bind-radio">
                    {virtualExpanded() ? "▾" : "▸"}
                  </span>
                  <span class="webserver-bind-main">
                    <span class="webserver-bind-addr">Virtual &amp; tunnel</span>
                    <span class="webserver-bind-iface">({virtualInterfaces().length})</span>
                  </span>
                </button>
                <Show when={virtualExpanded()}>
                  <For each={virtualInterfaces()}>
                    {(iface) => (
                      <BindOption
                        address={iface.address}
                        label={iface.address}
                        iface={iface.interfaceName}
                        nested
                        selected={configuredBind() === iface.address}
                        disabled={busy()}
                        onSelect={applyBind}
                      />
                    )}
                  </For>
                </Show>
              </Show>

              <Show when={storedCustom()}>
                <div class="webserver-bind-group">Stored</div>
                <button
                  class="webserver-bind-option"
                  disabled
                  data-ac-testid="titlebar.webserver.storedRow"
                >
                  <span class="webserver-bind-radio">{"●"}</span>
                  <span>
                    <span class="webserver-bind-main">
                      <span class="webserver-bind-addr">{configuredBind()}</span>
                      <span class="webserver-bind-iface">stored</span>
                    </span>
                    <Show when={storedUnavailable()}>
                      <span class="webserver-bind-note bad">
                        Unavailable &middot; not on this machine
                      </span>
                    </Show>
                  </span>
                </button>
              </Show>

              <div class="webserver-bind-group">Other address</div>
              <div class="webserver-bind-manual">
                <input
                  class={`webserver-port-input${
                    addrDraft().trim() === "" ? "" : addrDraftValid() ? " valid" : " invalid"
                  }`}
                  value={addrDraft()}
                  onInput={(event) => setAddrDraft(event.currentTarget.value)}
                  data-ac-testid="titlebar.webserver.addrInput"
                />
                <button
                  class="webserver-inline-btn"
                  onClick={() => applyBind(addrDraft().trim())}
                  disabled={busy() || !addrDraftValid()}
                  data-ac-testid="titlebar.webserver.addrUse"
                >
                  Use
                </button>
              </div>
              <Show when={addrDraft().trim() !== ""}>
                <div
                  class={`webserver-validation${
                    addrDraftValid() && !addrDraftUndetected() ? " ok" : ""
                  }`}
                >
                  {!addrDraftValid()
                    ? "Not a valid IPv4 address."
                    : addrDraftUndetected()
                      ? "Not detected on this machine. The bind may fail."
                      : "Valid IPv4 address."}
                </div>
              </Show>
            </div>
          </Show>

          <Show when={editingAddr()}>
            <div class="webserver-divider" />
          </Show>

          <div class={editingPort() ? "webserver-port-row" : "webserver-bind-row"}>
            <span class="webserver-port-label">Port</span>
            <span class="webserver-bind-value" hidden={editingPort()}>
              {configuredPort()}
            </span>
            <button
              class="webserver-inline-btn"
              onClick={() => {
                setPortDraft(String(configuredPort()));
                setEditingAddr(false);
                setEditingPort(true);
              }}
              disabled={busy()}
              hidden={editingPort()}
              data-ac-testid="titlebar.webserver.editPort"
            >
              Edit
            </button>
            <input
              class="webserver-port-input"
              value={portDraft()}
              onInput={(event) => setPortDraft(event.currentTarget.value)}
              hidden={!editingPort()}
              data-ac-testid="titlebar.webserver.portInput"
            />
            <button
              class="webserver-inline-btn"
              onClick={handleSavePort}
              disabled={busy()}
              hidden={!editingPort()}
              data-ac-testid="titlebar.webserver.savePort"
            >
              Save
            </button>
          </div>

          <Show
            when={ownedRunning()}
            fallback={
              <Show when={configuredBind() === LOCALHOST && !showBindAlert()}>
                <div class="webserver-bind-hint">
                  Localhost only. Not reachable from other devices.
                </div>
              </Show>
            }
          >
            <div class="webserver-bind-hint">Changing either value restarts the server.</div>
          </Show>

          <Show when={status()?.externalListening}>
            <div class="webserver-error">Port is already in use</div>
          </Show>
          <Show when={ownershipAmbiguous() && statusUnavailable()}>
            <div class="webserver-error">Ownership status unavailable</div>
          </Show>
          {/* #1453 - suppressed while the alert is up: the alert already carries
              the verbatim reason, and two stacked amber blocks in a 236px column
              add noise, not information. */}
          <Show when={error() && !showBindAlert()}>
            <div class="webserver-error" data-ac-testid="titlebar.webserver.error">{error()}</div>
          </Show>

          <div class="layout-section-label">Actions</div>
          <button
            class="layout-option"
            onClick={handleToggle}
            disabled={busy()}
            data-ac-testid="titlebar.webserver.toggle"
          >
            <span class="layout-option-icon">
              {ownedRunning() ? "○" : "●"}
            </span>
            {ownedRunning() ? "Stop Server" : "Start Server"}
          </button>
          <button
            class="layout-option"
            onClick={handleRestart}
            disabled={busy() || !ownedRunning()}
            data-ac-testid="titlebar.webserver.restart"
          >
            <span class="layout-option-icon">&#x21BB;</span>
            Restart Server
          </button>
          <button
            class="layout-option"
            onClick={handleOpenBrowser}
            disabled={busy() || !canOpenBrowser()}
            data-ac-testid="titlebar.webserver.open"
          >
            <span class="layout-option-icon">&#x2197;</span>
            Open in Browser
          </button>
        </div>
      </Show>
    </div>
  );
};

export default WebServerMenu;
