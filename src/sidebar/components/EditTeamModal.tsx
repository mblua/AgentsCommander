import { createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import type { Component } from "solid-js";
import { CANONICAL_AC_ROOT_DIR } from "../../shared/constants";
import { EntityAPI } from "../../shared/ipc";
import type {
  TeamWizardAgentEntry,
  TeamWizardRepoEntry,
  TeamWizardStep,
} from "../../shared/types";
import { projectStore } from "../stores/project";
import { TeamContextAlertsEditor } from "./TeamContextAlertsEditor";
import type { ContextAlertThresholdDraft } from "./team-context-alerts";
import {
  hydrateContextAlertThresholdDrafts,
  validateContextAlertThresholdDrafts,
} from "./team-context-alerts";

function normalizeCaughtError(error: unknown, fallback: string): string {
  if (typeof error === "string") return error;
  if (error instanceof Error && error.message) return error.message;
  return fallback;
}

const EditTeamModal: Component<{
  projectPath: string;
  teamName: string;
  onClose: () => void;
}> = (props) => {
  const [step, setStep] = createSignal<TeamWizardStep>(1);
  const [allAgents, setAllAgents] = createSignal<TeamWizardAgentEntry[]>([]);
  const [selectedAgents, setSelectedAgents] = createSignal<Set<string>>(new Set());
  const [coordinator, setCoordinator] = createSignal<string>("");
  const [repos, setRepos] = createSignal<TeamWizardRepoEntry[]>([]);
  const [repoInput, setRepoInput] = createSignal("");
  const [contextAlertDrafts, setContextAlertDrafts] =
    createSignal<ContextAlertThresholdDraft[]>([]);
  const [loadError, setLoadError] = createSignal("");
  const [repoError, setRepoError] = createSignal("");
  const [saveError, setSaveError] = createSignal("");
  const [saving, setSaving] = createSignal(false);
  const [loading, setLoading] = createSignal(true);
  const [configLoaded, setConfigLoaded] = createSignal(false);
  const [agentFilter, setAgentFilter] = createSignal("");

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

    const grouped = new Map<string, TeamWizardAgentEntry[]>();
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

  const canNext2 = createMemo(() => selectedAgents().size > 0 && coordinator() !== "");

  const canSave = createMemo(() =>
    configLoaded()
    && !saving()
    && canNext2()
    && contextAlertValidation().valid,
  );

  onMount(async () => {
    try {
      const paths = projectStore.projects.map((project) => project.path);
      const [agentList, teamConfig] = await Promise.all([
        EntityAPI.listAllAgents(paths),
        EntityAPI.getTeamConfig(props.projectPath, props.teamName),
      ]);

      const entries: TeamWizardAgentEntry[] = agentList.map((agent) => ({
        name: agent.name,
        path: agent.path,
        projectName: agent.projectName,
      }));
      const discoveredRefs = new Set(entries.map((entry) => portableAgentRef(entry.path)));

      for (const configPath of teamConfig.agents) {
        const agentRef = portableAgentRef(configPath);
        if (!discoveredRefs.has(agentRef)) {
          const fallbackPath =
            `${props.projectPath.replace(/[\\/]+$/, "")}/${CANONICAL_AC_ROOT_DIR}/${agentRef}`;
          entries.push({
            name: agentRef.replace(/^_agent_/, ""),
            path: fallbackPath,
            projectName: currentProjectName(),
          });
        }
      }

      const entryPathByRef = new Map(
        entries.map((entry) => [portableAgentRef(entry.path), entry.path]),
      );
      const configPathForUi = (agentRef: string): string =>
        entryPathByRef.get(portableAgentRef(agentRef)) ?? agentRef;
      const configAgentRefs = new Set(teamConfig.agents.map(portableAgentRef));
      const nextSelectedAgents = new Set<string>();
      for (const entry of entries) {
        if (configAgentRefs.has(portableAgentRef(entry.path))) {
          nextSelectedAgents.add(entry.path);
        }
      }

      let nextCoordinator = "";
      if (teamConfig.coordinator) {
        const coordinatorRef = portableAgentRef(teamConfig.coordinator);
        const coordinatorEntry = entries.find(
          (entry) => portableAgentRef(entry.path) === coordinatorRef,
        );
        if (coordinatorEntry) nextCoordinator = coordinatorEntry.path;
      }

      const nextRepos = teamConfig.repos.map((repo) => ({
        url: repo.url,
        agents: new Set(repo.agents.map(configPathForUi)),
      }));
      const nextContextAlertDrafts = hydrateContextAlertThresholdDrafts(
        teamConfig.contextAlertPercentages,
      );

      setAllAgents(entries);
      setSelectedAgents(nextSelectedAgents);
      setCoordinator(nextCoordinator);
      setRepos(nextRepos);
      setContextAlertDrafts(nextContextAlertDrafts);
      setConfigLoaded(true);
    } catch (error: unknown) {
      console.error("Failed to load team config:", error);
      setLoadError(normalizeCaughtError(error, "Failed to load team configuration"));
    } finally {
      setLoading(false);
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
    if (saving()) return;
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
    if (saving()) return;
    setRepos((previous) => previous.filter((repo) => repo.url !== url));
  };

  const toggleRepoAgent = (repoUrl: string, agentPath: string) => {
    if (saving()) return;
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
    if (saving()) return;
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

  const handleSave = async () => {
    const validation = contextAlertValidation();
    if (
      saving()
      || !configLoaded()
      || !canNext2()
      || !validation.valid
    ) {
      return;
    }

    const request = {
      projectPath: props.projectPath,
      teamName: props.teamName,
      agents: Array.from(selectedAgents()).map(portableAgentRef),
      coordinator: portableAgentRef(coordinator()),
      repos: repos().map((repo) => ({
        url: repo.url,
        agents: Array.from(repo.agents).map(portableAgentRef),
      })),
      contextAlertPercentages: [...validation.canonicalPercentages],
    };

    setSaving(true);
    setSaveError("");
    try {
      await EntityAPI.updateTeam(
        request.projectPath,
        request.teamName,
        request.agents,
        request.coordinator,
        request.repos,
        request.contextAlertPercentages,
      );
      await projectStore.reloadProject(request.projectPath);
      props.onClose();
    } catch (error: unknown) {
      console.error("update_team failed:", error);
      setSaveError(normalizeCaughtError(error, "Failed to update team"));
    } finally {
      setSaving(false);
    }
  };

  const handleDocumentKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape" && !saving()) props.onClose();
  };

  document.addEventListener("keydown", handleDocumentKeyDown);
  onCleanup(() => document.removeEventListener("keydown", handleDocumentKeyDown));

  return (
    <div class="modal-overlay">
      <div
        class="agent-modal entity-wizard-modal"
        aria-busy={saving() ? "true" : "false"}
      >
        <div class="agent-modal-header">
          <span class="agent-modal-title">Edit Team: {props.teamName}</span>
          <span class="wizard-step-indicator">Step {step()} of 3</span>
        </div>

        <Show when={loading()}>
          <div class="wizard-body">
            <div class="wizard-loading">Loading team configuration...</div>
          </div>
        </Show>

        <Show when={!loading() && !configLoaded()}>
          <div class="wizard-body">
            <div
              id="edit-team-load-error"
              class="new-agent-error"
              role="alert"
              aria-label="Team configuration load error"
            >
              {loadError()}
            </div>
          </div>
          <div class="new-agent-footer">
            <button class="new-agent-cancel-btn" onClick={() => props.onClose()}>Cancel</button>
          </div>
        </Show>

        <Show when={!loading() && configLoaded()}>
          <Show when={step() === 1}>
            <div class="new-agent-form">
              <div class="new-agent-field">
                <label class="new-agent-label">Team name</label>
                <input class="agent-search-input" value={props.teamName} disabled />
              </div>
              <div class="new-agent-field">
                <label class="new-agent-label" style={{ opacity: 0.6 }}>
                  Team name cannot be changed after creation.
                </label>
              </div>
            </div>
            <div class="new-agent-footer">
              <button class="new-agent-cancel-btn" onClick={() => props.onClose()}>Cancel</button>
              <button class="new-agent-create-btn" onClick={() => setStep(2)}>Next</button>
            </div>
          </Show>

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
                idPrefix="edit-team-context-alerts"
                drafts={contextAlertDrafts()}
                validation={contextAlertValidation()}
                disabled={saving()}
                onChange={(next) => {
                  if (saving()) return;
                  setContextAlertDrafts(next);
                }}
              />

              <section class="team-settings-section" aria-labelledby="edit-team-repositories-heading">
                <h3 id="edit-team-repositories-heading" class="team-settings-heading">
                  Repositories (optional)
                </h3>
                <div class="wizard-repo-input-row">
                  <input
                    class="agent-search-input"
                    value={repoInput()}
                    disabled={saving()}
                    onInput={(event) => {
                      if (saving()) {
                        event.currentTarget.value = repoInput();
                        return;
                      }
                      setRepoInput(event.currentTarget.value);
                      setRepoError("");
                    }}
                    placeholder="https://github.com/org/repo.git"
                    onKeyDown={(event) => {
                      if (saving()) return;
                      if (event.key === "Enter") {
                        event.preventDefault();
                        addRepo();
                      }
                    }}
                  />
                  <button
                    class="new-agent-browse-btn"
                    type="button"
                    disabled={saving()}
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
                                disabled={saving()}
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
                                  disabled={saving()}
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
                                      disabled={saving()}
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

                <Show when={repoError()}>
                  <div
                    id="edit-team-repository-error"
                    class="new-agent-error"
                    role="alert"
                    aria-label="Repository error"
                  >
                    {repoError()}
                  </div>
                </Show>
              </section>

              <Show when={saveError()}>
                <div
                  id="edit-team-save-error"
                  class="new-agent-error"
                  role="alert"
                  aria-label="Team save error"
                >
                  {saveError()}
                </div>
              </Show>
            </div>
            <div class="new-agent-footer">
              <button
                class="new-agent-cancel-btn"
                disabled={saving()}
                onClick={() => {
                  if (!saving()) setStep(2);
                }}
              >
                Back
              </button>
              <button
                class="new-agent-create-btn"
                disabled={!canSave()}
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
