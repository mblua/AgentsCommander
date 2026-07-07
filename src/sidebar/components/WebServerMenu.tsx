import { Component, Show, createEffect, createMemo, createSignal, onMount } from "solid-js";
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import type { AppSettings, WebServerOwnedStatus } from "../../shared/types";

interface WebServerMenuProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const PORT_MIN = 1;
const PORT_MAX = 65535;
const RESTART_POLL_DELAY_MS = 100;
const RESTART_POLL_ATTEMPTS = 15;

const sleep = (ms: number) => new Promise((resolve) => window.setTimeout(resolve, ms));

const errorMessage = (err: unknown): string =>
  err instanceof Error ? err.message : String(err);

const WebServerMenu: Component<WebServerMenuProps> = (props) => {
  const [settings, setSettings] = createSignal<AppSettings | null>(null);
  const [status, setStatus] = createSignal<WebServerOwnedStatus | null>(null);
  const [statusUnavailable, setStatusUnavailable] = createSignal(false);
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [editingPort, setEditingPort] = createSignal(false);
  const [portDraft, setPortDraft] = createSignal("");

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
  const buttonTitle = createMemo(() => {
    if (ownedRunning()) return `Web server running on port ${configuredPort()}`;
    if (status()?.externalListening) return `Port ${configuredPort()} is in use`;
    if (statusUnavailable()) return "Web server status unavailable";
    return "Web server stopped";
  });
  const buttonState = createMemo(() =>
    statusUnavailable()
      ? "unknown"
      : ownedRunning()
        ? "running"
        : status()?.externalListening
          ? "ambiguous"
          : "stopped"
  );
  const statusLabel = createMemo(() => {
    if (ownedRunning()) return "Running";
    if (status()?.externalListening) return "Port in use";
    if (statusUnavailable()) return "Unknown";
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

  const refreshState = async () => {
    const [nextSettings] = await Promise.all([
      SettingsAPI.get(),
      loadOwnedStatus(),
    ]);
    setSettings(nextSettings);
    setPortDraft(String(nextSettings.webServerPort));
  };

  const saveWebSettings = async (
    patch: Partial<Pick<AppSettings, "webServerEnabled" | "webServerPort">>
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
    const observed = await waitForOwnedStatus((nextStatus) => !nextStatus.owned);
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
      if (ownedRunning() || webServerEnabled()) {
        const stopped = await stopAndWaitForStopped();
        await saveWebSettings({ webServerEnabled: false });
        await refreshState();
        if (!stopped) setError("Port is still in use");
        return;
      }

      await saveWebSettings({ webServerEnabled: true });
      await startAndWaitForRunning("Server did not start");
      await refreshState();
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
    if (!props.open) setEditingPort(false);
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
          data-ac-testid="titlebar.webserver.menu"
        >
          <div class="webserver-status-row">
            <span class={`webserver-status-dot inline ${buttonState()}`} />
            <span>{statusLabel()}</span>
          </div>
          <div class="webserver-url">{baseUrl()}</div>

          <div class="webserver-port-row">
            <span class="webserver-port-label">Port</span>
            <span hidden={editingPort()}>{configuredPort()}</span>
            <button
              class="webserver-inline-btn"
              onClick={() => {
                setPortDraft(String(configuredPort()));
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

          <Show when={status()?.externalListening}>
            <div class="webserver-error">Port is already in use</div>
          </Show>
          <Show when={ownershipAmbiguous() && statusUnavailable()}>
            <div class="webserver-error">Ownership status unavailable</div>
          </Show>
          <Show when={error()}>
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
              {ownedRunning() || webServerEnabled() ? "\u25CB" : "\u25CF"}
            </span>
            {ownedRunning() || webServerEnabled() ? "Stop Server" : "Start Server"}
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
