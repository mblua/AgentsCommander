import { Component, createSignal, createMemo, For, Show, onMount } from "solid-js";
import { EntityAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";

interface AgentEntry {
  name: string;
  path: string;
  projectName: string;
}

interface RepoEntry {
  url: string;
  agents: Set<string>;
}

type Step = 1 | 2 | 3;

const NewTeamModal: Component<{
  projectPath: string;
  onClose: () => void;
}> = (props) => {
  const [step, setStep] = createSignal<Step>(1);
  const [teamName, setTeamName] = createSignal("");
  const [allAgents, setAllAgents] = createSignal<AgentEntry[]>([]);
  const [selectedAgents, setSelectedAgents] = createSignal<Set<string>>(new Set());
  const [coordinator, setCoordinator] = createSignal<string>("");
  const [repos, setRepos] = createSignal<RepoEntry[]>([]);
  const [repoInput, setRepoInput] = createSignal("");
  const [error, setError] = createSignal("");
  const [creating, setCreating] = createSignal(false);
  const [loadingAgents, setLoadingAgents] = createSignal(false);
  let nameRef!: HTMLInputElement;

  // Group agents by project
  const agentsByProject = createMemo(() => {
    const map = new Map<string, AgentEntry[]>();
    for (const a of allAgents()) {
      const list = map.get(a.projectName) ?? [];
      list.push(a);
      map.set(a.projectName, list);
    }
    return map;
  });

  const selectedAgentList = createMemo(() =>
    allAgents().filter((a) => selectedAgents().has(a.path))
  );

  const canNext1 = createMemo(() => {
    const n = teamName().trim();
    return n.length > 0 && !n.includes("/") && !n.includes("\\") && !n.includes(" ");
  });

  const canNext2 = createMemo(() =>
    selectedAgents().size > 0 && coordinator() !== ""
  );

  onMount(async () => {
    setLoadingAgents(true);
    try {
      const paths = projectStore.projects.map((p) => p.path);
      const result = await EntityAPI.listAllAgents(paths);
      const entries: AgentEntry[] = result.map((a) => ({
        name: a.name,
        path: a.path,
        projectName: a.projectName,
      }));
      setAllAgents(entries);
    } catch (e) {
      console.error("list_all_agents failed:", e);
    } finally {
      setLoadingAgents(false);
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

  const handleCreate = async () => {
    if (creating()) return;
    setCreating(true);
    setError("");
    try {
      const repoData = repos().map((r) => ({
        url: r.url,
        agents: Array.from(r.agents),
      }));
      await EntityAPI.createTeam(
        props.projectPath,
        teamName().trim(),
        Array.from(selectedAgents()),
        coordinator(),
        repoData
      );
      await projectStore.reloadProject(props.projectPath);
      props.onClose();
    } catch (e: any) {
      console.error("create_team failed:", e);
      setError(typeof e === "string" ? e : e.message || "Failed to create team");
    } finally {
      setCreating(false);
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Escape") props.onClose();
    if (e.key === "Enter" && !e.shiftKey && step() === 1) {
      e.preventDefault();
      if (canNext1()) setStep(2);
    }
  };

  const handleOverlayClick = (e: MouseEvent) => {
    if ((e.target as HTMLElement).classList.contains("modal-overlay")) props.onClose();
  };

  return (
    <div class="modal-overlay" onClick={handleOverlayClick} onKeyDown={handleKeyDown}>
      <div class="agent-modal entity-wizard-modal">
        <div class="agent-modal-header">
          <span class="agent-modal-title">New Team</span>
          <span class="wizard-step-indicator">Step {step()} of 3</span>
        </div>

        {/* Step 1: Name */}
        <Show when={step() === 1}>
          <div class="new-agent-form">
            <div class="new-agent-field">
              <label class="new-agent-label">Team name</label>
              <input
                ref={nameRef!}
                class="agent-search-input"
                value={teamName()}
                onInput={(e) => { setTeamName(e.currentTarget.value); setError(""); }}
                placeholder="dream-team"
                autofocus
              />
            </div>
            <Show when={error()}>
              <div class="new-agent-error">{error()}</div>
            </Show>
          </div>
          <div class="new-agent-footer">
            <button class="new-agent-cancel-btn" onClick={() => props.onClose()}>Cancel</button>
            <button
              class="new-agent-create-btn"
              disabled={!canNext1()}
              onClick={() => setStep(2)}
            >
              Next
            </button>
          </div>
        </Show>

        {/* Step 2: Agent selection */}
        <Show when={step() === 2}>
          <div class="wizard-body">
            <Show when={loadingAgents()}>
              <div class="wizard-loading">Loading agents...</div>
            </Show>
            <Show when={!loadingAgents() && allAgents().length === 0}>
              <div class="wizard-empty">No agents found in any project.</div>
            </Show>
            <Show when={!loadingAgents() && allAgents().length > 0}>
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
              <div class="wizard-empty">No repos added yet. Add repo URLs above.</div>
            </Show>

            <Show when={error()}>
              <div class="new-agent-error">{error()}</div>
            </Show>
          </div>
          <div class="new-agent-footer">
            <button class="new-agent-cancel-btn" onClick={() => setStep(2)}>Back</button>
            <button
              class="new-agent-create-btn"
              disabled={creating()}
              onClick={handleCreate}
            >
              {creating() ? "Creating..." : "Create"}
            </button>
          </div>
        </Show>
      </div>
    </div>
  );
};

export default NewTeamModal;
