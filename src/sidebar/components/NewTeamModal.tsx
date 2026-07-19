import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import type { Component } from "solid-js";
import { EntityAPI } from "../../shared/ipc";
import { projectStore } from "../stores/project";
import { TeamContextAlertsEditor } from "./TeamContextAlertsEditor";
import type { ContextAlertThresholdDraft } from "./team-context-alerts";
import { validateContextAlertThresholdDrafts } from "./team-context-alerts";

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

function normalizeCaughtError(error: unknown, fallback: string): string {
  if (typeof error === "string") return error;
  if (error instanceof Error && error.message) return error.message;
  return fallback;
}

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
  const [contextAlertDrafts, setContextAlertDrafts] =
    createSignal<ContextAlertThresholdDraft[]>([]);
  const [repoError, setRepoError] = createSignal("");
  const [createError, setCreateError] = createSignal("");
  const [creating, setCreating] = createSignal(false);
  const [loadingAgents, setLoadingAgents] = createSignal(false);
  const [agentFilter, setAgentFilter] = createSignal("");
  let nameRef!: HTMLInputElement;

  const contextAlertValidation = createMemo(() =>
    validateContextAlertThresholdDrafts(contextAlertDrafts()),
  );

  const currentProjectName = createMemo(() => {
    const path = props.projectPath.replace(/[\\/]+$/, "");
    const separatorIndex = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    return separatorIndex >= 0 ? path.slice(separatorIndex + 1) : path;
  });

  const agentsByProject = createMemo(() => {
    const filter = agentFilter().toLowerCase();
    const filtered = filter
      ? allAgents().filter((agent) => agent.name.toLowerCase().includes(filter))
      : allAgents();

    const grouped = new Map<string, AgentEntry[]>();
    for (const agent of filtered) {
      const entries = grouped.get(agent.projectName) ?? [];
      entries.push(agent);
      grouped.set(agent.projectName, entries);
    }

    const current = currentProjectName();
    const entries = Array.from(grouped.entries());
    entries.sort((left, right) => {
      if (left[0] === current) return -1;
      if (right[0] === current) return 1;
      return left[0].localeCompare(right[0]);
    });
    return new Map(entries);
  });

  const selectedAgentList = createMemo(() =>
    allAgents().filter((agent) => selectedAgents().has(agent.path)),
  );

  const portableAgentRef = (path: string): string => {
    const last = path.replace(/\\/g, "/").split("/").filter(Boolean).pop() ?? path;
    return last.startsWith("_agent_") ? last : `_agent_${last}`;
  };

  const canNext1 = createMemo(() => {
    const name = teamName().trim();
    return name.length > 0 && !name.includes("/") && !name.includes("\\") && !name.includes(" ");
  });

  const canNext2 = createMemo(() => selectedAgents().size > 0 && coordinator() !== "");

  const canCreate = createMemo(() =>
    !creating() && canNext1() && canNext2() && contextAlertValidation().valid,
  );

  onMount(async () => {
    setLoadingAgents(true);
    try {
      const paths = projectStore.projects.map((project) => project.path);
      const result = await EntityAPI.listAllAgents(paths);
      setAllAgents(
        result.map((agent) => ({
          name: agent.name,
          path: agent.path,
          projectName: agent.projectName,
        })),
      );
    } catch (error: unknown) {
      console.error("list_all_agents failed:", error);
    } finally {
      setLoadingAgents(false);
    }
  });

  const toggleAgent = (path: string) => {
    setSelectedAgents((previous) => {
      const next = new Set(previous);
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
    if (creating()) return;
    const url = repoInput().trim();
    if (!url) return;
    if (repos().some((repo) => repo.url === url)) {
      setRepoError("Repo already added");
      return;
    }
    setRepos((previous) => [
      ...previous,
      { url, agents: new Set(selectedAgentList().map((agent) => agent.path)) },
    ]);
    setRepoInput("");
    setRepoError("");
  };

  const removeRepo = (url: string) => {
    if (creating()) return;
    setRepos((previous) => previous.filter((repo) => repo.url !== url));
  };

  const toggleRepoAgent = (repoUrl: string, agentPath: string) => {
    if (creating()) return;
    setRepos((previous) =>
      previous.map((repo) => {
        if (repo.url !== repoUrl) return repo;
        const nextAgents = new Set(repo.agents);
        if (nextAgents.has(agentPath)) nextAgents.delete(agentPath);
        else nextAgents.add(agentPath);
        return { ...repo, agents: nextAgents };
      }),
    );
  };

  const toggleRepoAll = (repoUrl: string) => {
    if (creating()) return;
    setRepos((previous) =>
      previous.map((repo) => {
        if (repo.url !== repoUrl) return repo;
        const allSelected = selectedAgentList().every((agent) => repo.agents.has(agent.path));
        const nextAgents = allSelected
          ? new Set<string>()
          : new Set(selectedAgentList().map((agent) => agent.path));
        return { ...repo, agents: nextAgents };
      }),
    );
  };

  const repoDisplayName = (url: string): string => {
    const match = url.match(/\/([^/]+?)(?:\.git)?$/);
    return match ? match[1] : url;
  };

  const handleCreate = async () => {
    const validation = contextAlertValidation();
    if (
      creating()
      || !canNext1()
      || !canNext2()
      || !validation.valid
    ) {
      return;
    }

    const request = {
      projectPath: props.projectPath,
      name: teamName().trim(),
      agents: Array.from(selectedAgents()).map(portableAgentRef),
      coordinator: portableAgentRef(coordinator()),
      repos: repos().map((repo) => ({
        url: repo.url,
        agents: Array.from(repo.agents).map(portableAgentRef),
      })),
      contextAlertPercentages: [...validation.canonicalPercentages],
    };

    setCreating(true);
    setCreateError("");
    try {
      await EntityAPI.createTeam(
        request.projectPath,
        request.name,
        request.agents,
        request.coordinator,
        request.repos,
        request.contextAlertPercentages,
      );
      await projectStore.reloadProject(request.projectPath);
      props.onClose();
    } catch (error: unknown) {
      console.error("create_team failed:", error);
      setCreateError(normalizeCaughtError(error, "Failed to create team"));
    } finally {
      setCreating(false);
    }
  };

  const handleKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Enter" && !event.shiftKey && step() === 1) {
      event.preventDefault();
      if (canNext1()) setStep(2);
    }
  };

  const handleDocumentKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape" && !creating()) props.onClose();
  };

  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  return (
    <div class="modal-overlay" onKeyDown={handleKeyDown}>
      <div
        class="agent-modal entity-wizard-modal"
        aria-busy={creating() ? "true" : "false"}
      >
        <div class="agent-modal-header">
          <span class="agent-modal-title">New Team</span>
          <span class="wizard-step-indicator">Step {step()} of 3</span>
        </div>

        <Show when={step() === 1}>
          <div class="new-agent-form">
            <div class="new-agent-field">
              <label class="new-agent-label">Team name</label>
              <input
                ref={nameRef!}
                class="agent-search-input"
                value={teamName()}
                onInput={(event) => setTeamName(event.currentTarget.value)}
                placeholder="dream-team"
                autofocus
              />
            </div>
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

        <Show when={step() === 2}>
          <div class="wizard-body">
            <Show when={loadingAgents()}>
              <div class="wizard-loading">Loading agents...</div>
            </Show>
            <Show when={!loadingAgents() && allAgents().length === 0}>
              <div class="wizard-empty">No agents found in any project.</div>
            </Show>
            <Show when={!loadingAgents() && allAgents().length > 0}>
              <div class="wizard-search-row">
                <svg class="wizard-search-icon" viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.5">
                  <circle cx="6.5" cy="6.5" r="5" />
                  <line x1="10" y1="10" x2="14.5" y2="14.5" />
                </svg>
                <input
                  class="wizard-search-input"
                  value={agentFilter()}
                  onInput={(event) => setAgentFilter(event.currentTarget.value)}
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
                        const isCoordinator = () => coordinator() === agent.path;
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
                                  checked={isCoordinator()}
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

        <Show when={step() === 3}>
          <div class="wizard-body">
            <TeamContextAlertsEditor
              idPrefix="new-team-context-alerts"
              drafts={contextAlertDrafts()}
              validation={contextAlertValidation()}
              disabled={creating()}
              onChange={(next) => {
                if (creating()) return;
                setContextAlertDrafts(next);
              }}
            />

            <section class="team-settings-section" aria-labelledby="new-team-repositories-heading">
              <h3 id="new-team-repositories-heading" class="team-settings-heading">
                Repositories (optional)
              </h3>
              <div class="wizard-repo-input-row">
                <input
                  class="agent-search-input"
                  value={repoInput()}
                  disabled={creating()}
                  onInput={(event) => {
                    if (creating()) {
                      event.currentTarget.value = repoInput();
                      return;
                    }
                    setRepoInput(event.currentTarget.value);
                    setRepoError("");
                  }}
                  placeholder="https://github.com/org/repo.git"
                  onKeyDown={(event) => {
                    if (creating()) return;
                    if (event.key === "Enter") {
                      event.preventDefault();
                      addRepo();
                    }
                  }}
                />
                <button
                  class="new-agent-browse-btn"
                  type="button"
                  disabled={creating()}
                  onClick={addRepo}
                >
                  Add Repo
                </button>
              </div>

              <Show when={repos().length > 0}>
                <div class="wizard-repo-list">
                  <For each={repos()}>
                    {(repo) => {
                      const allChecked = () =>
                        selectedAgentList().every((agent) => repo.agents.has(agent.path));
                      return (
                        <div class="wizard-repo-card">
                          <div class="wizard-repo-header">
                            <span class="wizard-repo-name">{repoDisplayName(repo.url)}</span>
                            <button
                              class="wizard-repo-remove"
                              type="button"
                              disabled={creating()}
                              onClick={() => removeRepo(repo.url)}
                              title="Remove repo"
                            >
                              &#x2715;
                            </button>
                          </div>
                          <div class="wizard-repo-agents">
                            <label class="wizard-checkbox-label wizard-all-label">
                              <input
                                type="checkbox"
                                checked={allChecked()}
                                disabled={creating()}
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
                                    disabled={creating()}
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

              <Show when={repoError()}>
                <div
                  id="new-team-repository-error"
                  class="new-agent-error"
                  role="alert"
                  aria-label="Repository error"
                >
                  {repoError()}
                </div>
              </Show>
            </section>

            <Show when={createError()}>
              <div
                id="new-team-create-error"
                class="new-agent-error"
                role="alert"
                aria-label="Team creation error"
              >
                {createError()}
              </div>
            </Show>
          </div>
          <div class="new-agent-footer">
            <button
              class="new-agent-cancel-btn"
              disabled={creating()}
              onClick={() => {
                if (!creating()) setStep(2);
              }}
            >
              Back
            </button>
            <button
              class="new-agent-create-btn"
              disabled={!canCreate()}
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
