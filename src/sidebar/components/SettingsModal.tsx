import { Component, createSignal, For, Show, onMount } from "solid-js";
import type { AppSettings, AgentConfig, TelegramBotConfig } from "../../shared/types";
import { SettingsAPI, TelegramAPI } from "../../shared/ipc";

const AGENT_PRESETS: Record<string, Omit<AgentConfig, "id">> = {
  claude: {
    label: "Claude Code",
    command: "claude",
    args: ["--dangerously-skip-permissions"],
    color: "#d97706",
    gitPullBefore: true,
  },
  codex: {
    label: "Codex",
    command: "codex",
    args: [],
    color: "#10b981",
    gitPullBefore: true,
  },
};

let idCounter = 0;
function newId(): string {
  return `agent_${Date.now()}_${idCounter++}`;
}

const SettingsModal: Component<{ onClose: () => void }> = (props) => {
  const [settings, setSettings] = createSignal<AppSettings | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [testingBot, setTestingBot] = createSignal<string | null>(null);
  const [testResult, setTestResult] = createSignal<{ id: string; ok: boolean; msg?: string } | null>(null);

  onMount(async () => {
    const s = await SettingsAPI.get();
    setSettings(s);
  });

  const updateField = <K extends keyof AppSettings>(
    key: K,
    value: AppSettings[K]
  ) => {
    const s = settings();
    if (s) setSettings({ ...s, [key]: value });
  };

  // Repo paths
  const updateRepoPath = (index: number, value: string) => {
    const s = settings();
    if (!s) return;
    const paths = [...s.repoPaths];
    paths[index] = value;
    updateField("repoPaths", paths);
  };

  const addRepoPath = () => {
    const s = settings();
    if (!s) return;
    updateField("repoPaths", [...s.repoPaths, ""]);
  };

  const removeRepoPath = (index: number) => {
    const s = settings();
    if (!s) return;
    const paths = s.repoPaths.filter((_, i) => i !== index);
    updateField("repoPaths", paths);
  };

  // Agents
  const updateAgent = (
    index: number,
    field: keyof AgentConfig,
    value: string | boolean | string[]
  ) => {
    const s = settings();
    if (!s) return;
    const agents = [...s.agents];
    agents[index] = { ...agents[index], [field]: value };
    updateField("agents", agents);
  };

  const addAgent = (preset?: Omit<AgentConfig, "id">) => {
    const s = settings();
    if (!s) return;
    const agent: AgentConfig = preset
      ? { id: newId(), ...preset }
      : {
          id: newId(),
          label: "",
          command: "",
          args: [],
          color: "#6366f1",
          gitPullBefore: false,
        };
    updateField("agents", [...s.agents, agent]);
  };

  const removeAgent = (index: number) => {
    const s = settings();
    if (!s) return;
    updateField(
      "agents",
      s.agents.filter((_, i) => i !== index)
    );
  };

  // Telegram Bots
  const updateBot = (
    index: number,
    field: keyof TelegramBotConfig,
    value: string | number
  ) => {
    const s = settings();
    if (!s) return;
    const bots = [...(s.telegramBots || [])];
    bots[index] = { ...bots[index], [field]: value };
    updateField("telegramBots", bots);
  };

  const addBot = () => {
    const s = settings();
    if (!s) return;
    const bot: TelegramBotConfig = {
      id: newId(),
      label: "",
      token: "",
      chatId: 0,
      color: "#0088cc",
    };
    updateField("telegramBots", [...(s.telegramBots || []), bot]);
  };

  const removeBot = (index: number) => {
    const s = settings();
    if (!s) return;
    updateField(
      "telegramBots",
      (s.telegramBots || []).filter((_, i) => i !== index)
    );
  };

  const handleTestBot = async (bot: TelegramBotConfig) => {
    setTestingBot(bot.id);
    setTestResult(null);
    try {
      await TelegramAPI.sendTest(bot.token, bot.chatId);
      setTestResult({ id: bot.id, ok: true });
    } catch (e: any) {
      setTestResult({ id: bot.id, ok: false, msg: e?.toString() });
    }
    setTestingBot(null);
  };

  const hasAgentByCommand = (command: string): boolean => {
    const s = settings();
    if (!s) return false;
    return s.agents.some((a) => a.command === command);
  };

  const handleSave = async () => {
    const s = settings();
    if (!s) return;
    setSaving(true);
    await SettingsAPI.update(s);
    setSaving(false);
    props.onClose();
  };

  const handleOverlayClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("modal-overlay")) {
      props.onClose();
    }
  };

  return (
    <div class="modal-overlay" onClick={handleOverlayClick}>
      <div class="modal-container">
        <div class="modal-header">
          <span class="modal-title">Settings</span>
          <button class="modal-close" onClick={props.onClose}>
            &#x2715;
          </button>
        </div>

        {settings() && (
          <div class="modal-body">
            {/* General */}
            <div class="settings-section">
              <div class="settings-section-title">General</div>
              <label class="settings-field">
                <span class="settings-label">Default Shell</span>
                <input
                  class="settings-input"
                  value={settings()!.defaultShell}
                  onInput={(e) =>
                    updateField("defaultShell", e.currentTarget.value)
                  }
                />
              </label>
              <label class="settings-field">
                <span class="settings-label">Shell Arguments</span>
                <input
                  class="settings-input"
                  value={settings()!.defaultShellArgs.join(" ")}
                  onInput={(e) =>
                    updateField(
                      "defaultShellArgs",
                      e.currentTarget.value.split(" ").filter(Boolean)
                    )
                  }
                />
              </label>
            </div>

            {/* Repo Paths */}
            <div class="settings-section">
              <div class="settings-section-title">Repo Scan Paths</div>
              <For each={settings()!.repoPaths}>
                {(path, i) => (
                  <div class="settings-path-row">
                    <input
                      class="settings-input settings-path-input"
                      value={path}
                      onInput={(e) =>
                        updateRepoPath(i(), e.currentTarget.value)
                      }
                      placeholder="C:\path\to\repos"
                    />
                    <button
                      class="settings-path-remove"
                      onClick={() => removeRepoPath(i())}
                      title="Remove"
                    >
                      &#x2715;
                    </button>
                  </div>
                )}
              </For>
              <button class="settings-add-btn" onClick={addRepoPath}>
                + Add Path
              </button>
            </div>

            {/* Agents */}
            <div class="settings-section">
              <div class="settings-section-title">Agents</div>

              <For each={settings()!.agents}>
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
                      <span class="settings-label">Arguments</span>
                      <input
                        class="settings-input"
                        value={agent.args.join(" ")}
                        onInput={(e) =>
                          updateAgent(
                            i(),
                            "args",
                            e.currentTarget.value.split(" ").filter(Boolean)
                          )
                        }
                        placeholder="--flag value"
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
                          updateAgent(
                            i(),
                            "gitPullBefore",
                            e.currentTarget.checked
                          )
                        }
                      />
                      <span>Run git pull before launch</span>
                    </label>
                  </div>
                )}
              </For>

              {/* Add agent actions */}
              <div class="settings-agent-actions">
                <Show when={!hasAgentByCommand("claude")}>
                  <button
                    class="settings-preset-btn"
                    onClick={() => addAgent(AGENT_PRESETS.claude)}
                  >
                    <span
                      class="settings-color-dot"
                      style={{ background: AGENT_PRESETS.claude.color }}
                    />
                    + Claude Code
                  </button>
                </Show>
                <Show when={!hasAgentByCommand("codex")}>
                  <button
                    class="settings-preset-btn"
                    onClick={() => addAgent(AGENT_PRESETS.codex)}
                  >
                    <span
                      class="settings-color-dot"
                      style={{ background: AGENT_PRESETS.codex.color }}
                    />
                    + Codex
                  </button>
                </Show>
                <button
                  class="settings-add-btn"
                  onClick={() => addAgent()}
                >
                  + Custom Agent
                </button>
              </div>
            </div>

            {/* Telegram Bots */}
            <div class="settings-section">
              <div class="settings-section-title">Telegram Bots</div>

              <For each={settings()!.telegramBots || []}>
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
                    <label class="settings-field">
                      <span class="settings-label">Chat ID</span>
                      <input
                        class="settings-input"
                        type="number"
                        value={bot.chatId}
                        onInput={(e) =>
                          updateBot(
                            i(),
                            "chatId",
                            parseInt(e.currentTarget.value) || 0
                          )
                        }
                        placeholder="123456789"
                      />
                    </label>
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
                        onClick={() => handleTestBot(bot)}
                        disabled={testingBot() === bot.id || !bot.token || !bot.chatId}
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
          </div>
        )}

        <div class="modal-footer">
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
