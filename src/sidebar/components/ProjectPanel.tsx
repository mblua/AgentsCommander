import { Component, For, Show, createMemo, createSignal, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcWorkgroup, AcAgentReplica, AcTeam, Session } from "../../shared/types";
import { SessionAPI, WindowAPI, EntityAPI, onDiscoveryBranchUpdated } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import SessionItem from "./SessionItem";
import NewEntityAgentModal from "./NewEntityAgentModal";
import NewTeamModal from "./NewTeamModal";
import NewWorkgroupModal from "./NewWorkgroupModal";
import AgentPickerModal from "./AgentPickerModal";
import EditTeamModal from "./EditTeamModal";

interface PendingLaunch {
  path: string;
  sessionName: string;
  gitBranchSource?: string;
  gitBranchPrefix?: string;
}

/** Derive the repo name from a replica's repoPaths (strip 'repo-' prefix) */
function replicaRepoName(replica: AcAgentReplica): string | undefined {
  if (!replica.repoPaths?.length) return undefined;
  const dirName = replica.repoPaths[0].replace(/\\/g, "/").split("/").pop() ?? "";
  return dirName.startsWith("repo-") ? dirName.slice(5) : dirName;
}

/** Build the session name used to link a replica to its session */
function replicaSessionName(wg: AcWorkgroup, replica: AcAgentReplica): string {
  return `${wg.name}/${replica.name}`;
}

/** Find existing session for a replica, if any */
function replicaSession(wg: AcWorkgroup, replica: AcAgentReplica): Session | undefined {
  return sessionsStore.findSessionByName(replicaSessionName(wg, replica));
}

/** Check if a replica is the coordinator of its workgroup's team */
function isReplicaCoordinator(replica: AcAgentReplica, projectFolder: string, teams: AcTeam[], teamName?: string): boolean {
  const project = replica.originProject || projectFolder;
  const fullRef = `${project}/${replica.name}`;
  if (!teamName) return false;
  const team = teams.find((t) => t.name === teamName);
  return team ? team.coordinator === fullRef : false;
}

/** Compute CSS class for replica status dot */
function replicaDotClass(wg: AcWorkgroup, replica: AcAgentReplica): string {
  const session = replicaSession(wg, replica);
  if (!session) return "offline";
  if (session.pendingReview) return "pending";
  if (session.waitingForInput) return "waiting";
  if (typeof session.status === "string") return session.status;
  return "exited";
}

/** Check if a session has a live PTY process (not exited, not offline) */
function isSessionLive(session: Session | undefined): boolean {
  if (!session) return false;
  if (typeof session.status === "object" && "exited" in session.status) return false;
  return true;
}

/** Get replicas in a workgroup that have active (live) sessions */
function getActiveReplicasForWg(wg: AcWorkgroup): AcAgentReplica[] {
  return (wg.agents ?? []).filter(replica => isSessionLive(replicaSession(wg, replica)));
}

const ProjectPanel: Component = () => {
  // Listen for replica branch updates from the discovery branch watcher
  let unlistenBranch: (() => void) | null = null;
  onMount(async () => {
    unlistenBranch = await onDiscoveryBranchUpdated((data) => {
      projectStore.updateReplicaBranch(data.replicaPath, data.branch);
    });
  });
  onCleanup(() => unlistenBranch?.());

  const [pendingLaunch, setPendingLaunch] = createSignal<PendingLaunch | null>(null);

  const handleReplicaClick = async (replica: AcAgentReplica, wg: AcWorkgroup) => {
    const existing = replicaSession(wg, replica);
    if (existing) {
      // Already instantiated — just switch to it
      await SessionAPI.switch(existing.id);
      if (isTauri) {
        const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
        const detachedLabel = `terminal-${existing.id.replace(/-/g, "")}`;
        const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
        if (!detachedWin) {
          await WindowAPI.ensureTerminal();
        }
      }
      return;
    }

    // Not instantiated — create session in-place
    const repoPaths = replica.repoPaths ?? [];
    let gitBranchSource: string | undefined;
    let gitBranchPrefix: string | undefined;

    if (repoPaths.length === 1) {
      gitBranchSource = repoPaths[0];
      const dirName = repoPaths[0].replace(/\\/g, "/").split("/").pop() ?? "";
      gitBranchPrefix = dirName.startsWith("repo-") ? dirName.slice(5) : dirName;
    } else if (repoPaths.length > 1) {
      gitBranchPrefix = "multi-repo";
    }

    if (!replica.preferredAgentId) {
      setPendingLaunch({
        path: replica.path,
        sessionName: replicaSessionName(wg, replica),
        gitBranchSource,
        gitBranchPrefix,
      });
      return;
    }

    const newSession = await SessionAPI.create({
      cwd: replica.path,
      sessionName: replicaSessionName(wg, replica),
      agentId: replica.preferredAgentId,
      gitBranchSource,
      gitBranchPrefix,
    });
    await SessionAPI.switch(newSession.id);
    if (isTauri) {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const detachedLabel = `terminal-${newSession.id.replace(/-/g, "")}`;
      const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
      if (!detachedWin) {
        await WindowAPI.ensureTerminal();
      }
    }
  };

  const handleAgentClick = async (agent: { name: string; path: string; preferredAgentId?: string }) => {
    const existing = sessionsStore.findSessionByName(agent.name);
    if (existing) {
      await SessionAPI.switch(existing.id);
      if (isTauri) {
        const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
        const detachedLabel = `terminal-${existing.id.replace(/-/g, "")}`;
        const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
        if (!detachedWin) {
          await WindowAPI.ensureTerminal();
        }
      }
      return;
    }

    if (!agent.preferredAgentId) {
      setPendingLaunch({ path: agent.path, sessionName: agent.name });
      return;
    }

    const newSession = await SessionAPI.create({
      cwd: agent.path,
      sessionName: agent.name,
      agentId: agent.preferredAgentId,
    });
    await SessionAPI.switch(newSession.id);
    if (isTauri) {
      const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
      const detachedLabel = `terminal-${newSession.id.replace(/-/g, "")}`;
      const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
      if (!detachedWin) {
        await WindowAPI.ensureTerminal();
      }
    }
  };

  return (
    <>
    <For each={projectStore.projects}>
      {(proj) => {
        const [collapsed, setCollapsed] = createSignal(false);
        const [showCtxMenu, setShowCtxMenu] = createSignal(false);
        const [ctxMenuPos, setCtxMenuPos] = createSignal({ x: 0, y: 0 });
        const [showNewAgent, setShowNewAgent] = createSignal(false);
        const [showNewTeam, setShowNewTeam] = createSignal(false);
        const [showNewWorkgroup, setShowNewWorkgroup] = createSignal(false);
        const [teamCtxMenu, setTeamCtxMenu] = createSignal<{ team: AcTeam; x: number; y: number } | null>(null);
        const [editingTeam, setEditingTeam] = createSignal<AcTeam | null>(null);
        const [deletingTeam, setDeletingTeam] = createSignal<AcTeam | null>(null);
        const [deleteError, setDeleteError] = createSignal("");
        const [deleteInProgress, setDeleteInProgress] = createSignal(false);
        const [wgCtxMenu, setWgCtxMenu] = createSignal<{ wg: AcWorkgroup; x: number; y: number } | null>(null);
        const [deletingWg, setDeletingWg] = createSignal<AcWorkgroup | null>(null);
        const [wgDeleteError, setWgDeleteError] = createSignal("");
        const [wgDeleteInProgress, setWgDeleteInProgress] = createSignal(false);
        const [wgDirtyRepos, setWgDirtyRepos] = createSignal(false);
        const [wgConfirmText, setWgConfirmText] = createSignal("");
        const [agentCtxMenu, setAgentCtxMenu] = createSignal<{ agent: { name: string; path: string; preferredAgentId?: string }; x: number; y: number } | null>(null);
        const [agentsHeaderCtxMenu, setAgentsHeaderCtxMenu] = createSignal<{ x: number; y: number } | null>(null);
        const [deletingAgent, setDeletingAgent] = createSignal<{ name: string; path: string } | null>(null);
        const [agentDeleteError, setAgentDeleteError] = createSignal("");
        const [agentDeleteInProgress, setAgentDeleteInProgress] = createSignal(false);
        const closeAgentDeleteModal = () => {
          setAgentDeleteError("");
          setAgentDeleteInProgress(false);
          setDeletingAgent(null);
        };
        const closeWgDeleteModal = () => {
          setWgDeleteError("");
          setWgDirtyRepos(false);
          setWgConfirmText("");
          setWgDeleteInProgress(false);
          setDeletingWg(null);
        };
        const activeReplicas = createMemo(() => {
          const wg = deletingWg();
          return wg ? getActiveReplicasForWg(wg) : [];
        });

        let dismissCtx: (() => void) | null = null;

        const cleanupCtx = () => {
          if (dismissCtx) {
            window.removeEventListener("click", dismissCtx);
            window.removeEventListener("contextmenu", dismissCtx);
            window.removeEventListener("keydown", dismissCtx as any);
            dismissCtx = null;
          }
        };

        onCleanup(cleanupCtx);

        const handleProjectContextMenu = (e: MouseEvent) => {
          e.preventDefault();
          e.stopPropagation();
          cleanupCtx();
          setTeamCtxMenu(null);
          setWgCtxMenu(null);
          setAgentCtxMenu(null);
          setAgentsHeaderCtxMenu(null);
          setCtxMenuPos({ x: e.clientX, y: e.clientY });
          setShowCtxMenu(true);
          const dismiss = (ev?: Event) => {
            if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
            setShowCtxMenu(false);
            cleanupCtx();
          };
          dismissCtx = dismiss;
          setTimeout(() => {
            window.addEventListener("click", dismiss);
            window.addEventListener("contextmenu", dismiss);
            window.addEventListener("keydown", dismiss as any);
          });
        };

        const hasTeams = () => proj.teams.length > 0;

        const handleRemoveProject = () => {
          setShowCtxMenu(false);
          projectStore.removeProject(proj.path);
        };

        const handleTeamContextMenu = (e: MouseEvent, team: AcTeam) => {
          e.preventDefault();
          e.stopPropagation();
          cleanupCtx();
          setShowCtxMenu(false);
          setWgCtxMenu(null);
          setAgentCtxMenu(null);
          setAgentsHeaderCtxMenu(null);
          setTeamCtxMenu({ team, x: e.clientX, y: e.clientY });
          const dismiss = (ev?: Event) => {
            if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
            setTeamCtxMenu(null);
            cleanupCtx();
          };
          dismissCtx = dismiss;
          setTimeout(() => {
            window.addEventListener("click", dismiss);
            window.addEventListener("contextmenu", dismiss);
            window.addEventListener("keydown", dismiss as any);
          });
        };

        const handleWgContextMenu = (e: MouseEvent, wg: AcWorkgroup) => {
          e.preventDefault();
          e.stopPropagation();
          cleanupCtx();
          setShowCtxMenu(false);
          setTeamCtxMenu(null);
          setAgentCtxMenu(null);
          setAgentsHeaderCtxMenu(null);
          setWgCtxMenu({ wg, x: e.clientX, y: e.clientY });
          const dismiss = (ev?: Event) => {
            if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
            setWgCtxMenu(null);
            cleanupCtx();
          };
          dismissCtx = dismiss;
          setTimeout(() => {
            window.addEventListener("click", dismiss);
            window.addEventListener("contextmenu", dismiss);
            window.addEventListener("keydown", dismiss as any);
          });
        };

        return (
          <div class="project-panel">
            <button
              class="project-header"
              onClick={() => setCollapsed((c) => !c)}
              onContextMenu={handleProjectContextMenu}
            >
              <span class="ac-discovery-chevron" classList={{ collapsed: collapsed() }}>
                &#x25BE;
              </span>
              <span class="project-title">Project: {proj.folderName}</span>
            </button>

            {/* Project context menu */}
            {showCtxMenu() && (
              <Portal>
                <div
                  class="session-context-menu"
                  style={{ left: `${ctxMenuPos().x}px`, top: `${ctxMenuPos().y}px` }}
                  onClick={(e) => e.stopPropagation()}
                >
                  <button
                    class="session-context-option"
                    onClick={() => { setShowCtxMenu(false); setShowNewAgent(true); }}
                  >
                    New Agent
                  </button>
                  <button
                    class="session-context-option"
                    onClick={() => { setShowCtxMenu(false); setShowNewTeam(true); }}
                  >
                    New Team
                  </button>
                  <button
                    class="session-context-option"
                    classList={{ "context-option-disabled": !hasTeams() }}
                    disabled={!hasTeams()}
                    onClick={() => {
                      if (!hasTeams()) return;
                      setShowCtxMenu(false);
                      setShowNewWorkgroup(true);
                    }}
                  >
                    New Workgroup
                  </button>
                  <div class="context-separator" />
                  <button
                    class="session-context-option context-option-danger"
                    onClick={handleRemoveProject}
                  >
                    Remove Project
                  </button>
                </div>
              </Portal>
            )}

            {/* Entity creation modals */}
            {showNewAgent() && (
              <Portal>
                <NewEntityAgentModal
                  projectPath={proj.path}
                  onClose={() => setShowNewAgent(false)}
                />
              </Portal>
            )}
            {showNewTeam() && (
              <Portal>
                <NewTeamModal
                  projectPath={proj.path}
                  onClose={() => setShowNewTeam(false)}
                />
              </Portal>
            )}
            {showNewWorkgroup() && (
              <Portal>
                <NewWorkgroupModal
                  projectPath={proj.path}
                  teams={proj.teams}
                  onClose={() => setShowNewWorkgroup(false)}
                />
              </Portal>
            )}

            <Show when={!collapsed()}>
              <div class="project-content">
                {/* Coordinator Quick-Access — shown by styles that enable it via CSS */}
                {(() => {
                  const coordinators = createMemo(() => {
                    const result: { replica: AcAgentReplica; wg: AcWorkgroup; dotClass: string }[] = [];
                    for (const wg of proj.workgroups) {
                      for (const replica of wg.agents) {
                        if (isReplicaCoordinator(replica, proj.folderName, proj.teams, wg.teamName)) {
                          result.push({ replica, wg, dotClass: replicaDotClass(wg, replica) });
                        }
                      }
                    }
                    return result;
                  });
                  return (
                    <Show when={coordinators().length > 0}>
                      <div class="coord-quick-access">
                        <For each={coordinators()}>
                          {(item) => {
                            const session = () => replicaSession(item.wg, item.replica);
                            return (
                              <Show
                                when={session()}
                                fallback={
                                  <div
                                    class="coord-quick-item"
                                    onClick={() => handleReplicaClick(item.replica, item.wg)}
                                    title={item.replica.path}
                                  >
                                    <div class={`session-item-status ${item.dotClass}`} />
                                    <div class="coord-quick-info">
                                      <span class="coord-quick-name">{item.replica.name}</span>
                                      <span class="coord-quick-wg">{item.wg.name}</span>
                                    </div>
                                  </div>
                                }
                              >
                                {(s) => {
                                  const patched = () => {
                                    const sess = s();
                                    const repoName = replicaRepoName(item.replica);
                                    if (sess.gitBranch && !sess.gitBranch.includes("/") && repoName) {
                                      return { ...sess, gitBranch: `${repoName}/${sess.gitBranch}` };
                                    }
                                    return sess;
                                  };
                                  return (
                                    <div class="coord-quick-item coord-quick-item--session">
                                      <SessionItem
                                        session={patched()}
                                        isActive={s().id === sessionsStore.activeId}
                                      />
                                    </div>
                                  );
                                }}
                              </Show>
                            );
                          }}
                        </For>
                      </div>
                    </Show>
                  );
                })()}
                {/* Workgroups */}
                {(() => {
                  const [wgsCollapsed, setWgsCollapsed] = createSignal(false);
                  return (
                    <div class="ac-wg-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        onClick={() => setWgsCollapsed((c) => !c)}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: wgsCollapsed() }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Workgroups</span>
                        </div>
                        <span class="ac-team-count">{proj.workgroups.length}</span>
                      </div>
                      <Show when={!wgsCollapsed()}>
                        <Show
                          when={proj.workgroups.length > 0}
                          fallback={
                            <div class="ac-empty-hint">No workgroups</div>
                          }
                        >
                          <For each={proj.workgroups}>
                            {(wg) => {
                              const [wgCollapsed, setWgCollapsed] = createSignal(false);
                              return (
                                <div class="ac-wg-subgroup">
                                  <div
                                    class="ac-wg-header ac-wg-header--collapsible"
                                    title={wg.path}
                                    onClick={() => setWgCollapsed((c) => !c)}
                                    onContextMenu={(e) => handleWgContextMenu(e, wg)}
                                  >
                                    <span class="ac-discovery-chevron" classList={{ collapsed: wgCollapsed() }}>
                                      &#x25BE;
                                    </span>
                                    <div class="ac-wg-header-text">
                                      <span class="ac-wg-name">{wg.name}</span>
                                      <Show when={wg.brief}>
                                        <span class="ac-wg-brief">{wg.brief}</span>
                                      </Show>
                                    </div>
                                  </div>
                                  <Show when={!wgCollapsed()}>
                                    <For each={wg.agents}>
                                      {(replica) => {
                                        const session = () => replicaSession(wg, replica);
                                        return (
                                          <Show
                                            when={session()}
                                            fallback={
                                              (() => {
                                                const dotClass = () => replicaDotClass(wg, replica);
                                                const repoName = () => replicaRepoName(replica);
                                                const branchLabel = () => {
                                                  const name = repoName();
                                                  if (!name) return null;
                                                  return replica.repoBranch ? `${name}/${replica.repoBranch}` : name;
                                                };
                                                const isCoord = () => isReplicaCoordinator(replica, proj.folderName, proj.teams, wg.teamName);
                                                return (
                                                  <div
                                                    class="ac-discovery-item"
                                                    onClick={() => handleReplicaClick(replica, wg)}
                                                    title={replica.path}
                                                  >
                                                    <div class={`session-item-status ${dotClass()}`} />
                                                    <div class="ac-discovery-item-info">
                                                      <span class="ac-discovery-item-name">{replica.originProject ? `${replica.name}@${replica.originProject}` : replica.name}</span>
                                                      <div class="ac-discovery-badges">
                                                        <Show when={isCoord()}>
                                                          <span class="ac-discovery-badge coord">coordinator</span>
                                                        </Show>
                                                        <Show when={branchLabel()}>
                                                          <span class="ac-discovery-badge branch">{branchLabel()}</span>
                                                        </Show>
                                                      </div>
                                                    </div>
                                                  </div>
                                                );
                                              })()
                                            }
                                          >
                                            {(s) => {
                                              const patched = () => {
                                                const sess = s();
                                                if (sess.gitBranch && !sess.gitBranch.includes("/")) {
                                                  const repoName = replicaRepoName(replica);
                                                  if (repoName) {
                                                    return { ...sess, gitBranch: `${repoName}/${sess.gitBranch}` };
                                                  }
                                                }
                                                return sess;
                                              };
                                              return (
                                                <SessionItem
                                                  session={patched()}
                                                  isActive={s().id === sessionsStore.activeId}
                                                  originProject={replica.originProject}
                                                />
                                              );
                                            }}
                                          </Show>
                                        );
                                      }}
                                    </For>
                                  </Show>
                                </div>
                              );
                            }}
                          </For>
                        </Show>
                      </Show>
                    </div>
                  );
                })()}
                {/* Agents */}
                {(() => {
                  const [matrixCollapsed, setMatrixCollapsed] = createSignal(false);

                  const handleAgentContextMenu = (e: MouseEvent, agent: { name: string; path: string; preferredAgentId?: string }) => {
                    e.preventDefault();
                    e.stopPropagation();
                    cleanupCtx();
                    setShowCtxMenu(false);
                    setTeamCtxMenu(null);
                    setWgCtxMenu(null);
                    setAgentsHeaderCtxMenu(null);
                    setAgentCtxMenu({ agent, x: e.clientX, y: e.clientY });
                    const dismiss = (ev?: Event) => {
                      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
                      setAgentCtxMenu(null);
                      cleanupCtx();
                    };
                    dismissCtx = dismiss;
                    setTimeout(() => {
                      window.addEventListener("click", dismiss);
                      window.addEventListener("contextmenu", dismiss);
                      window.addEventListener("keydown", dismiss as any);
                    });
                  };

                  const handleAgentsHeaderContextMenu = (e: MouseEvent) => {
                    e.preventDefault();
                    e.stopPropagation();
                    cleanupCtx();
                    setShowCtxMenu(false);
                    setTeamCtxMenu(null);
                    setWgCtxMenu(null);
                    setAgentCtxMenu(null);
                    setAgentsHeaderCtxMenu({ x: e.clientX, y: e.clientY });
                    const dismiss = (ev?: Event) => {
                      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
                      setAgentsHeaderCtxMenu(null);
                      cleanupCtx();
                    };
                    dismissCtx = dismiss;
                    setTimeout(() => {
                      window.addEventListener("click", dismiss);
                      window.addEventListener("contextmenu", dismiss);
                      window.addEventListener("keydown", dismiss as any);
                    });
                  };

                  return (
                    <>
                    <div class="ac-wg-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        onClick={() => setMatrixCollapsed((c) => !c)}
                        onContextMenu={handleAgentsHeaderContextMenu}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: matrixCollapsed() }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Agents</span>
                        </div>
                      </div>
                      <Show when={!matrixCollapsed()}>
                        <Show
                          when={proj.agents.length > 0}
                          fallback={
                            <div class="ac-empty-hint">No agents</div>
                          }
                        >
                          <For each={proj.agents}>
                            {(agent) => {
                              const session = () => sessionsStore.findSessionByName(agent.name);
                              return (
                                <Show
                                  when={session()}
                                  fallback={
                                    <div
                                      class="ac-discovery-item"
                                      onClick={() => handleAgentClick(agent)}
                                      onContextMenu={(e) => handleAgentContextMenu(e, agent)}
                                      title={agent.path}
                                    >
                                      <div class="session-item-status offline" />
                                      <div class="ac-discovery-item-info">
                                        <span class="ac-discovery-item-name">
                                          {agent.name.slice(agent.name.lastIndexOf("/") + 1)}
                                        </span>
                                      </div>
                                    </div>
                                  }
                                >
                                  {(s) => (
                                    <SessionItem
                                      session={s()}
                                      isActive={s().id === sessionsStore.activeId}
                                    />
                                  )}
                                </Show>
                              );
                            }}
                          </For>
                        </Show>
                      </Show>
                    </div>

                    {/* Agent item context menu */}
                    {agentCtxMenu() && (
                      <Portal>
                        <div
                          class="session-context-menu"
                          style={{ left: `${agentCtxMenu()!.x}px`, top: `${agentCtxMenu()!.y}px` }}
                          onClick={(e) => e.stopPropagation()}
                        >
                          <button
                            class="session-context-option"
                            onClick={() => {
                              const menu = agentCtxMenu();
                              setAgentCtxMenu(null);
                              if (menu) WindowAPI.openInExplorer(menu.agent.path);
                            }}
                          >
                            Open in Explorer
                          </button>
                          <button
                            class="session-context-option context-option-danger"
                            onClick={() => {
                              const menu = agentCtxMenu();
                              if (menu) setDeletingAgent({ name: menu.agent.name, path: menu.agent.path });
                              setAgentCtxMenu(null);
                            }}
                          >
                            Delete Agent
                          </button>
                        </div>
                      </Portal>
                    )}

                    {/* Agents header context menu */}
                    {agentsHeaderCtxMenu() && (
                      <Portal>
                        <div
                          class="session-context-menu"
                          style={{ left: `${agentsHeaderCtxMenu()!.x}px`, top: `${agentsHeaderCtxMenu()!.y}px` }}
                          onClick={(e) => e.stopPropagation()}
                        >
                          <button
                            class="session-context-option"
                            onClick={() => {
                              setAgentsHeaderCtxMenu(null);
                              setShowNewAgent(true);
                            }}
                          >
                            New Agent
                          </button>
                        </div>
                      </Portal>
                    )}

                    {/* Delete agent confirmation */}
                    {deletingAgent() && (
                      <Portal>
                        <div
                          class="modal-overlay"
                          onClick={(e) => {
                            if ((e.target as HTMLElement).classList.contains("modal-overlay")) closeAgentDeleteModal();
                          }}
                          onKeyDown={(e) => {
                            if (e.key === "Escape") closeAgentDeleteModal();
                          }}
                        >
                          <div class="agent-modal" style={{ "max-width": "360px" }}>
                            <div class="agent-modal-header">
                              <span class="agent-modal-title">Delete Agent</span>
                            </div>
                            <div class="new-agent-form">
                              <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
                                Delete agent <strong>{deletingAgent()!.name.slice(deletingAgent()!.name.lastIndexOf("/") + 1)}</strong>? This will remove the agent directory and all its contents. This action cannot be undone.
                              </p>
                              <Show when={agentDeleteError()}>
                                <div class="new-agent-error">{agentDeleteError()}</div>
                              </Show>
                            </div>
                            <div class="new-agent-footer">
                              <button class="new-agent-cancel-btn" onClick={closeAgentDeleteModal}>
                                Cancel
                              </button>
                              <button
                                class="new-agent-create-btn"
                                style={{ "background": "var(--danger, #c0392b)" }}
                                disabled={agentDeleteInProgress()}
                                onClick={async () => {
                                  if (agentDeleteInProgress()) return;
                                  setAgentDeleteInProgress(true);
                                  const agent = deletingAgent()!;
                                  const shortName = agent.name.slice(agent.name.lastIndexOf("/") + 1);
                                  try {
                                    await EntityAPI.deleteAgentMatrix(proj.path, shortName);
                                    await projectStore.reloadProject(proj.path);
                                  } catch (e: any) {
                                    console.error("delete_agent_matrix failed:", e);
                                    setAgentDeleteError(typeof e === "string" ? e : e?.message ?? "Failed to delete agent");
                                    setAgentDeleteInProgress(false);
                                    return;
                                  }
                                  closeAgentDeleteModal();
                                }}
                              >
                                {agentDeleteInProgress() ? "Deleting..." : "Delete"}
                              </button>
                            </div>
                          </div>
                        </div>
                      </Portal>
                    )}
                    </>
                  );
                })()}
                {/* Teams */}
                {(() => {
                  const [teamsCollapsed, setTeamsCollapsed] = createSignal(false);
                  return (
                    <div class="ac-wg-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        onClick={() => setTeamsCollapsed((c) => !c)}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: teamsCollapsed() }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Teams</span>
                        </div>
                      </div>
                      <Show when={!teamsCollapsed()}>
                        <Show
                          when={proj.teams.length > 0}
                          fallback={
                            <div class="ac-empty-hint">No teams</div>
                          }
                        >
                          <For each={proj.teams}>
                            {(team) => {
                              const [teamExpanded, setTeamExpanded] = createSignal(false);
                              return (
                                <div class="ac-team-group">
                                  <div
                                    class="ac-team-header"
                                    onClick={() => setTeamExpanded((e) => !e)}
                                    onContextMenu={(e) => handleTeamContextMenu(e, team)}
                                  >
                                    <span class="ac-discovery-chevron" classList={{ collapsed: !teamExpanded() }}>
                                      &#x25BE;
                                    </span>
                                    <span class="ac-team-name">{team.name}</span>
                                    <span class="ac-team-count">{team.agents.length}</span>
                                  </div>
                                  <Show when={teamExpanded()}>
                                    <div class="ac-team-members">
                                      <For each={team.agents}>
                                        {(agentName) => {
                                          const shortName = () => {
                                            const parts = agentName.replace(/\\/g, "/").split("/");
                                            const agent = parts[parts.length - 1].replace(/^__?agent_/, "");
                                            const project = parts[0];
                                            return project && project !== agent ? `${agent}@${project}` : agent;
                                          };
                                          return (
                                            <div class="ac-team-member" title={agentName}>
                                              <span class="ac-team-member-name">{shortName()}</span>
                                              <Show when={agentName === team.coordinator}>
                                                <span class="ac-discovery-badge coord">coordinator</span>
                                              </Show>
                                            </div>
                                          );
                                        }}
                                      </For>
                                    </div>
                                  </Show>
                                </div>
                              );
                            }}
                          </For>
                        </Show>
                      </Show>
                    </div>
                  );
                })()}
              </div>
            </Show>

            {/* Team context menu */}
            {teamCtxMenu() && (
              <Portal>
                <div
                  class="session-context-menu"
                  style={{ left: `${teamCtxMenu()!.x}px`, top: `${teamCtxMenu()!.y}px` }}
                  onClick={(e) => e.stopPropagation()}
                >
                  <button
                    class="session-context-option"
                    onClick={() => {
                      const menu = teamCtxMenu();
                      if (menu) setEditingTeam(menu.team);
                      setTeamCtxMenu(null);
                    }}
                  >
                    Edit Team
                  </button>
                  <button
                    class="session-context-option context-option-danger"
                    onClick={() => {
                      const menu = teamCtxMenu();
                      if (menu) setDeletingTeam(menu.team);
                      setTeamCtxMenu(null);
                    }}
                  >
                    Delete Team
                  </button>
                </div>
              </Portal>
            )}

            {/* WG context menu */}
            {wgCtxMenu() && (
              <Portal>
                <div
                  class="session-context-menu"
                  style={{ left: `${wgCtxMenu()!.x}px`, top: `${wgCtxMenu()!.y}px` }}
                  onClick={(e) => e.stopPropagation()}
                >
                  <button
                    class="session-context-option context-option-danger"
                    onClick={() => {
                      cleanupCtx();
                      const menu = wgCtxMenu();
                      if (menu) setDeletingWg(menu.wg);
                      setWgCtxMenu(null);
                    }}
                  >
                    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" style={{ "flex-shrink": "0" }}>
                      <polyline points="3 6 5 6 21 6" />
                      <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                    </svg>
                    Delete Workgroup
                  </button>
                </div>
              </Portal>
            )}

            {/* Delete WG confirmation */}
            {deletingWg() && (
              <Portal>
                <div
                  class="modal-overlay"
                  onClick={(e) => {
                    if ((e.target as HTMLElement).classList.contains("modal-overlay")) closeWgDeleteModal();
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") closeWgDeleteModal();
                  }}
                >
                  <div class="agent-modal" style={{ "max-width": "360px" }}>
                    <div class="agent-modal-header">
                      <span class="agent-modal-title">Delete Workgroup</span>
                    </div>
                    <div class="new-agent-form">
                      <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
                        Delete workgroup <strong>{deletingWg()!.name}</strong>? This will remove the workgroup directory and all its contents. This action cannot be undone.
                      </p>
                      <Show when={activeReplicas().length > 0}>
                        <div style={{
                          "background": "var(--danger, #c0392b)",
                          "color": "#fff",
                          "padding": "10px 12px",
                          "border-radius": "6px",
                          "margin-top": "10px",
                          "font-size": "12px",
                          "line-height": "1.5",
                        }}>
                          <strong>Cannot delete:</strong> the following sessions are still active:
                          <ul style={{ margin: "6px 0 6px 16px", padding: "0" }}>
                            <For each={activeReplicas()}>
                              {(replica) => <li>{replica.name}</li>}
                            </For>
                          </ul>
                          Close all active sessions first by hovering over each session and clicking the <strong>✕</strong> button.
                        </div>
                      </Show>
                      <Show when={wgDeleteError()}>
                        <div class="new-agent-error">{wgDeleteError()}</div>
                      </Show>
                      <Show when={wgDirtyRepos()}>
                        <div style={{ "margin-top": "10px" }}>
                          <label style={{ "font-size": "12px", opacity: 0.8, display: "block", "margin-bottom": "6px" }}>
                            To delete anyway, type <strong>{deletingWg()!.name}</strong> below:
                          </label>
                          <input
                            type="text"
                            class="new-agent-input"
                            placeholder={deletingWg()!.name}
                            value={wgConfirmText()}
                            onInput={(e) => setWgConfirmText(e.currentTarget.value)}
                            spellcheck={false}
                            autocomplete="off"
                          />
                        </div>
                      </Show>
                    </div>
                    <div class="new-agent-footer">
                      <button class="new-agent-cancel-btn" onClick={closeWgDeleteModal}>
                        Cancel
                      </button>
                      <button
                        class="new-agent-create-btn"
                        style={{ "background": "var(--danger, #c0392b)" }}
                        disabled={wgDeleteInProgress() || activeReplicas().length > 0 || (wgDirtyRepos() && wgConfirmText() !== deletingWg()!.name)}
                        onClick={async () => {
                          if (wgDeleteInProgress()) return;
                          if (activeReplicas().length > 0) return;
                          setWgDeleteInProgress(true);
                          const wg = deletingWg()!;
                          const forceDelete = wgDirtyRepos();
                          try {
                            await EntityAPI.deleteWorkgroup(proj.path, wg.name, forceDelete);
                            await projectStore.reloadProject(proj.path);
                          } catch (e: any) {
                            console.error("delete_workgroup failed:", e);
                            const msg = typeof e === "string" ? e : e?.message ?? "Failed to delete workgroup";
                            // DIRTY_REPOS: sentinel prefix — switch to force-confirm mode
                            if (!forceDelete && msg.startsWith("DIRTY_REPOS:")) {
                              setWgDeleteError(msg.slice("DIRTY_REPOS:".length));
                              setWgDirtyRepos(true);
                              setWgConfirmText("");
                              setWgDeleteInProgress(false);
                              return;
                            }
                            setWgDeleteError(msg);
                            setWgDeleteInProgress(false);
                            return;
                          }
                          closeWgDeleteModal();
                        }}
                      >
                        {wgDeleteInProgress() ? "Deleting..." : "Delete"}
                      </button>
                    </div>
                  </div>
                </div>
              </Portal>
            )}

            {/* Edit team modal */}
            {editingTeam() && (
              <Portal>
                <EditTeamModal
                  projectPath={proj.path}
                  team={editingTeam()!}
                  onClose={() => setEditingTeam(null)}
                />
              </Portal>
            )}

            {/* Delete team confirmation */}
            {deletingTeam() && (
              <Portal>
                <div
                  class="modal-overlay"
                  onClick={(e) => {
                    if ((e.target as HTMLElement).classList.contains("modal-overlay")) {
                      setDeleteError("");
                      setDeletingTeam(null);
                    }
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") {
                      setDeleteError("");
                      setDeletingTeam(null);
                    }
                  }}
                >
                  <div class="agent-modal" style={{ "max-width": "360px" }}>
                    <div class="agent-modal-header">
                      <span class="agent-modal-title">Delete Team</span>
                    </div>
                    <div class="new-agent-form">
                      <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
                        Delete team <strong>{deletingTeam()!.name}</strong>? This will remove the team configuration and all associated workgroups. This action cannot be undone.
                      </p>
                      <Show when={deleteError()}>
                        <div class="new-agent-error">{deleteError()}</div>
                      </Show>
                    </div>
                    <div class="new-agent-footer">
                      <button
                        class="new-agent-cancel-btn"
                        onClick={() => {
                          setDeleteError("");
                          setDeletingTeam(null);
                        }}
                      >
                        Cancel
                      </button>
                      <button
                        class="new-agent-create-btn"
                        style={{ "background": "var(--danger, #c0392b)" }}
                        disabled={deleteInProgress()}
                        onClick={async () => {
                          if (deleteInProgress()) return;
                          setDeleteInProgress(true);
                          const team = deletingTeam()!;
                          try {
                            await EntityAPI.deleteTeam(proj.path, team.name);
                            await projectStore.reloadProject(proj.path);
                          } catch (e: any) {
                            console.error("delete_team failed:", e);
                            setDeleteError(typeof e === "string" ? e : e?.message ?? "Failed to delete team");
                            setDeleteInProgress(false);
                            return;
                          }
                          setDeleteError("");
                          setDeleteInProgress(false);
                          setDeletingTeam(null);
                        }}
                      >
                        {deleteInProgress() ? "Deleting..." : "Delete"}
                      </button>
                    </div>
                  </div>
                </div>
              </Portal>
            )}
          </div>
        );
      }}
    </For>

    {/* Agent picker for agents/replicas without a preferredAgentId */}
    {pendingLaunch() && (
      <Portal>
        <AgentPickerModal
          sessionName={pendingLaunch()!.sessionName}
          onSelect={async (agent) => {
            const pending = pendingLaunch()!;
            const newSession = await SessionAPI.create({
              cwd: pending.path,
              sessionName: pending.sessionName,
              agentId: agent.id,
              gitBranchSource: pending.gitBranchSource,
              gitBranchPrefix: pending.gitBranchPrefix,
            });
            await SessionAPI.switch(newSession.id);
            if (isTauri) {
              const { WebviewWindow } = await import("@tauri-apps/api/webviewWindow");
              const detachedLabel = `terminal-${newSession.id.replace(/-/g, "")}`;
              const detachedWin = await WebviewWindow.getByLabel(detachedLabel);
              if (!detachedWin) {
                await WindowAPI.ensureTerminal();
              }
            }
            setPendingLaunch(null);
          }}
          onClose={() => setPendingLaunch(null)}
        />
      </Portal>
    )}
    </>
  );
};

export default ProjectPanel;
