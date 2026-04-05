import { Component, createSignal, createMemo, For, Show, onMount } from "solid-js";
import { EntityAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";
import type { AcTeam, TeamWizardAgentEntry, TeamWizardRepoEntry, TeamWizardStep } from "../../shared/types";

const EditTeamModal: Component<{
  projectPath: string;
  team: AcTeam;
  onClose: () => void;
}> = (props) => {
  const [step, setStep] = createSignal<TeamWizardStep>(1);
  const [allAgents, setAllAgents] = createSignal<TeamWizardAgentEntry[]>([]);
  const [selectedAgents, setSelectedAgents] = createSignal<Set<string>>(new Set());
  const [coordinator, setCoordinator] = createSignal<string>("");
  const [repos, setRepos] = createSignal<TeamWizardRepoEntry[]>([]);
  const [repoInput, setRepoInput] = createSignal("");
  const [error, setError] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [loading, setLoading] = createSignal(true);

  const [agentFilter, setAgentFilter] = createSignal("");

  const currentProjectName = createMemo(() => {
    const p = props.projectPath.replace(/[\\/]+$/, "");
    const idx = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
    return idx >= 0 ? p.slice(idx + 1) : p;
  });

  const agentsByProject = createMemo(() => {
    const filter = agentFilter().toLowerCase();
    const filtered = filter
      ? allAgents().filter((a) => a.name.toLowerCase().includes(filter))
      : allAgents();

    const map = new Map<string, TeamWizardAgentEntry[]>();
    for (const a of filtered) {
      const list = map.get(a.projectName) ?? [];
      list.push(a);
      map.set(a.projectName, list);
    }

    const cur = currentProjectName();
    const entries = Array.from(map.entries());
    entries.sort((a, b) => {
      if (a[0] === cur) return -1;
      if (b[0] === cur) return 1;
      return a[0].localeCompare(b[0]);
    });
    return new Map(entries);
  });

  const selectedAgentList = createMemo(() =>
    allAgents().filter((a) => selectedAgents().has(a.path))
  );

  const canNext2 = createMemo(() =>
    selectedAgents().size > 0 && coordinator() !== ""
  );

  onMount(async () => {
    try {
      // Load all agents and current team config in parallel
      const paths = projectStore.projects.map((p) => p.path);
      const [agentList, teamConfig] = await Promise.all([
        EntityAPI.listAllAgents(paths),
        EntityAPI.getTeamConfig(props.projectPath, props.team.name),
      ]);

      const entries: TeamWizardAgentEntry[] = agentList.map((a) => ({
        name: a.name,
        path: a.path,
        projectName: a.projectName,
      }));
      setAllAgents(entries);

      // Pre-select agents by matching raw config paths to listAllAgents paths.
      // Normalize paths for case/separator differences on Windows.
      const norm = (p: string) => p.replace(/\\/g, "/").toLowerCase();
      const configAgentNorm = new Set(teamConfig.agents.map(norm));
      const matched = new Set<string>();
      for (const entry of entries) {
        if (configAgentNorm.has(norm(entry.path))) {
          matched.add(entry.path);
        }
      }
      setSelectedAgents(matched);

      // Pre-select coordinator
      if (teamConfig.coordinator) {
        const coordNorm = norm(teamConfig.coordinator);
        const coordEntry = entries.find((e) => norm(e.path) === coordNorm);
        if (coordEntry) {
          setCoordinator(coordEntry.path);
        }
      }

      // Pre-populate repos
      if (teamConfig.repos && teamConfig.repos.length > 0) {
        setRepos(
          teamConfig.repos.map((r) => ({
            url: r.url,
            agents: new Set(r.agents),
          }))
        );
      }
    } catch (e) {
      console.error("Failed to load team config:", e);
      setError("Failed to load team configuration");
    } finally {
      setLoading(false);
    }
  });

  const toggleAgent = (path: string) => {
    setSelectedAgents((prev) => {
      const next = new Set(prev);
      if (next.has(path)) {
        next.delete(path);
        if (coordinator() === path) setCoordinator("");
      } else {
        next.add(path);
      }
      return next;
    });
  };

  const addRepo = () => {
    const url = repoInput().trim();
    if (!url) return;
    if (repos().some((r) => r.url === url)) {
      setError("Repo already added");
      return;
    }
    setRepos((prev) => [...prev, { url, agents: new Set(selectedAgentList().map((a) => a.path)) }]);
    setRepoInput("");
    setError("");
  };

  const removeRepo = (url: string) => {
    setRepos((prev) => prev.filter((r) => r.url !== url));
  };

  const toggleRepoAgent = (repoUrl: string, agentPath: string) => {
    setRepos((prev) =>
      prev.map((r) => {
        if (r.url !== repoUrl) return r;
        const next = new Set(r.agents);
        if (next.has(agentPath)) next.delete(agentPath);
        else next.add(agentPath);
        return { ...r, agents: next };
      })
    );
  };

  const toggleRepoAll = (repoUrl: string) => {
    setRepos((prev) =>
      prev.map((r) => {
        if (r.url !== repoUrl) return r;
        const allSelected = selectedAgentList().every((a) => r.agents.has(a.path));
        const next = allSelected ? new Set<string>() : new Set(selectedAgentList().map((a) => a.path));
        return { ...r, agents: next };
      })
    );
  };

  const repoDisplayName = (url: string) => {
    const match = url.match(/\/([^/]+?)(?:\.git)?$/);
    return match ? match[1] : url;
  };

  const handleSave = async () => {
    if (saving()) return;
    setSaving(true);
    setError("");
    try {
      const repoData = repos().map((r) => ({
        url: r.url,
        agents: Array.from(r.agents),
      }));
      await EntityAPI.updateTeam(
        props.projectPath,
        props.team.name,
        Array.from(selectedAgents()),
        coordinator(),
        repoData
      );
      await projectStore.reloadProject(props.projectPath);
      props.onClose();
    } catch (e: any) {
      console.error("update_team failed:", e);
      setError(typeof e === "string" ? e : e.message || "Failed to update team");
    } finally {
      setSaving(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
  };

  const handleOverlayClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("modal-overlay")) props.onClose();
  };

  return (
    <div class="modal-overlay" onClick={handleOverlayClick} onKeyDown={handleKeyDown}>
      <div class="agent-modal entity-wizard-modal">
        <div class="agent-modal-header">
          <span class="agent-modal-title">Edit Team: {props.team.name}</span>
          <span class="wizard-step-indicator">Step {step()} of 3</span>
        </div>

        <Show when={loading()}>
          <div class="wizard-body">
            <div class="wizard-loading">Loading team configuration...</div>
          </div>
        </Show>

        <Show when={!loading()}>
          {/* Step 1: Info (read-only name) */}
          <Show when={step() === 1}>
            <div class="new-agent-form">
              <div class="new-agent-field">
                <label class="new-agent-label">Team name</label>
                <input
                  class="agent-search-input"
                  value={props.team.name}
                  disabled
                />
              </div>
              <div class="new-agent-field">
                <label class="new-agent-label" style={{ opacity: 0.6 }}>
                  Team name cannot be changed after creation.
                </label>
              </div>
            </div>
            <div class="new-agent-footer">
              <button class="new-agent-cancel-btn" onClick={() => props.onClose()}>Cancel</button>
              <button
                class="new-agent-create-btn"
                onClick={() => setStep(2)}
              >
                Next
              </button>
            </div>
          </Show>

          {/* Step 2: Agent selection */}
          <Show when={step() === 2}>
            <div class="wizard-body">
              <Show when={allAgents().length === 0}>
                <div class="wizard-empty">No agents found in any project.</div>
              </Show>
              <Show when={allAgents().length > 0}>
                <div class="wizard-search-row">
                  <svg class="wizard-search-icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5">
                    <circle cx="6.5" cy="6.5" r="5" />
                    <line x1="10" y1="10" x2="14.5" y2="14.5" />
                  </svg>
                  <input
                    class="wizard-search-input"
                    value={agentFilter()}
                    onInput={(e) => setAgentFilter(e.currentTarget.value)}
                    placeholder="Filter agents..."
                  />
                </div>
                <For each={Array.from(agentsByProject().entries())}>
                  {([projectName, agents]) => (
                    <div class="wizard-agent-group">
                      <div class="wizard-group-title">{projectName}</div>
                      <For each={agents}>
                        {(agent) => {
                          const isSelected = () => selectedAgents().has(agent.path);
                          const isCoord = () => coordinator() === agent.path;
                          return (
                            <div class="wizard-agent-row">
                              <label class="wizard-checkbox-label">
                                <input
                                  type="checkbox"
                                  checked={isSelected()}
                                  onChange={() => toggleAgent(agent.path)}
                                />
                                <span class="wizard-agent-name">{agent.name}</span>
                              </label>
                              <Show when={isSelected()}>
                                <label class="wizard-coord-label" title="Set as coordinator">
                                  <input
                                    type="radio"
                                    name="coordinator"
                                    checked={isCoord()}
                                    onChange={() => setCoordinator(agent.path)}
                                  />
                                  <span class="wizard-coord-text">Coord</span>
                                </label>
                              </Show>
                            </div>
                          );
                        }}
                      </For>
                    </div>
                  )}
                </For>
              </Show>
              <Show when={error()}>
                <div class="new-agent-error">{error()}</div>
              </Show>
            </div>
            <div class="new-agent-footer">
              <button class="new-agent-cancel-btn" onClick={() => setStep(1)}>Back</button>
              <button
                class="new-agent-create-btn"
                disabled={!canNext2()}
                onClick={() => setStep(3)}
              >
                Next
              </button>
            </div>
          </Show>

          {/* Step 3: Repo assignment */}
          <Show when={step() === 3}>
            <div class="wizard-body">
              <div class="wizard-repo-input-row">
                <input
                  class="agent-search-input"
                  value={repoInput()}
                  onInput={(e) => { setRepoInput(e.currentTarget.value); setError(""); }}
                  placeholder="https://github.com/org/repo.git"
                  onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); addRepo(); } }}
                />
                <button class="new-agent-browse-btn" onClick={addRepo}>Add Repo</button>
              </div>

              <Show when={repos().length > 0}>
                <div class="wizard-repo-list">
                  <For each={repos()}>
                    {(repo) => {
                      const allChecked = () => selectedAgentList().every((a) => repo.agents.has(a.path));
                      return (
                        <div class="wizard-repo-card">
                          <div class="wizard-repo-header">
                            <span class="wizard-repo-name">{repoDisplayName(repo.url)}</span>
                            <button class="wizard-repo-remove" onClick={() => removeRepo(repo.url)} title="Remove repo">
                              &#x2715;
                            </button>
                          </div>
                          <div class="wizard-repo-agents">
                            <label class="wizard-checkbox-label wizard-all-label">
                              <input
                                type="checkbox"
                                checked={allChecked()}
                                onChange={() => toggleRepoAll(repo.url)}
                              />
                              <span>All agents</span>
                            </label>
                            <For each={selectedAgentList()}>
                              {(agent) => (
                                <label class="wizard-checkbox-label">
                                  <input
                                    type="checkbox"
                                    checked={repo.agents.has(agent.path)}
                                    onChange={() => toggleRepoAgent(repo.url, agent.path)}
                                  />
                                  <span>{agent.name}</span>
                                </label>
                              )}
                            </For>
                          </div>
                        </div>
                      );
                    }}
                  </For>
                </div>
              </Show>

              <Show when={repos().length === 0}>
                <div class="wizard-empty">No repos assigned. Add repo URLs above.</div>
              </Show>

              <Show when={error()}>
                <div class="new-agent-error">{error()}</div>
              </Show>
            </div>
            <div class="new-agent-footer">
              <button class="new-agent-cancel-btn" onClick={() => setStep(2)}>Back</button>
              <button
                class="new-agent-create-btn"
                disabled={saving()}
                onClick={handleSave}
              >
                {saving() ? "Saving..." : "Save"}
              </button>
            </div>
          </Show>
        </Show>
      </div>
    </div>
  );
};

export default EditTeamModal;
