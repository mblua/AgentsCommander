import { Component, createSignal, For, Show, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcAgentMatrix, AcTeam, AcWorkgroup, AcAgentReplica } from "../../shared/types";
import { AcDiscoveryAPI, SessionAPI } from "../../shared/ipc";

const AcDiscoveryPanel: Component = () => {
  const [agents, setAgents] = createSignal<AcAgentMatrix[]>([]);
  const [teams, setTeams] = createSignal<AcTeam[]>([]);
  const [workgroups, setWorkgroups] = createSignal<AcWorkgroup[]>([]);
  const [collapsed, setCollapsed] = createSignal(false);
  const [wgCollapsed, setWgCollapsed] = createSignal(false);
  const [loading, setLoading] = createSignal(true);

  /** Find which teams an agent belongs to */
  const teamsForAgent = (agentName: string): string[] => {
    return teams()
      .filter((t) => t.agents.includes(agentName))
      .map((t) => t.name);
  };

  /** Check if agent is a coordinator of any team */
  const isCoordinator = (agentName: string): boolean => {
    return teams().some((t) => t.coordinator === agentName);
  };

  const handleAgentClick = (agent: AcAgentMatrix) => {
    SessionAPI.create({
      cwd: agent.path,
      sessionName: agent.name,
      agentId: agent.preferredAgentId,
    });
  };

  const handleReplicaClick = (replica: AcAgentReplica, wg: AcWorkgroup) => {
    const repoPaths = replica.repoPaths ?? [];
    let gitBranchSource: string | undefined;
    let gitBranchPrefix: string | undefined;

    if (repoPaths.length === 1) {
      gitBranchSource = repoPaths[0];
      // Derive repo name from directory name, strip "repo-" prefix if present
      const dirName = repoPaths[0].replace(/\\/g, "/").split("/").pop() ?? "";
      gitBranchPrefix = dirName.startsWith("repo-") ? dirName.slice(5) : dirName;
    } else if (repoPaths.length > 1) {
      gitBranchPrefix = "multi-repo";
    }

    SessionAPI.create({
      cwd: replica.path,
      sessionName: `${wg.name}/${replica.name}`,
      agentId: replica.preferredAgentId,
      gitBranchSource,
      gitBranchPrefix,
    });
  };

  // --- Context menu state for replicas ---
  const [ctxMenuPos, setCtxMenuPos] = createSignal({ x: 0, y: 0 });
  const [ctxMenuReplica, setCtxMenuReplica] = createSignal<AcAgentReplica | null>(null);

  // --- Context files panel state ---
  const [ctxFilesReplica, setCtxFilesReplica] = createSignal<AcAgentReplica | null>(null);
  const [ctxFiles, setCtxFiles] = createSignal<string[]>([]);
  const [ctxFilesLoading, setCtxFilesLoading] = createSignal(false);
  const [newCtxPath, setNewCtxPath] = createSignal("");

  let dismissCtxMenu: (() => void) | null = null;

  const cleanupCtxMenu = () => {
    if (dismissCtxMenu) {
      window.removeEventListener("click", dismissCtxMenu);
      window.removeEventListener("contextmenu", dismissCtxMenu);
      window.removeEventListener("keydown", dismissCtxMenu as any);
      dismissCtxMenu = null;
    }
  };

  onCleanup(cleanupCtxMenu);

  const handleReplicaContextMenu = (e: MouseEvent, replica: AcAgentReplica) => {
    e.preventDefault();
    e.stopPropagation();
    cleanupCtxMenu();
    setCtxMenuPos({ x: e.clientX, y: e.clientY });
    setCtxMenuReplica(replica);
    const dismiss = (ev?: Event) => {
      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
      setCtxMenuReplica(null);
      cleanupCtxMenu();
    };
    dismissCtxMenu = dismiss;
    setTimeout(() => {
      window.addEventListener("click", dismiss);
      window.addEventListener("contextmenu", dismiss);
      window.addEventListener("keydown", dismiss as any);
    });
  };

  const openContextFilesPanel = async (replica: AcAgentReplica) => {
    setCtxMenuReplica(null);
    cleanupCtxMenu();
    setCtxFilesReplica(replica);
    setCtxFilesLoading(true);
    try {
      const files = await AcDiscoveryAPI.getReplicaContextFiles(replica.path);
      setCtxFiles(files);
    } catch (e) {
      console.error("Failed to load context files:", e);
      setCtxFiles([]);
    } finally {
      setCtxFilesLoading(false);
    }
  };

  const handleRemoveCtxFile = async (index: number) => {
    const replica = ctxFilesReplica();
    if (!replica) return;
    const prev = ctxFiles();
    const updated = prev.filter((_, i) => i !== index);
    setCtxFiles(updated);
    try {
      await AcDiscoveryAPI.setReplicaContextFiles(replica.path, updated);
    } catch (e) {
      console.error("Failed to save context files:", e);
      setCtxFiles(prev);
    }
  };

  const handleAddCtxFile = async () => {
    const replica = ctxFilesReplica();
    const path = newCtxPath().trim();
    if (!replica || !path) return;
    const prev = ctxFiles();
    const updated = [...prev, path];
    setCtxFiles(updated);
    setNewCtxPath("");
    try {
      await AcDiscoveryAPI.setReplicaContextFiles(replica.path, updated);
    } catch (e) {
      console.error("Failed to save context files:", e);
      setCtxFiles(prev);
      setNewCtxPath(path);
    }
  };

  const closeContextFilesPanel = () => {
    setCtxFilesReplica(null);
    setCtxFiles([]);
    setNewCtxPath("");
  };

  onMount(async () => {
    try {
      const result = await AcDiscoveryAPI.discover();
      setAgents(result.agents);
      setTeams(result.teams);
      setWorkgroups(result.workgroups);
    } catch (e) {
      console.error("AC discovery failed:", e);
    } finally {
      setLoading(false);
    }
  });

  return (
    <Show when={!loading() && (agents().length > 0 || workgroups().length > 0)}>
      <div class="ac-discovery-panel">
        <button
          class="ac-discovery-header"
          onClick={() => setCollapsed((c) => !c)}
        >
          <span class="ac-discovery-chevron" classList={{ collapsed: collapsed() }}>
            &#x25BE;
          </span>
          <span class="ac-discovery-title">AC Agents</span>
          <span class="ac-discovery-count">{agents().length}</span>
        </button>
        <Show when={!collapsed()}>
          <div class="ac-discovery-list">
            <For each={agents()}>
              {(agent) => {
                const agentTeams = () => teamsForAgent(agent.name);
                const coord = () => isCoordinator(agent.name);
                return (
                  <div
                    class="ac-discovery-item"
                    onClick={() => handleAgentClick(agent)}
                    title={agent.path}
                  >
                    <div class="ac-discovery-item-info">
                      <span class="ac-discovery-item-name">
                        <span class="ac-discovery-prefix">
                          {agent.name.slice(0, agent.name.lastIndexOf("/") + 1)}
                        </span>
                        {agent.name.slice(agent.name.lastIndexOf("/") + 1)}
                      </span>
                      <div class="ac-discovery-badges">
                        <Show when={coord()}>
                          <span class="ac-discovery-badge coord">C</span>
                        </Show>
                        <For each={agentTeams()}>
                          {(teamName) => (
                            <span class="ac-discovery-badge team">{teamName}</span>
                          )}
                        </For>
                        <Show when={!agent.roleExists}>
                          <span class="ac-discovery-badge no-role">no role</span>
                        </Show>
                      </div>
                    </div>
                  </div>
                );
              }}
            </For>
          </div>
        </Show>
        <Show when={workgroups().length > 0}>
          <button
            class="ac-discovery-header"
            onClick={() => setWgCollapsed((c) => !c)}
          >
            <span class="ac-discovery-chevron" classList={{ collapsed: wgCollapsed() }}>
              &#x25BE;
            </span>
            <span class="ac-discovery-title">Workgroups</span>
            <span class="ac-discovery-count">{workgroups().length}</span>
          </button>
          <Show when={!wgCollapsed()}>
            <div class="ac-discovery-list">
              <For each={workgroups()}>
                {(wg) => (
                  <div class="ac-wg-group">
                    <div class="ac-wg-header" title={wg.path}>
                      <span class="ac-wg-name">{wg.name}</span>
                      <Show when={wg.brief}>
                        <span class="ac-wg-brief">{wg.brief}</span>
                      </Show>
                    </div>
                    <For each={wg.agents}>
                      {(replica) => {
                        const repoCount = () => replica.repoPaths.length;
                        const branchLabel = () => {
                          if (repoCount() === 1) return replica.repoBranch ?? "1 repo";
                          if (repoCount() > 1) return "multi-repo";
                          return null;
                        };
                        return (
                          <div
                            class="ac-discovery-item"
                            onClick={() => handleReplicaClick(replica, wg)}
                            onContextMenu={(e) => handleReplicaContextMenu(e, replica)}
                            title={replica.path}
                          >
                            <div class="ac-discovery-item-info">
                              <span class="ac-discovery-item-name">{replica.name}</span>
                              <div class="ac-discovery-badges">
                                <Show when={branchLabel()}>
                                  <span class="ac-discovery-badge branch">{branchLabel()}</span>
                                </Show>
                                <span class="ac-discovery-badge team">replica</span>
                              </div>
                            </div>
                          </div>
                        );
                      }}
                    </For>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </Show>
      </div>

      {/* Replica context menu */}
      {ctxMenuReplica() && (
        <Portal>
          <div
            class="session-context-menu"
            style={{ left: `${ctxMenuPos().x}px`, top: `${ctxMenuPos().y}px` }}
            onClick={(e) => e.stopPropagation()}
          >
            <button
              class="session-context-option"
              onClick={() => openContextFilesPanel(ctxMenuReplica()!)}
            >
              Context Files
            </button>
          </div>
        </Portal>
      )}

      {/* Context files panel */}
      {ctxFilesReplica() && (
        <Portal>
          <div class="ctx-files-overlay" onClick={closeContextFilesPanel}>
            <div class="ctx-files-panel" onClick={(e) => e.stopPropagation()}>
              <div class="ctx-files-header">
                <span class="ctx-files-title">
                  Context Files — {ctxFilesReplica()!.name}
                </span>
                <button class="ctx-files-close" onClick={closeContextFilesPanel}>
                  &times;
                </button>
              </div>
              <Show when={!ctxFilesLoading()} fallback={<div class="ctx-files-loading">Loading...</div>}>
                <div class="ctx-files-list">
                  <Show when={ctxFiles().length === 0}>
                    <div class="ctx-files-empty">No context files configured</div>
                  </Show>
                  <For each={ctxFiles()}>
                    {(file, i) => (
                      <div class="ctx-files-item">
                        <span class="ctx-files-path" title={file}>{file}</span>
                        <button
                          class="ctx-files-remove"
                          onClick={() => handleRemoveCtxFile(i())}
                        >
                          &times;
                        </button>
                      </div>
                    )}
                  </For>
                </div>
                <div class="ctx-files-add">
                  <input
                    class="ctx-files-input"
                    type="text"
                    placeholder="Relative path (e.g. ../../_agent_foo/Role.md)"
                    value={newCtxPath()}
                    onInput={(e) => setNewCtxPath(e.currentTarget.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") handleAddCtxFile();
                    }}
                  />
                  <button
                    class="ctx-files-add-btn"
                    onClick={handleAddCtxFile}
                    disabled={!newCtxPath().trim()}
                  >
                    Add
                  </button>
                </div>
              </Show>
            </div>
          </div>
        </Portal>
      )}
    </Show>
  );
};

export default AcDiscoveryPanel;
