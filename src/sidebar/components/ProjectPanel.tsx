import { Component, For, Show, createSignal, onMount, onCleanup } from "solid-js";
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

        const hasTeamsWithCoord = () =>
          proj.teams.some((t) => t.coordinator !== null && t.coordinator !== "");

        const handleRemoveProject = () => {
          setShowCtxMenu(false);
          projectStore.removeProject(proj.path);
        };

        const handleTeamContextMenu = (e: MouseEvent, team: AcTeam) => {
          e.preventDefault();
          e.stopPropagation();
          cleanupCtx();
          setShowCtxMenu(false);
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
                    classList={{ "context-option-disabled": !hasTeamsWithCoord() }}
                    disabled={!hasTeamsWithCoord()}
                    onClick={() => {
                      if (!hasTeamsWithCoord()) return;
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
                {/* Agent Matrix */}
                {(() => {
                  const [matrixCollapsed, setMatrixCollapsed] = createSignal(false);
                  return (
                    <div class="ac-wg-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        onClick={() => setMatrixCollapsed((c) => !c)}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: matrixCollapsed() }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Agent Matrix</span>
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
                    class="session-context-option session-context-option--danger"
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
                        Delete team <strong>{deletingTeam()!.name}</strong>? This will remove the team configuration. This action cannot be undone.
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
