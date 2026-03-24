import { Component, createSignal, For, Show, onMount } from "solid-js";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  AppSettings,
  AgentConfig,
  TelegramBotConfig,
  DarkFactoryConfig,
  Team,
  TeamMember,
  RepoMatch,
} from "../../shared/types";
import { SettingsAPI, TelegramAPI, DarkFactoryAPI, ReposAPI } from "../../shared/ipc";

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

type SettingsTab = "general" | "agents" | "integrations" | "darkfactory";

const TABS: { key: SettingsTab; label: string }[] = [
  { key: "general", label: "General" },
  { key: "agents", label: "Coding Agents" },
  { key: "integrations", label: "Integrations" },
  { key: "darkfactory", label: "Dark Factory" },
];

const SettingsModal: Component<{ onClose: () => void }> = (props) => {
  const [settings, setSettings] = createSignal<AppSettings | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [testingBot, setTestingBot] = createSignal<string | null>(null);
  const [testResult, setTestResult] = createSignal<{
    id: string;
    ok: boolean;
    msg?: string;
  } | null>(null);
  const [activeTab, setActiveTab] = createSignal<SettingsTab>("general");

  // Dark Factory state (separate from AppSettings)
  const [dfConfig, setDfConfig] = createSignal<DarkFactoryConfig>({ teams: [] });
  const [repoResults, setRepoResults] = createSignal<RepoMatch[]>([]);
  const [repoQuery, setRepoQuery] = createSignal("");
  const [searchingRepos, setSearchingRepos] = createSignal(false);
  const [addingMemberToTeam, setAddingMemberToTeam] = createSignal<string | null>(null);
  const [newTeamName, setNewTeamName] = createSignal("");
  const [teamNameError, setTeamNameError] = createSignal("");

  onMount(async () => {
    const [s, df] = await Promise.all([SettingsAPI.get(), DarkFactoryAPI.get()]);
    setSettings(s);
    setDfConfig(df);
  });

  // ── Generic field updater ──
  const updateField = <K extends keyof AppSettings>(
    key: K,
    value: AppSettings[K]
  ) => {
    const s = settings();
    if (s) setSettings({ ...s, [key]: value });
  };

  // ── Repo paths ──
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

  // ── Agents ──
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

  // ── Telegram Bots ──
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
    const s = settings();
    if (!s) return false;
    return s.agents.some((a) => a.command === command);
  };

  // ── Dark Factory: Teams ──
  const addTeam = () => {
    const name = newTeamName().trim();
    if (!name) {
      setTeamNameError("Name cannot be empty");
      return;
    }
    const df = dfConfig();
    if (df.teams.some((t) => t.name.toLowerCase() === name.toLowerCase())) {
      setTeamNameError("Team name already exists");
      return;
    }
    setTeamNameError("");
    const team: Team = {
      id: newId(),
      name,
      members: [],
    };
    setDfConfig({ teams: [...df.teams, team] });
    setNewTeamName("");
  };

  const removeTeam = (teamId: string) => {
    const df = dfConfig();
    setDfConfig({ teams: df.teams.filter((t) => t.id !== teamId) });
  };

  const updateTeamName = (teamId: string, name: string) => {
    const df = dfConfig();
    setDfConfig({
      teams: df.teams.map((t) => (t.id === teamId ? { ...t, name } : t)),
    });
  };

  const addMemberToTeam = (teamId: string, repo: RepoMatch) => {
    const df = dfConfig();
    setDfConfig({
      teams: df.teams.map((t) => {
        if (t.id !== teamId) return t;
        // Prevent duplicates
        if (t.members.some((m) => m.path === repo.path)) return t;
        const member: TeamMember = { name: repo.name, path: repo.path };
        return { ...t, members: [...t.members, member] };
      }),
    });
    setAddingMemberToTeam(null);
    setRepoQuery("");
    setRepoResults([]);
  };

  const removeMember = (teamId: string, memberPath: string) => {
    const df = dfConfig();
    setDfConfig({
      teams: df.teams.map((t) => {
        if (t.id !== teamId) return t;
        const members = t.members.filter((m) => m.path !== memberPath);
        // Clear coordinator if removed
        const coordCleared =
          t.coordinatorName &&
          !members.some((m) => m.name === t.coordinatorName)
            ? undefined
            : t.coordinatorName;
        return { ...t, members, coordinatorName: coordCleared };
      }),
    });
  };

  const setCoordinator = (teamId: string, memberName: string | undefined) => {
    const df = dfConfig();
    setDfConfig({
      teams: df.teams.map((t) =>
        t.id === teamId ? { ...t, coordinatorName: memberName } : t
      ),
    });
  };

  const handleRepoSearch = async (query: string) => {
    setRepoQuery(query);
    if (query.length < 1) {
      setRepoResults([]);
      return;
    }
    setSearchingRepos(true);
    try {
      const results = await ReposAPI.search(query);
      setRepoResults(results);
    } catch {
      setRepoResults([]);
    }
    setSearchingRepos(false);
  };

  // ── Save ──
  const handleSave = async () => {
    const s = settings();
    if (!s) return;
    setSaving(true);
    await Promise.all([
      SettingsAPI.update(s),
      DarkFactoryAPI.save(dfConfig()),
    ]);
    await getCurrentWindow().setAlwaysOnTop(s.sidebarAlwaysOnTop);
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
            value={settings()!.defaultShell}
            onInput={(e) => updateField("defaultShell", e.currentTarget.value)}
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

      <div class="settings-section">
        <div class="settings-section-title">Window</div>
        <label class="settings-checkbox-field">
          <input
            type="checkbox"
            class="settings-checkbox"
            checked={settings()!.sidebarAlwaysOnTop}
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
            checked={settings()!.raiseTerminalOnClick}
            onChange={(e) =>
              updateField("raiseTerminalOnClick", e.currentTarget.checked)
            }
          />
          <span>Raise terminal when clicking sidebar</span>
        </label>
      </div>

      <div class="settings-section">
        <div class="settings-section-title">Repo Scan Paths</div>
        <For each={settings()!.repoPaths}>
          {(path, i) => (
            <div class="settings-path-row">
              <input
                class="settings-input settings-path-input"
                value={path}
                onInput={(e) => updateRepoPath(i(), e.currentTarget.value)}
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
    </>
  );

  const renderAgentsTab = () => (
    <div class="settings-section">
      <div class="settings-section-title">Coding Agents</div>

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
                  updateAgent(i(), "gitPullBefore", e.currentTarget.checked)
                }
              />
              <span>Run git pull before launch</span>
            </label>
          </div>
        )}
      </For>

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
        <button class="settings-add-btn" onClick={() => addAgent()}>
          + Custom Agent
        </button>
      </div>
    </div>
  );

  const renderIntegrationsTab = () => (
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
  );

  const renderDarkFactoryTab = () => (
    <>
      <div class="settings-section">
        <div class="settings-section-title">Teams</div>
        <p class="settings-hint">
          Teams group agents (repos) together. Members of a team can communicate
          with each other. When a coordinator is set, all messages route through
          them.
        </p>

        <For each={dfConfig().teams}>
          {(team) => (
            <div class="settings-button-card df-team-card">
              <div class="settings-button-card-header">
                <span class="df-team-name">{team.name}</span>
                <span class="df-team-count">
                  {team.members.length} member{team.members.length !== 1 ? "s" : ""}
                </span>
                <button
                  class="settings-agent-remove"
                  onClick={() => removeTeam(team.id)}
                  title="Remove team"
                >
                  &#x2715;
                </button>
              </div>

              {/* Team name edit */}
              <label class="settings-field">
                <span class="settings-label">Team Name</span>
                <input
                  class="settings-input"
                  value={team.name}
                  onInput={(e) => updateTeamName(team.id, e.currentTarget.value)}
                />
              </label>

              {/* Coordinator */}
              <label class="settings-field">
                <span class="settings-label">Coordinator</span>
                <select
                  class="settings-input settings-select"
                  value={team.coordinatorName || ""}
                  onChange={(e) =>
                    setCoordinator(
                      team.id,
                      e.currentTarget.value || undefined
                    )
                  }
                >
                  <option value="">None</option>
                  <For each={team.members}>
                    {(member) => (
                      <option value={member.name}>{member.name}</option>
                    )}
                  </For>
                </select>
              </label>

              {/* Members */}
              <div class="df-members">
                <span class="settings-label">Members</span>
                <For each={team.members}>
                  {(member) => (
                    <div class="df-member-row">
                      <Show when={team.coordinatorName === member.name}>
                        <span class="df-coord-badge">COORD</span>
                      </Show>
                      <span class="df-member-name">{member.name}</span>
                      <span class="df-member-path" title={member.path}>
                        {member.path}
                      </span>
                      <button
                        class="settings-path-remove"
                        onClick={() => removeMember(team.id, member.path)}
                        title="Remove member"
                      >
                        &#x2715;
                      </button>
                    </div>
                  )}
                </For>

                {/* Add member */}
                <Show when={addingMemberToTeam() === team.id}>
                  <div class="df-add-member-search">
                    <input
                      class="settings-input"
                      placeholder="Search repos..."
                      value={repoQuery()}
                      onInput={(e) => handleRepoSearch(e.currentTarget.value)}
                      autofocus
                    />
                    <Show when={repoResults().length > 0}>
                      <div class="df-repo-results">
                        <For each={repoResults()}>
                          {(repo) => (
                            <button
                              class="df-repo-result-item"
                              onClick={() => addMemberToTeam(team.id, repo)}
                              disabled={team.members.some(
                                (m) => m.path === repo.path
                              )}
                            >
                              <span class="df-repo-name">{repo.name}</span>
                              <span class="df-repo-agents">
                                {repo.agents.join(", ")}
                              </span>
                            </button>
                          )}
                        </For>
                      </div>
                    </Show>
                    <Show when={searchingRepos()}>
                      <span class="df-searching">Searching...</span>
                    </Show>
                  </div>
                </Show>

                <button
                  class="settings-add-btn"
                  onClick={() =>
                    setAddingMemberToTeam(
                      addingMemberToTeam() === team.id ? null : team.id
                    )
                  }
                >
                  {addingMemberToTeam() === team.id
                    ? "Cancel"
                    : "+ Add Member"}
                </button>
              </div>
            </div>
          )}
        </For>

        {/* New team */}
        <div class="df-new-team-row">
          <input
            class="settings-input df-new-team-input"
            placeholder="New team name..."
            value={newTeamName()}
            onInput={(e) => {
              setNewTeamName(e.currentTarget.value);
              setTeamNameError("");
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") addTeam();
            }}
          />
          <button class="settings-add-btn" onClick={addTeam}>
            + Add Team
          </button>
        </div>
        <Show when={teamNameError()}>
          <span class="df-team-error">{teamNameError()}</span>
        </Show>
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

        {settings() && (
          <div class="modal-body">
            <Show when={activeTab() === "general"}>{renderGeneralTab()}</Show>
            <Show when={activeTab() === "agents"}>{renderAgentsTab()}</Show>
            <Show when={activeTab() === "integrations"}>
              {renderIntegrationsTab()}
            </Show>
            <Show when={activeTab() === "darkfactory"}>
              {renderDarkFactoryTab()}
            </Show>
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
