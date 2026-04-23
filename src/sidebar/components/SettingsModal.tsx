import { Component, createSignal, createEffect, For, Show, onMount } from "solid-js";
import { createStore } from "solid-js/store";
import { isTauri } from "../../shared/platform";
import type {
  AppSettings,
  AgentConfig,
  TelegramBotConfig,
} from "../../shared/types";
import { SettingsAPI, TelegramAPI, ReposAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { sessionsStore } from "../stores/sessions";
import { AGENT_PRESET_MAP, newAgentId } from "../../shared/agent-presets";

const GEMINI_MODELS = [
  { id: "gemini-2.5-flash", label: "Gemini 2.5 Flash (recommended)" },
  { id: "gemini-2.5-pro", label: "Gemini 2.5 Pro" },
  { id: "gemini-2.0-flash", label: "Gemini 2.0 Flash" },
  { id: "gemini-1.5-flash", label: "Gemini 1.5 Flash" },
  { id: "gemini-1.5-pro", label: "Gemini 1.5 Pro" },
];


type SettingsTab = "general" | "agents" | "integrations";

const TABS: { key: SettingsTab; label: string }[] = [
  { key: "general", label: "General" },
  { key: "agents", label: "Coding Agents" },
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

  const s = () => settings.data;

  onMount(async () => {
    const [loaded, wsRunning] = await Promise.all([
      SettingsAPI.get(),
      SettingsAPI.getWebServerStatus().catch(() => false),
    ]);
    setSettings("data", loaded);
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
    value: string | boolean | string[]
  ) => {
    if (!settings.data) return;
    setSettings("data", "agents", index, field as any, value as any);
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
    await SettingsAPI.update(settings.data);
    if (isTauri) {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      await getCurrentWindow().setAlwaysOnTop(settings.data.sidebarAlwaysOnTop);
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
  };

  const handleOverlayClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("modal-overlay")) {
      props.onClose();
    }
  };

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
            checked={settings.data!.startOnlyCoordinators}
            onChange={(e) =>
              updateField("startOnlyCoordinators", e.currentTarget.checked)
            }
          />
          <span>On start only start Coordinators</span>
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

    </>
  );

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
    <div class="modal-overlay" onClick={handleOverlayClick}>
      <div class="modal-container modal-container-lg">
        <div class="modal-header">
          <span class="modal-title">Settings</span>
          <button class="modal-close" onClick={props.onClose}>
            &#x2715;
          </button>
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
            disabled={saving()}
          >
            {saving() ? "Saving..." : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
};

export default SettingsModal;
