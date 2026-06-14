import { Component, createSignal, createEffect, For, Show, onMount, onCleanup } from "solid-js";
import { createStore } from "solid-js/store";
import { isTauri } from "../../shared/platform";
import type {
  AppSettings,
  AgentConfig,
  CodingAgentEnv,
  TelegramBotConfig,
  ProfileCellConfig,
} from "../../shared/types";
import { SettingsAPI, TelegramAPI, ReposAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { setSoundsEnabled } from "../../shared/sound";
import { sessionsStore } from "../stores/sessions";
import { AGENT_PRESET_MAP, newAgentId } from "../../shared/agent-presets";
import { mergeSettingsForSavePreservingProjects } from "./settings-save";
import {
  hasEnabledEnvKey,
  isCodexAgent,
  nextAvailableProfileLetter,
  parseArgvText,
  profileDisplayLabel,
  resolveProfilePreview,
  sortedProfileLetters,
  stringifyArgv,
  validateEnvRows,
} from "../../shared/profile-utils";

const GEMINI_MODELS = [
  { id: "gemini-2.5-flash", label: "Gemini 2.5 Flash (recommended)" },
  { id: "gemini-2.5-pro", label: "Gemini 2.5 Pro" },
  { id: "gemini-2.0-flash", label: "Gemini 2.0 Flash" },
  { id: "gemini-1.5-flash", label: "Gemini 1.5 Flash" },
  { id: "gemini-1.5-pro", label: "Gemini 1.5 Pro" },
];


type SettingsTab = "general" | "agents" | "profiles" | "integrations";

const TABS: { key: SettingsTab; label: string }[] = [
  { key: "general", label: "General" },
  { key: "agents", label: "Coding Agents" },
  { key: "profiles", label: "Profiles" },
  { key: "integrations", label: "Integrations" },
];

const isValidSettingsTab = (s: string): s is SettingsTab =>
  TABS.some((t) => t.key === s);

const SettingsModal: Component<{ onClose: () => void; section?: string }> = (props) => {
  const [settings, setSettings] = createStore<{ data: AppSettings | null }>({ data: null });
  const [saving, setSaving] = createSignal(false);
  const [testingBot, setTestingBot] = createSignal<string | null>(null);
  const [testResult, setTestResult] = createSignal<{
    id: string;
    ok: boolean;
    msg?: string;
  } | null>(null);
  // `props.section` lets callers (e.g. disabled mic click) open on a specific
  // tab. Invalid or absent → fall back to "general" default. The effect below
  // also snaps to the requested section when props.section changes while the
  // modal is already mounted (double-click on disabled mic re-targets).
  const initialTab: SettingsTab =
    props.section && isValidSettingsTab(props.section) ? props.section : "general";
  const [activeTab, setActiveTab] = createSignal<SettingsTab>(initialTab);
  createEffect(() => {
    const s = props.section;
    if (s && isValidSettingsTab(s)) setActiveTab(s);
  });

  const [webServerRunning, setWebServerRunning] = createSignal(false);
  const [saveError, setSaveError] = createSignal("");
  const [profileCellText, setProfileCellText] = createStore<Record<string, string>>({});
  const [profileCellErrors, setProfileCellErrors] = createStore<Record<string, string>>({});
  // Snapshot of injectRtkHook captured at modal open. handleSave compares it
  // against the live form value to decide whether to fire sweepRtkHook.
  // updateField is local-only (mutates the form draft), so the sweep only
  // dispatches when the user actually clicks Save and the value changed.
  const [initialInjectRtk, setInitialInjectRtk] = createSignal<boolean | null>(null);
  // Disables the Save button and the rtk checkbox while the per-replica sweep
  // is in flight, preventing a rapid double-Save from queuing two concurrent
  // sweeps with opposite enabled values (silent partial state).
  const [rtkSweepInFlight, setRtkSweepInFlight] = createSignal(false);

  const s = () => settings.data;

  onMount(async () => {
    const [loaded, wsRunning] = await Promise.all([
      SettingsAPI.get(),
      SettingsAPI.getWebServerStatus().catch(() => false),
    ]);
    setSettings("data", loaded);
    setInitialInjectRtk(loaded.injectRtkHook);
    setWebServerRunning(wsRunning);
  });

  // ── Generic field updater ──
  const updateField = <K extends keyof AppSettings>(
    key: K,
    value: AppSettings[K]
  ) => {
    if (!settings.data) return;
    setSettings("data", key as any, value as any);
  };

  // ── Agents ──
  const updateAgent = (
    index: number,
    field: keyof AgentConfig,
    value: string | boolean | string[] | CodingAgentEnv[]
  ) => {
    if (!settings.data) return;
    setSettings("data", "agents", index, field as any, value as any);
  };

  const updateAgentEnv = (
    agentIndex: number,
    rowIndex: number,
    field: keyof CodingAgentEnv,
    value: string | boolean
  ) => {
    if (!settings.data) return;
    setSettings("data", "agents", agentIndex, "envs", rowIndex, field as any, value as any);
  };

  const addAgentEnv = (agentIndex: number) => {
    if (!settings.data) return;
    const row: CodingAgentEnv = {
      key: "",
      value: "",
      source: "user",
      enabled: true,
    };
    setSettings("data", "agents", agentIndex, "envs", (prev) => [...(prev ?? []), row]);
  };

  const removeAgentEnv = (agentIndex: number, rowIndex: number) => {
    if (!settings.data) return;
    setSettings("data", "agents", agentIndex, "envs", (prev) =>
      (prev ?? []).filter((_, i) => i !== rowIndex)
    );
  };

  const emptyProfileCell = (): ProfileCellConfig => ({
    enabled: true,
    argv: [],
    env: {},
    notes: "",
  });

  const profileLetters = () =>
    settings.data ? sortedProfileLetters(settings.data.codingAgentProfiles) : ["A"];

  const profileCellKey = (agentId: string, letter: string) => `${agentId}:${letter}`;

  const profileCell = (agentId: string, letter: string): ProfileCellConfig | null =>
    settings.data?.codingAgentProfiles.matrix[agentId]?.[letter] ?? null;

  const setProfileCell = (
    agentId: string,
    letter: string,
    cell: ProfileCellConfig
  ) => {
    if (!settings.data) return;
    setSettings("data", "codingAgentProfiles", "matrix", (matrix) => ({
      ...matrix,
      [agentId]: {
        ...(matrix[agentId] ?? {}),
        [letter]: cell,
      },
    }));
  };

  const addProfileCell = (agentId: string, letter: string) => {
    setProfileCell(agentId, letter, emptyProfileCell());
    const key = profileCellKey(agentId, letter);
    setProfileCellText(key, "");
    setProfileCellErrors(key, "");
  };

  const removeProfileCell = (agentId: string, letter: string) => {
    if (!settings.data || letter === "A") return;
    const cells = settings.data.codingAgentProfiles.matrix[agentId] ?? {};
    const nextCells = { ...cells };
    delete nextCells[letter];
    setSettings("data", "codingAgentProfiles", "matrix", (matrix) => ({
      ...matrix,
      [agentId]: nextCells,
    }));
    const key = profileCellKey(agentId, letter);
    setProfileCellText((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
    setProfileCellErrors((prev) => {
      const next = { ...prev };
      delete next[key];
      return next;
    });
  };

  const updateProfileName = (letter: string, name: string) => {
    if (!settings.data) return;
    setSettings("data", "codingAgentProfiles", "letters", (letters) => ({
      ...letters,
      [letter]: { name },
    }));
  };

  const addProfileLetter = () => {
    if (!settings.data) return;
    const letter = nextAvailableProfileLetter(settings.data.codingAgentProfiles);
    if (!letter) return;
    setSettings("data", "codingAgentProfiles", "letters", (letters) => ({
      ...letters,
      [letter]: { name: "" },
    }));
  };

  const removeProfileLetter = (letter: string) => {
    if (!settings.data || letter === "A") return;
    const letters = { ...settings.data.codingAgentProfiles.letters };
    delete letters[letter];
    const matrix = Object.fromEntries(
      Object.entries(settings.data.codingAgentProfiles.matrix).map(([agentId, cells]) => {
        const nextCells = { ...cells };
        delete nextCells[letter];
        return [agentId, nextCells];
      })
    );
    setSettings("data", "codingAgentProfiles", "letters", letters);
    setSettings("data", "codingAgentProfiles", "matrix", matrix);
  };

  const updateProfileCellText = (agentId: string, letter: string, text: string) => {
    const key = profileCellKey(agentId, letter);
    setProfileCellText(key, text);
    const parsed = parseArgvText(text);
    if (parsed.error) {
      setProfileCellErrors(key, parsed.error);
      return;
    }
    setProfileCellErrors(key, "");
    const existing = profileCell(agentId, letter) ?? emptyProfileCell();
    setProfileCell(agentId, letter, { ...existing, argv: parsed.argv });
  };

  const displayedProfileCellText = (agentId: string, letter: string): string => {
    const key = profileCellKey(agentId, letter);
    if (Object.prototype.hasOwnProperty.call(profileCellText, key)) {
      return profileCellText[key] ?? "";
    }
    return stringifyArgv(profileCell(agentId, letter)?.argv ?? []);
  };

  const addAgent = (preset?: Omit<AgentConfig, "id">) => {
    if (!settings.data) return;
    const agent: AgentConfig = preset
      ? { id: newAgentId(), ...preset }
      : {
          id: newAgentId(),
          label: "",
          command: "",
          color: "#6366f1",
          gitPullBefore: false,
          excludeGlobalClaudeMd: true,
          envs: [],
          isolateCodexHome: false,
        };
    setSettings("data", "agents", (prev) => [...prev, agent]);
  };

  const removeAgent = (index: number) => {
    if (!settings.data) return;
    setSettings("data", "agents", (prev) => prev.filter((_, i) => i !== index));
  };

  // ── Telegram Bots ──
  const updateBot = (
    index: number,
    field: keyof TelegramBotConfig,
    value: string | number
  ) => {
    if (!settings.data) return;
    setSettings("data", "telegramBots", index, field as any, value as any);
  };

  const addBot = () => {
    if (!settings.data) return;
    const bot: TelegramBotConfig = {
      id: newAgentId(),
      label: "",
      token: "",
      chatId: 0,
      color: "#0088cc",
    };
    setSettings("data", "telegramBots", (prev) => [...(prev || []), bot]);
  };

  const removeBot = (index: number) => {
    if (!settings.data) return;
    setSettings("data", "telegramBots", (prev) => (prev || []).filter((_, i) => i !== index));
  };

  const handleTestBot = async (bot: TelegramBotConfig, index: number) => {
    setTestingBot(bot.id);
    setTestResult(null);
    try {
      const chatId = await TelegramAPI.sendTest(bot.token);
      updateBot(index, "chatId", chatId);
      setTestResult({ id: bot.id, ok: true });
    } catch (e: any) {
      setTestResult({ id: bot.id, ok: false, msg: e?.toString() });
    }
    setTestingBot(null);
  };

  const hasAgentByCommand = (command: string): boolean => {
    if (!settings.data) return false;
    return settings.data.agents.some((a) => a.command.startsWith(command));
  };

  const executableBasename = (token: string): string => {
    const normalized = token.replace(/\\/g, "/");
    const leaf = normalized.split("/").pop() || normalized;
    return leaf.replace(/\.[^.]+$/, "").toLowerCase();
  };

  const tokenHasUnclosedQuote = (token: string, quote: string): boolean =>
    (token.split(quote).length - 1) % 2 === 1;

  const advancePastConfigValue = (tokens: string[], start: number): number => {
    if (start >= tokens.length) return start;
    let index = start;
    let inSingle = false;
    let inDouble = false;
    while (index < tokens.length) {
      const token = tokens[index];
      if (tokenHasUnclosedQuote(token, "'")) inSingle = !inSingle;
      if (tokenHasUnclosedQuote(token, '"')) inDouble = !inDouble;
      index += 1;
      if (!inSingle && !inDouble) break;
    }
    return index;
  };

  const codexHasManualResume = (tokens: string[], codexIndex: number): boolean => {
    let index = codexIndex + 1;
    while (index < tokens.length) {
      const token = tokens[index].toLowerCase();
      if (token === "-c" || token === "--config") {
        index = advancePastConfigValue(tokens, index + 1);
        continue;
      }
      if (token === "resume" || token === "--last") return true;
      index += 1;
    }
    return false;
  };

  // ── Validation ──
  const validateAgents = (): string | null => {
    if (!settings.data) return null;
    for (const agent of settings.data.agents) {
      const envError = validateEnvRows(agent.envs ?? []);
      if (envError) {
        return `Agent "${agent.label || "Unnamed"}": ${envError}`;
      }
      const tokens = agent.command.trim().split(/\s+/).filter(Boolean);
      const claudeIndex = tokens.findIndex((token) => executableBasename(token) === "claude");
      if (
        claudeIndex >= 0 &&
        tokens
          .slice(claudeIndex + 1)
          .some((token) => token === "--continue" || token === "-c")
      ) {
        return `Agent "${agent.label || "Unnamed"}": Claude commands must not include --continue or -c`;
      }

      const codexIndex = tokens.findIndex((token) => executableBasename(token) === "codex");
      if (codexIndex >= 0 && codexHasManualResume(tokens, codexIndex)) {
        return `Agent "${agent.label || "Unnamed"}": Codex commands must not include resume or --last; AgentsCommander injects codex resume --last automatically`;
      }
    }
    for (const [key, error] of Object.entries(profileCellErrors)) {
      if (error) return `Profile cell ${key}: ${error}`;
    }
    return null;
  };

  // ── Save ──
  const handleSave = async () => {
    if (!settings.data) return;
    const validationError = validateAgents();
    if (validationError) {
      setSaveError(validationError);
      return;
    }
    setSaveError("");
    setSaving(true);
    try {
      const nextSettings = mergeSettingsForSavePreservingProjects(
        settings.data,
        await SettingsAPI.get()
      );
      await SettingsAPI.update(nextSettings);
      await SettingsAPI.updateCodingAgentProfiles(nextSettings.codingAgentProfiles);
      for (const agent of nextSettings.agents) {
        await SettingsAPI.updateCodingAgentEnvSettings(
          agent.id,
          agent.envs ?? [],
          agent.isolateCodexHome ?? false
        );
      }
      setSettings("data", nextSettings);
      // #158 — push soundsEnabled into sound.ts synchronously so the gate
      // updates before the settingsStore.refresh() roundtrip below resolves.
      // Without this, a beep emitted between this point and the next load()
      // would see the stale gate value.
      setSoundsEnabled(nextSettings.soundsEnabled ?? true);
      if (isTauri) {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        await getCurrentWindow().setAlwaysOnTop(nextSettings.sidebarAlwaysOnTop);
      }
      // RTK sweep — only when the toggle value changed during this modal session.
      // Fired AFTER update_settings persists, so a sweep failure cannot leave
      // the persisted setting in disagreement with the on-disk replica state
      // worse than the pre-save baseline.
      const initial = initialInjectRtk();
      const next = nextSettings.injectRtkHook;
      if (initial !== null && initial !== next) {
        setRtkSweepInFlight(true);
        try {
          const result = await SettingsAPI.sweepRtkHook(next);
          if (result.errors.length > 0) {
            console.error(
              `[rtk] sweep partial failure: ${result.errors.length}/${result.total} dirs failed`,
              result.errors,
            );
          }
          setInitialInjectRtk(next);
        } catch (err) {
          console.error("[rtk] sweep failed:", err);
        } finally {
          setRtkSweepInFlight(false);
        }
      }
      // Refresh settings store so mic button visibility updates
      settingsStore.refresh();
      // Refresh repos (project_paths may have changed)
      try {
        const allRepos = await ReposAPI.search("");
        sessionsStore.setRepos(allRepos.filter((r) => r.agents.length > 0));
      } catch {}
      setSaving(false);
      props.onClose();
    } catch (err: unknown) {
      setSaveError(err instanceof Error ? err.message : String(err));
      setSaving(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
  };

  document.addEventListener("keydown", handleKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleKeyDown));

  // ── Tab renderers ──

  const renderGeneralTab = () => (
    <>
      <div class="settings-section">
        <div class="settings-section-title">Shell</div>
        <label class="settings-field">
          <span class="settings-label">Default Shell</span>
          <input
            class="settings-input"
            value={settings.data!.defaultShell}
            onInput={(e) => updateField("defaultShell", e.currentTarget.value)}
          />
        </label>
        <label class="settings-field">
          <span class="settings-label">Shell Arguments</span>
          <input
            class="settings-input"
            value={settings.data!.defaultShellArgs.join(" ")}
            onInput={(e) =>
              updateField(
                "defaultShellArgs",
                e.currentTarget.value.split(" ").filter(Boolean)
              )
            }
          />
        </label>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Window</div>
        <label class="settings-field">
          <span class="settings-label">App Theme</span>
          <select
            class="settings-input"
            value={settings.data!.sidebarStyle ?? "noir-minimal"}
            onChange={(e) => {
              updateField("sidebarStyle", e.currentTarget.value);
              document.documentElement.dataset.sidebarStyle = e.currentTarget.value;
            }}
          >
            <option value="noir-minimal">Noir Minimal</option>
            <option value="card-sections">Card Sections</option>
            <option value="command-center">Command Center</option>
            <option value="deep-space">Deep Space</option>
            <option value="arctic-ops">Arctic Ops</option>
            <option value="obsidian-mesh">Obsidian Mesh</option>
            <option value="neon-circuit">Neon Circuit</option>
          </select>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.restoreCoordinatorWakeState}
            onChange={(e) =>
              updateField("restoreCoordinatorWakeState", e.currentTarget.checked)
            }
          />
          <span>On start, wake coordinators that were awake when the app closed</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.sidebarAlwaysOnTop}
            onChange={(e) =>
              updateField("sidebarAlwaysOnTop", e.currentTarget.checked)
            }
          />
          <span>Sidebar always on top</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.raiseTerminalOnClick}
            onChange={(e) =>
              updateField("raiseTerminalOnClick", e.currentTarget.checked)
            }
          />
          <span>Raise terminal when clicking sidebar</span>
        </label>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">RTK Token Compression</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.injectRtkHook}
            disabled={saving() || rtkSweepInFlight()}
            onChange={(e) => updateField("injectRtkHook", e.currentTarget.checked)}
          />
          <span>Inject RTK hook into agent replicas</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.informWhenRtkInstalled}
            disabled={saving()}
            onChange={(e) =>
              updateField("informWhenRtkInstalled", e.currentTarget.checked)
            }
          />
          <span>Show the startup banner when RTK is installed but not enabled</span>
        </label>
        <div class="settings-hint">
          Off by default. When on, AC offers to enable RTK injection via a sidebar
          banner at startup. This banner setting is read once at launch, so changes
          to it take effect the next time AC starts.
        </div>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Web Remote Access</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.webServerEnabled}
            onChange={(e) =>
              updateField("webServerEnabled", e.currentTarget.checked)
            }
          />
          <span>Enable web server</span>
        </label>
        <Show when={settings.data!.webServerEnabled}>
          <div style="display: flex; gap: 6px; margin-top: 6px; align-items: center;">
            <button
              class="settings-add-btn"
              onClick={async () => {
                try {
                  const running = await SettingsAPI.getWebServerStatus();
                  if (running) {
                    await SettingsAPI.stopWebServer();
                    setWebServerRunning(false);
                  } else {
                    await SettingsAPI.startWebServer();
                    setWebServerRunning(true);
                  }
                } catch (err) {
                  console.error("Web server toggle failed:", err);
                }
              }}
            >
              {webServerRunning() ? "Stop Server" : "Start Server"}
            </button>
            <button
              class="settings-add-btn"
              disabled={!webServerRunning()}
              style={!webServerRunning() ? "opacity: 0.4; cursor: default;" : ""}
              onClick={() => {
                SettingsAPI.openWebRemote().catch((err) =>
                  console.error("Failed to open web remote:", err)
                );
              }}
            >
              Open in Browser
            </button>
            <span style={`font-size: 11px; opacity: 0.6;`}>
              {webServerRunning() ? "● Running" : "○ Stopped"}
            </span>
          </div>
        </Show>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Notifications</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.soundsEnabled}
            onChange={(e) =>
              updateField("soundsEnabled", e.currentTarget.checked)
            }
          />
          <span>Enable app sounds (master switch)</span>
        </label>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.teamIdleBeepEnabled}
            disabled={!settings.data!.soundsEnabled}
            onChange={(e) =>
              updateField("teamIdleBeepEnabled", e.currentTarget.checked)
            }
          />
          <span>Beep when a team finishes working (all agents idle)</span>
        </label>
      </div>

    </>
  );

  const renderAgentEnvEditor = (agent: AgentConfig, agentIndex: number) => {
    const codex = () => isCodexAgent(agent);
    const hasUserCodexHome = () => hasEnabledEnvKey(agent.envs ?? [], "CODEX_HOME");
    return (
      <div class="settings-env-editor">
        <div class="settings-subsection-title">Environment</div>
        <Show when={(agent.envs ?? []).length > 0} fallback={
          <div class="settings-empty-note">No environment rows configured.</div>
        }>
          <For each={agent.envs ?? []}>
            {(row, rowIndex) => {
              const readOnly = () => row.source === "agentsCommander";
              return (
                <div class="settings-env-row">
                  <input
                    class="settings-input settings-env-key"
                    value={row.key}
                    disabled={readOnly()}
                    onInput={(e) => updateAgentEnv(agentIndex, rowIndex(), "key", e.currentTarget.value)}
                    placeholder="KEY"
                  />
                  <input
                    class="settings-input settings-env-value"
                    type="password"
                    value={row.value}
                    disabled={readOnly()}
                    onInput={(e) => updateAgentEnv(agentIndex, rowIndex(), "value", e.currentTarget.value)}
                    placeholder="value"
                  />
                  <label class="settings-env-toggle" title="Enable row">
                    <input
                      type="checkbox"
                      class="settings-checkbox"
                      checked={row.enabled}
                      disabled={readOnly()}
                      onChange={(e) => updateAgentEnv(agentIndex, rowIndex(), "enabled", e.currentTarget.checked)}
                    />
                  </label>
                  <span class={`settings-env-source ${row.source === "agentsCommander" ? "generated" : ""}`}>
                    {row.source}
                  </span>
                  <button
                    class="settings-env-delete"
                    disabled={readOnly()}
                    onClick={() => removeAgentEnv(agentIndex, rowIndex())}
                    title={readOnly() ? "Managed by AgentsCommander" : "Delete environment row"}
                  >
                    &#x2715;
                  </button>
                </div>
              );
            }}
          </For>
        </Show>
        <button class="settings-add-btn settings-env-add" onClick={() => addAgentEnv(agentIndex)}>
          + Environment Row
        </button>
        <Show when={codex()}>
          <label class="settings-checkbox-field">
            <input
              type="checkbox"
              class="settings-checkbox"
              checked={agent.isolateCodexHome}
              onChange={(e) =>
                updateAgent(agentIndex, "isolateCodexHome", e.currentTarget.checked)
              }
            />
            <span>Isolate CODEX_HOME for this Codex agent</span>
          </label>
          <Show when={agent.isolateCodexHome}>
            <div class="settings-codex-home-preview warning">
              <span class="settings-env-source generated">agentsCommander</span>
              Generated CODEX_HOME will be used for launches from this agent.
              <Show when={hasUserCodexHome()}>
                <span> Saved CODEX_HOME rows are kept but overridden while isolation is on.</span>
              </Show>
            </div>
          </Show>
          <Show when={!agent.isolateCodexHome && hasUserCodexHome()}>
            <div class="settings-codex-home-preview">
              User CODEX_HOME is active for this Codex agent. Values remain masked in Settings.
            </div>
          </Show>
        </Show>
        <div class="settings-hint">
          Env values are stored as plaintext local settings and are masked here by default.
        </div>
      </div>
    );
  };

  const renderAgentsTab = () => (
    <div class="settings-section">
      <div class="settings-section-title">Coding Agents</div>

      <For each={settings.data!.agents}>
        {(agent, i) => (
          <div class="settings-button-card">
            <div class="settings-button-card-header">
              <div
                class="settings-color-dot"
                style={{ background: agent.color }}
              />
              <span>{agent.label || "New Agent"}</span>
              <button
                class="settings-agent-remove"
                onClick={() => removeAgent(i())}
                title="Remove agent"
              >
                &#x2715;
              </button>
            </div>
            <label class="settings-field">
              <span class="settings-label">Label</span>
              <input
                class="settings-input"
                value={agent.label}
                onInput={(e) =>
                  updateAgent(i(), "label", e.currentTarget.value)
                }
                placeholder="My Agent"
              />
            </label>
            <label class="settings-field">
              <span class="settings-label">Command</span>
              <input
                class="settings-input"
                value={agent.command}
                onInput={(e) =>
                  updateAgent(i(), "command", e.currentTarget.value)
                }
                placeholder="agent-cli"
              />
            </label>
            <label class="settings-field">
              <span class="settings-label">Color</span>
              <div class="settings-color-row">
                <input
                  type="color"
                  class="settings-color-picker"
                  value={agent.color}
                  onInput={(e) =>
                    updateAgent(i(), "color", e.currentTarget.value)
                  }
                />
                <input
                  class="settings-input settings-input-sm"
                  value={agent.color}
                  onInput={(e) =>
                    updateAgent(i(), "color", e.currentTarget.value)
                  }
                />
              </div>
            </label>
            <label class="settings-checkbox-field">
              <input
                type="checkbox"
                class="settings-checkbox"
                checked={agent.gitPullBefore}
                onChange={(e) =>
                  updateAgent(i(), "gitPullBefore", e.currentTarget.checked)
                }
              />
              <span>Run git pull before launch</span>
            </label>
            <label class="settings-checkbox-field">
              <input
                type="checkbox"
                class="settings-checkbox"
                checked={agent.excludeGlobalClaudeMd}
                onChange={(e) =>
                  updateAgent(i(), "excludeGlobalClaudeMd", e.currentTarget.checked)
                }
              />
              <span>Exclude global CLAUDE.md on agent creation</span>
            </label>
            {renderAgentEnvEditor(agent, i())}
          </div>
        )}
      </For>

      <div class="settings-agent-actions">
        <Show when={!hasAgentByCommand("claude")}>
          <button
            class="settings-preset-btn"
            onClick={() => addAgent(AGENT_PRESET_MAP.claude)}
          >
            <span
              class="settings-color-dot"
              style={{ background: AGENT_PRESET_MAP.claude.color }}
            />
            + Claude Code
          </button>
        </Show>
        <Show when={!hasAgentByCommand("codex")}>
          <button
            class="settings-preset-btn"
            onClick={() => addAgent(AGENT_PRESET_MAP.codex)}
          >
            <span
              class="settings-color-dot"
              style={{ background: AGENT_PRESET_MAP.codex.color }}
            />
            + Codex
          </button>
        </Show>
        <Show when={!hasAgentByCommand("gemini")}>
          <button
            class="settings-preset-btn"
            onClick={() => addAgent(AGENT_PRESET_MAP.gemini)}
          >
            <span
              class="settings-color-dot"
              style={{ background: AGENT_PRESET_MAP.gemini.color }}
            />
            + Gemini CLI
          </button>
        </Show>
        <button class="settings-add-btn" onClick={() => addAgent()}>
          + Custom Agent
        </button>
      </div>
    </div>
  );

  const renderProfilesTab = () => (
    <div class="settings-section settings-profiles-section">
      <div class="settings-section-title">Profiles</div>

      <div class="settings-profile-letter-list">
        <For each={profileLetters()}>
          {(letter) => (
            <div class="settings-profile-letter-row">
              <span class="settings-profile-letter-badge">
                {letter}
              </span>
              <input
                class="settings-input"
                value={settings.data!.codingAgentProfiles.letters[letter]?.name ?? ""}
                onInput={(e) => updateProfileName(letter, e.currentTarget.value)}
                placeholder={letter === "A" ? "Baseline" : "Profile name"}
              />
              <button
                class="settings-env-delete"
                disabled={letter === "A"}
                onClick={() => removeProfileLetter(letter)}
                title={letter === "A" ? "Profile A cannot be deleted" : "Delete profile"}
              >
                &#x2715;
              </button>
            </div>
          )}
        </For>
      </div>

      <div class="settings-profile-matrix-wrap">
        <div
          class="settings-profile-matrix"
          style={{
            "grid-template-columns": `minmax(96px, 128px) repeat(${Math.max(settings.data!.agents.length, 1)}, minmax(180px, 1fr))`,
          }}
        >
          <div class="settings-profile-cell settings-profile-head">Profile</div>
          <For each={settings.data!.agents}>
            {(agent) => (
              <div
                class="settings-profile-cell settings-profile-head"
                style={{ "--agent-color": agent.color }}
              >
                <span class="settings-color-dot" style={{ background: agent.color }} />
                <span>{agent.label || agent.id}</span>
              </div>
            )}
          </For>

          <For each={profileLetters()}>
            {(letter) => (
              <>
                <div class="settings-profile-cell settings-profile-row-head">
                  {profileDisplayLabel(settings.data!.codingAgentProfiles, letter)}
                </div>
                <For each={settings.data!.agents}>
                  {(agent) => {
                    const rawCell = () => profileCell(agent.id, letter);
                    const editable = () => letter === "A" || !!rawCell()?.enabled;
                    const preview = () =>
                      resolveProfilePreview(
                        settings.data!.codingAgentProfiles,
                        agent.id,
                        letter
                      );
                    return (
                      <div class="settings-profile-cell settings-profile-edit-cell">
                        <Show
                          when={editable()}
                          fallback={
                            <div class="settings-profile-missing">
                              <span>{letter} -&gt; {preview().effectiveProfile}</span>
                              <button
                                class="settings-profile-cell-btn"
                                onClick={() => addProfileCell(agent.id, letter)}
                              >
                                Add
                              </button>
                            </div>
                          }
                        >
                          <input
                            class="settings-input settings-profile-argv"
                            value={displayedProfileCellText(agent.id, letter)}
                            onInput={(e) =>
                              updateProfileCellText(agent.id, letter, e.currentTarget.value)
                            }
                            placeholder="argv"
                          />
                          <div class="settings-profile-cell-actions">
                            <Show when={profileCellErrors[profileCellKey(agent.id, letter)]}>
                              <span class="settings-profile-cell-error">
                                {profileCellErrors[profileCellKey(agent.id, letter)]}
                              </span>
                            </Show>
                            <Show when={preview().fallbackApplied}>
                              <span class="settings-profile-fallback">
                                {preview().requestedProfile} -&gt; {preview().effectiveProfile}
                              </span>
                            </Show>
                            <button
                              class="settings-profile-cell-btn"
                              disabled={letter === "A"}
                              onClick={() => removeProfileCell(agent.id, letter)}
                              title={letter === "A" ? "Profile A cell cannot be deleted" : "Delete cell"}
                            >
                              Delete
                            </button>
                          </div>
                        </Show>
                      </div>
                    );
                  }}
                </For>
              </>
            )}
          </For>
        </div>
      </div>

      <button
        class="settings-add-btn settings-profile-add"
        onClick={addProfileLetter}
        disabled={!nextAvailableProfileLetter(settings.data!.codingAgentProfiles)}
      >
        + Profile
      </button>
    </div>
  );

  const renderIntegrationsTab = () => (
    <>
      {/* Voice to Text */}
      <div class="settings-section">
        <div class="settings-section-title">Voice to Text</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings.data!.voiceToTextEnabled}
            onChange={(e) =>
              updateField("voiceToTextEnabled", e.currentTarget.checked)
            }
          />
          <span>Enable microphone button on sessions</span>
        </label>
        <Show when={settings.data!.voiceToTextEnabled}>
          <label class="settings-field">
            <span class="settings-label">Gemini API Key</span>
            <input
              class="settings-input"
              type="password"
              value={settings.data!.geminiApiKey}
              onInput={(e) =>
                updateField("geminiApiKey", e.currentTarget.value)
              }
              placeholder="AIza..."
            />
          </label>
          <label class="settings-field">
            <span class="settings-label">Gemini Model</span>
            <select
              class="settings-input"
              value={settings.data!.geminiModel}
              onChange={(e) =>
                updateField("geminiModel", e.currentTarget.value)
              }
            >
              <For each={GEMINI_MODELS}>
                {(m) => (
                  <option value={m.id}>{m.label}</option>
                )}
              </For>
            </select>
          </label>
          <label class="settings-checkbox-field">
            <input
              type="checkbox"
              class="settings-checkbox"
              checked={settings.data!.voiceAutoExecute}
              onChange={(e) =>
                updateField("voiceAutoExecute", e.currentTarget.checked)
              }
            />
            <span>Auto-execute after transcription</span>
          </label>
          <Show when={settings.data!.voiceAutoExecute}>
            <label class="settings-field">
              <span class="settings-label">Auto-execute delay (seconds)</span>
              <input
                class="settings-input settings-input-sm"
                type="number"
                min="1"
                max="120"
                value={settings.data!.voiceAutoExecuteDelay}
                onInput={(e) => {
                  const v = parseInt(e.currentTarget.value, 10);
                  if (!isNaN(v)) updateField("voiceAutoExecuteDelay", Math.max(1, Math.min(120, v)));
                }}
              />
            </label>
          </Show>
        </Show>
      </div>

      {/* Telegram Bots */}
      <div class="settings-section">
        <div class="settings-section-title">Telegram Bots</div>

      <For each={settings.data!.telegramBots || []}>
        {(bot, i) => (
          <div class="settings-button-card">
            <div class="settings-button-card-header">
              <div
                class="settings-color-dot"
                style={{ background: bot.color }}
              />
              <span>{bot.label || "New Bot"}</span>
              <button
                class="settings-agent-remove"
                onClick={() => removeBot(i())}
                title="Remove bot"
              >
                &#x2715;
              </button>
            </div>
            <label class="settings-field">
              <span class="settings-label">Label</span>
              <input
                class="settings-input"
                value={bot.label}
                onInput={(e) =>
                  updateBot(i(), "label", e.currentTarget.value)
                }
                placeholder="My Bot"
              />
            </label>
            <label class="settings-field">
              <span class="settings-label">Bot Token</span>
              <input
                class="settings-input"
                type="password"
                value={bot.token}
                onInput={(e) =>
                  updateBot(i(), "token", e.currentTarget.value)
                }
                placeholder="123456:ABC-DEF..."
              />
            </label>
            <Show when={bot.chatId}>
              <div class="settings-field">
                <span class="settings-label">Chat ID</span>
                <span class="settings-chat-id">{bot.chatId}</span>
              </div>
            </Show>
            <label class="settings-field">
              <span class="settings-label">Color</span>
              <div class="settings-color-row">
                <input
                  type="color"
                  class="settings-color-picker"
                  value={bot.color}
                  onInput={(e) =>
                    updateBot(i(), "color", e.currentTarget.value)
                  }
                />
                <input
                  class="settings-input settings-input-sm"
                  value={bot.color}
                  onInput={(e) =>
                    updateBot(i(), "color", e.currentTarget.value)
                  }
                />
              </div>
            </label>
            <div class="settings-bot-actions">
              <button
                class="settings-test-btn"
                onClick={() => handleTestBot(bot, i())}
                disabled={testingBot() === bot.id || !bot.token}
              >
                {testingBot() === bot.id ? "Testing..." : "Test"}
              </button>
              <Show when={testResult()?.id === bot.id}>
                <span
                  class={`settings-test-result ${testResult()!.ok ? "ok" : "fail"}`}
                >
                  {testResult()!.ok
                    ? "Connected"
                    : testResult()!.msg || "Failed"}
                </span>
              </Show>
            </div>
          </div>
        )}
      </For>

      <button class="settings-add-btn" onClick={addBot}>
        + Add Telegram Bot
      </button>
    </div>
    </>
  );

  return (
    <div class="modal-overlay">
      <div class="modal-container modal-container-lg">
        <div class="modal-header">
          <span class="modal-title">Settings</span>
        </div>

        {/* Tab bar */}
        <div class="settings-tabs">
          <For each={TABS}>
            {(tab) => (
              <button
                class={`settings-tab ${activeTab() === tab.key ? "active" : ""}`}
                onClick={() => setActiveTab(tab.key)}
              >
                {tab.label}
              </button>
            )}
          </For>
        </div>

        <Show
          when={settings.data}
          fallback={
            <div class="modal-body" style="display:flex;align-items:center;justify-content:center;min-height:200px;color:#555;font-size:13px">
              Loading...
            </div>
          }
        >
          <div class="modal-body">
            <Show when={activeTab() === "general"}>{renderGeneralTab()}</Show>
            <Show when={activeTab() === "agents"}>{renderAgentsTab()}</Show>
            <Show when={activeTab() === "profiles"}>{renderProfilesTab()}</Show>
            <Show when={activeTab() === "integrations"}>
              {renderIntegrationsTab()}
            </Show>
          </div>
        </Show>

        <div class="modal-footer">
          <Show when={saveError()}>
            <span class="modal-save-error">{saveError()}</span>
          </Show>
          <button class="modal-btn modal-btn-cancel" onClick={props.onClose}>
            Cancel
          </button>
          <button
            class="modal-btn modal-btn-save"
            onClick={handleSave}
            disabled={saving() || rtkSweepInFlight()}
          >
            {saving() ? "Saving..." : rtkSweepInFlight() ? "Sweeping..." : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default SettingsModal;
