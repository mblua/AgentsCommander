import { Component, For, Show, createEffect, createMemo, createSignal, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcWorkgroup, AcAgentReplica, AcTeam, AcLoopSummary, Session, TelegramBotConfig, BlockerReport } from "../../shared/types";
import { SessionAPI, WindowAPI, EntityAPI, LoopAPI, TelegramAPI, SettingsAPI, onDiscoveryBranchUpdated, emitOpenSettings } from "../../shared/ipc";
import type { SessionRepoInput } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import { stripFrontmatter } from "../../shared/markdown";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { bridgesStore } from "../stores/bridges";
import { settingsStore } from "../../shared/stores/settings";
import { voiceRecorder } from "../../shared/voice-recorder";
import SessionItem from "./SessionItem";
import NewEntityAgentModal from "./NewEntityAgentModal";
import NewTeamModal from "./NewTeamModal";
import NewWorkgroupModal from "./NewWorkgroupModal";
import NewLoopModal from "./NewLoopModal";
import EditLoopModal from "./EditLoopModal";
import AgentPickerModal from "./AgentPickerModal";
import EditTeamModal from "./EditTeamModal";
import { TelegramIcon } from "./TelegramIcon";
import { normalizeBlockerReport } from "./workgroup-delete-diagnostics";
import {
  automationIdPart,
  configuredReplicaRepoBadges,
  formatReplicaRepoBadgeLabel,
  repoLabelFromPath,
} from "./replica-repo-badges";

interface PendingLaunch {
  path: string;
  sessionName: string;
  gitRepos: SessionRepoInput[];
}

/** Build the gitRepos list for a replica. Order = replica.repoPaths order (invariant §3.1.2). */
function buildGitRepos(replica: AcAgentReplica): SessionRepoInput[] {
  return (replica.repoPaths ?? []).map((p) => {
    return { label: repoLabelFromPath(p), sourcePath: p };
  });
}

const CONTEXT_MENU_VIEWPORT_MARGIN = 8;

/** Build the session name used to link a replica to its session */
function replicaSessionName(wg: AcWorkgroup, replica: AcAgentReplica): string {
  return `${wg.name}/${replica.name}`;
}

/** Find existing session for a replica, if any */
function replicaSession(wg: AcWorkgroup, replica: AcAgentReplica): Session | undefined {
  return sessionsStore.findSessionByName(replicaSessionName(wg, replica));
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

function coordinatorItemKey(item: { replica: AcAgentReplica; wg: AcWorkgroup }): string {
  return `${item.wg.path}\u0000${item.replica.path}`;
}

/** Get replicas in a workgroup that have active (live) sessions */
function getActiveReplicasForWg(wg: AcWorkgroup): AcAgentReplica[] {
  return (wg.agents ?? []).filter(replica => isSessionLive(replicaSession(wg, replica)));
}

const ProjectPanel: Component = () => {
  // Listen for replica branch updates from the discovery watcher. TASK.md
  // updates are wired in sidebar/App.tsx (it owns the listener for the
  // canonical `workgroup_task_updated` event); ProjectPanel reads the
  // resulting state through projectStore.
  let unlistenBranch: (() => void) | null = null;
  onMount(async () => {
    unlistenBranch = await onDiscoveryBranchUpdated((data) => {
      projectStore.updateReplicaBranch(data.replicaPath, data.branch);
    });
  });
  onCleanup(() => {
    unlistenBranch?.();
  });

  const [pendingLaunch, setPendingLaunch] = createSignal<PendingLaunch | null>(null);

  const handleReplicaClick = async (replica: AcAgentReplica, wg: AcWorkgroup) => {
    const existing = replicaSession(wg, replica);
    if (existing) {
      if (!isSessionLive(existing)) {
        // Session exists but PTY has exited. Possible causes:
        //  - deferred at startup by the #248 policy (non-coord, or coord that was
        //    asleep at shutdown, or `restoreCoordinatorWakeState=false`)
        //  - user closed it during the prior run
        // Wake it with provider auto-resume so the prior conversation continues —
        // this is NOT a user-intent "fresh conversation" restart.
        try {
          await SessionAPI.restart(existing.id, { skipAutoResume: false });
          if (isTauri) {
            await WindowAPI.ensureTerminal();
          }
        } catch (e) {
          console.error("Failed to wake session:", e);
        }
        return;
      }
      // Already instantiated and live — just switch to it
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
    const gitRepos = buildGitRepos(replica);

    if (!replica.preferredAgentId) {
      setPendingLaunch({
        path: replica.path,
        sessionName: replicaSessionName(wg, replica),
        gitRepos,
      });
      return;
    }

    const newSession = await SessionAPI.create({
      cwd: replica.path,
      sessionName: replicaSessionName(wg, replica),
      agentId: replica.preferredAgentId,
      gitRepos,
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
      setPendingLaunch({ path: agent.path, sessionName: agent.name, gitRepos: [] });
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
        const [showNewLoop, setShowNewLoop] = createSignal(false);
        const [editingLoop, setEditingLoop] = createSignal<AcLoopSummary | null>(null);
        const [teamCtxMenu, setTeamCtxMenu] = createSignal<{ team: AcTeam; x: number; y: number } | null>(null);
        const [editingTeam, setEditingTeam] = createSignal<AcTeam | null>(null);
        const [deletingTeam, setDeletingTeam] = createSignal<AcTeam | null>(null);
        const [deleteError, setDeleteError] = createSignal("");
        const [deleteInProgress, setDeleteInProgress] = createSignal(false);
        const [wgCtxMenu, setWgCtxMenu] = createSignal<{ wg: AcWorkgroup; x: number; y: number } | null>(null);
        const [replicaCtxMenu, setReplicaCtxMenu] = createSignal<{ sessionId: string; sessionName: string; x: number; y: number } | null>(null);
        const [replicaCodingAgentTarget, setReplicaCodingAgentTarget] = createSignal<{ sessionId: string; sessionName: string } | null>(null);
        const [deletingWg, setDeletingWg] = createSignal<AcWorkgroup | null>(null);
        const [wgDeleteError, setWgDeleteError] = createSignal("");
        const [wgDeleteInProgress, setWgDeleteInProgress] = createSignal(false);
        const [wgDirtyRepos, setWgDirtyRepos] = createSignal(false);
        const [wgConfirmText, setWgConfirmText] = createSignal("");
        const [wgBlockers, setWgBlockers] = createSignal<BlockerReport | null>(null);
        const [wgRetryInProgress, setWgRetryInProgress] = createSignal(false);
        const [wgLastForceUsed, setWgLastForceUsed] = createSignal(false);
        let retryGen = 0;
        const [agentCtxMenu, setAgentCtxMenu] = createSignal<{ agent: { name: string; path: string; preferredAgentId?: string }; x: number; y: number } | null>(null);
        const [agentsHeaderCtxMenu, setAgentsHeaderCtxMenu] = createSignal<{ x: number; y: number } | null>(null);
        const [workgroupsHeaderCtxMenu, setWorkgroupsHeaderCtxMenu] = createSignal<{ x: number; y: number } | null>(null);
        const [loopCtxMenu, setLoopCtxMenu] = createSignal<{ loop: AcLoopSummary; x: number; y: number } | null>(null);
        const [loopsHeaderCtxMenu, setLoopsHeaderCtxMenu] = createSignal<{ x: number; y: number } | null>(null);
        const [loopActionInProgress, setLoopActionInProgress] = createSignal<string | null>(null);
        const [teamsHeaderCtxMenu, setTeamsHeaderCtxMenu] = createSignal<{ x: number; y: number } | null>(null);
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
          setWgBlockers(null);
          setWgRetryInProgress(false);
          setWgLastForceUsed(false);
          retryGen++;
          setDeletingWg(null);
        };
        const closeTeamDeleteModal = () => {
          setDeleteError("");
          setDeleteInProgress(false);
          setDeletingTeam(null);
        };
        createEffect(() => {
          if (!deletingAgent() && !deletingWg() && !deletingTeam()) return;
          const handleDeleteModalKeyDown = (e: KeyboardEvent) => {
            if (e.key !== "Escape") return;
            if (deletingAgent()) {
              closeAgentDeleteModal();
              return;
            }
            if (deletingWg()) {
              closeWgDeleteModal();
              return;
            }
            closeTeamDeleteModal();
          };
          document.addEventListener("keydown", handleDeleteModalKeyDown);
          onCleanup(() => document.removeEventListener("keydown", handleDeleteModalKeyDown));
        });
        const retryWgDelete = async () => {
          if (wgRetryInProgress()) return;
          const wg = deletingWg();
          if (!wg) return;
          setWgRetryInProgress(true);
          const myGen = ++retryGen;
          const force = wgLastForceUsed();
          try {
            await EntityAPI.deleteWorkgroup(proj.path, wg.name, force);
            if (myGen !== retryGen) return;
            await projectStore.reloadProject(proj.path);
            if (myGen !== retryGen) return;
            closeWgDeleteModal();
          } catch (e: any) {
            if (myGen !== retryGen) return;
            const msg = typeof e === "string" ? e : e?.message ?? "Failed to delete workgroup";
            if (msg.startsWith("BLOCKERS:")) {
              try {
                const report = JSON.parse(msg.slice("BLOCKERS:".length)) as BlockerReport;
                setWgBlockers(report);
                setWgDirtyRepos(false);
                setWgConfirmText("");
                setWgDeleteError("");
                setWgRetryInProgress(false);
                return;
              } catch (parseErr) {
                console.error("Failed to parse BLOCKERS: payload on retry:", parseErr);
                setWgBlockers(null);
                setWgDeleteError("Workgroup is still locked, but the blocker report could not be parsed. Try again.");
                setWgRetryInProgress(false);
                return;
              }
            }
            if (msg.startsWith("DIRTY_REPOS:")) {
              setWgBlockers(null);
              setWgDeleteError(msg.slice("DIRTY_REPOS:".length));
              setWgDirtyRepos(true);
              setWgConfirmText("");
              setWgRetryInProgress(false);
              return;
            }
            setWgBlockers(null);
            setWgDeleteError(msg);
            setWgRetryInProgress(false);
          }
        };
        const activeReplicas = createMemo(() => {
          const wg = deletingWg();
          return wg ? getActiveReplicasForWg(wg) : [];
        });

        let replicaCtxMenuEl: HTMLDivElement | undefined;
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

        const positionReplicaCtxMenu = (x: number, y: number) => {
          if (!replicaCtxMenuEl) return;

          const { width, height } = replicaCtxMenuEl.getBoundingClientRect();
          const maxX = Math.max(
            CONTEXT_MENU_VIEWPORT_MARGIN,
            window.innerWidth - width - CONTEXT_MENU_VIEWPORT_MARGIN
          );
          const maxY = Math.max(
            CONTEXT_MENU_VIEWPORT_MARGIN,
            window.innerHeight - height - CONTEXT_MENU_VIEWPORT_MARGIN
          );

          setReplicaCtxMenu((current) =>
            current
              ? {
                  ...current,
                  x: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, x), maxX),
                  y: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, y), maxY),
                }
              : current
          );
        };

        const restartReplicaSession = async (sessionId: string, agentId?: string) => {
          setReplicaCtxMenu(null);
          cleanupCtx();
          try {
            await SessionAPI.restart(sessionId, agentId ? { agentId } : undefined);
          } catch (e) {
            console.error("Failed to restart session:", e);
          }
        };

        const toggleReplicaDetach = async (sessionId: string) => {
          setReplicaCtxMenu(null);
          cleanupCtx();
          try {
            if (sessionsStore.isDetached(sessionId)) {
              await WindowAPI.attach(sessionId);
            } else {
              await WindowAPI.detach(sessionId);
            }
          } catch (e) {
            console.error("Failed to toggle detached session:", e);
          }
        };

        const handleProjectContextMenu = (e: MouseEvent) => {
          e.preventDefault();
          e.stopPropagation();
          cleanupCtx();
          setTeamCtxMenu(null);
          setWgCtxMenu(null);
          setAgentCtxMenu(null);
          setAgentsHeaderCtxMenu(null);
          setWorkgroupsHeaderCtxMenu(null);
          setLoopCtxMenu(null);
          setLoopsHeaderCtxMenu(null);
          setTeamsHeaderCtxMenu(null);
          setReplicaCtxMenu(null);
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
        const projectAutomationId = () => automationIdPart(proj.path);
        const hasLoopTargets = () =>
          proj.workgroups.some((wg) => wg.agents.some((agent) => agent.isCoordinator));
        const naturalCoordinatorItems = createMemo(() => {
          const result: { replica: AcAgentReplica; wg: AcWorkgroup }[] = [];
          for (const wg of proj.workgroups) {
            for (const replica of wg.agents) {
              if (replica.isCoordinator) {
                result.push({ replica, wg });
              }
            }
          }
          if (sessionsStore.coordSortByActivity) {
            const activityMap = sessionsStore.lastActivityBySessionId;
            const tsFor = (item: { replica: AcAgentReplica; wg: AcWorkgroup }): number => {
              const session = replicaSession(item.wg, item.replica);
              if (!session) return 0;
              return activityMap[session.id] ?? 0;
            };
            result.sort((a, b) => tsFor(b) - tsFor(a));
          }
          return result;
        });
        const coordinatorItems = createMemo(() => {
          const naturalItems = naturalCoordinatorItems();
          if (!sessionsStore.coordSortByActivity) {
            sessionsStore.recordCoordinatorVisibleOrder(proj.path, naturalItems.map(coordinatorItemKey));
            return naturalItems;
          }

          const naturalByKey = new Map(naturalItems.map((item) => [coordinatorItemKey(item), item]));
          const visibleKeys = sessionsStore.coordinatorVisibleOrder(proj.path, naturalItems.map(coordinatorItemKey));
          const visibleItems = visibleKeys
            .map((key) => naturalByKey.get(key))
            .filter((item): item is { replica: AcAgentReplica; wg: AcWorkgroup } => item !== undefined);
          sessionsStore.recordCoordinatorVisibleOrder(proj.path, visibleKeys);
          return visibleItems;
        });
        const selectedCoordinatorItem = createMemo(() =>
          coordinatorItems().find((item) => replicaSession(item.wg, item.replica)?.id === sessionsStore.activeId) ?? null
        );
        const selectedWorkgroup = createMemo<AcWorkgroup | null>((prev) => {
          const coord = selectedCoordinatorItem();
          if (coord) return coord.wg;
          for (const wg of proj.workgroups) {
            for (const replica of wg.agents) {
              if (replicaSession(wg, replica)?.id === sessionsStore.activeId) {
                return wg;
              }
            }
          }
          if (!prev) return null;
            return proj.workgroups.find(w => w.name === prev.name) ?? null;
        });

        const runLoopAction = async (
          loop: AcLoopSummary,
          action: "run" | "toggle" | "delete",
          task: () => Promise<void>
        ) => {
          const actionKey = `${loop.id}:${action}`;
          if (loopActionInProgress()) return;
          setLoopActionInProgress(actionKey);
          try {
            await task();
            await projectStore.reloadProject(proj.path);
          } catch (e) {
            console.error(`Loop ${action} failed:`, e);
          } finally {
            setLoopActionInProgress(null);
          }
        };

        const handleLoopContextMenu = (e: MouseEvent, loop: AcLoopSummary) => {
          e.preventDefault();
          e.stopPropagation();
          cleanupCtx();
          setShowCtxMenu(false);
          setTeamCtxMenu(null);
          setWgCtxMenu(null);
          setAgentCtxMenu(null);
          setAgentsHeaderCtxMenu(null);
          setWorkgroupsHeaderCtxMenu(null);
          setLoopsHeaderCtxMenu(null);
          setTeamsHeaderCtxMenu(null);
          setReplicaCtxMenu(null);
          setLoopCtxMenu({ loop, x: e.clientX, y: e.clientY });
          const dismiss = (ev?: Event) => {
            if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
            setLoopCtxMenu(null);
            cleanupCtx();
          };
          dismissCtx = dismiss;
          setTimeout(() => {
            window.addEventListener("click", dismiss);
            window.addEventListener("contextmenu", dismiss);
            window.addEventListener("keydown", dismiss as any);
          });
        };

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
          setWorkgroupsHeaderCtxMenu(null);
          setLoopCtxMenu(null);
          setLoopsHeaderCtxMenu(null);
          setTeamsHeaderCtxMenu(null);
          setReplicaCtxMenu(null);
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
          setWorkgroupsHeaderCtxMenu(null);
          setLoopCtxMenu(null);
          setLoopsHeaderCtxMenu(null);
          setTeamsHeaderCtxMenu(null);
          setReplicaCtxMenu(null);
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

        const handleReplicaContextMenu = (e: MouseEvent, session: Session) => {
          e.preventDefault();
          e.stopPropagation();
          cleanupCtx();
          setShowCtxMenu(false);
          setTeamCtxMenu(null);
          setWgCtxMenu(null);
          setAgentCtxMenu(null);
          setAgentsHeaderCtxMenu(null);
          setWorkgroupsHeaderCtxMenu(null);
          setLoopCtxMenu(null);
          setLoopsHeaderCtxMenu(null);
          setTeamsHeaderCtxMenu(null);
          setReplicaCtxMenu({
            sessionId: session.id,
            sessionName: session.name,
            x: e.clientX,
            y: e.clientY,
          });
          const dismiss = (ev?: Event) => {
            if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
            setReplicaCtxMenu(null);
            cleanupCtx();
          };
          dismissCtx = dismiss;
          setTimeout(() => {
            positionReplicaCtxMenu(e.clientX, e.clientY);
            window.addEventListener("click", dismiss);
            window.addEventListener("contextmenu", dismiss);
            window.addEventListener("keydown", dismiss as any);
          });
        };

        const renderReplicaItem = (
          replica: AcAgentReplica,
          wg: AcWorkgroup,
          extraBadge?: string,
          runningPeers?: () => AcAgentReplica[],
          taskTitle?: string | null,
          rowContext = "workgroups"
        ) => {
          const dotClass = () => replicaDotClass(wg, replica);
          const isCoord = () => replica.isCoordinator;
          const session = () => replicaSession(wg, replica);
          const repoBadges = createMemo(() => {
            const s = session();
            return s && s.gitRepos.length > 0
              ? s.gitRepos
              : configuredReplicaRepoBadges(replica, wg);
          });
          const rowTestId = () =>
            `replica.row.${automationIdPart(rowContext)}.${automationIdPart(wg.name)}.${automationIdPart(replica.name)}`;
          const badgesTestId = () =>
            `replica.badges.${automationIdPart(rowContext)}.${automationIdPart(wg.name)}.${automationIdPart(replica.name)}`;
          const repoBadgeTestId = (label: string, index: number) =>
            `replica.repoBadge.${automationIdPart(rowContext)}.${automationIdPart(wg.name)}.${automationIdPart(replica.name)}.${index}.${automationIdPart(label)}`;
          const liveAgentLabel = () => {
            const s = session();
            if (!s) return null;
            if (s.agentLabel) return s.agentLabel;
            if (!s.agentId) return null;
            return settingsStore.current?.agents?.find((a) => a.id === s.agentId)?.label ?? null;
          };
          const isLive = () => isSessionLive(session());
          const bridge = () => { const s = session(); return s ? bridgesStore.getBridge(s.id) : undefined; };
          const isDetached = () => { const s = session(); return s ? sessionsStore.isDetached(s.id) : false; };
          const isRecording = () => { const s = session(); return s ? voiceRecorder.recordingSessionId() === s.id : false; };
          const isProcessing = () => { const s = session(); return s ? voiceRecorder.processingSessionId() === s.id : false; };
          const [showBotMenu, setShowBotMenu] = createSignal(false);
          const [availableBots, setAvailableBots] = createSignal<TelegramBotConfig[]>([]);

          const handleMicClick = (e: MouseEvent) => {
            e.stopPropagation();
            if (!settingsStore.voiceEnabled) {
              emitOpenSettings("integrations").catch(console.error);
              return;
            }
            const s = session();
            if (s) voiceRecorder.toggle(s.id);
          };
          const handleCancelRecording = (e: MouseEvent) => {
            e.stopPropagation();
            voiceRecorder.cancel();
          };
          const handleOpenExplorer = async (e: MouseEvent) => {
            e.stopPropagation();
            const s = session();
            try { await WindowAPI.openInExplorer(s ? s.workingDirectory : replica.path); } catch (err) { console.error("Failed to open explorer:", err); }
          };
          const handleDetach = async (e: MouseEvent) => {
            e.stopPropagation();
            const s = session();
            if (!s) return;
            try {
              if (isDetached()) {
                await WindowAPI.attach(s.id);
              } else {
                await WindowAPI.detach(s.id);
              }
            } catch (err) {
              console.error("replica detach/attach toggle failed:", err);
            }
          };
          const handleTelegramClick = async (e: MouseEvent) => {
            e.stopPropagation();
            const s = session();
            if (!s) return;
            const b = bridge();
            if (b) {
              await TelegramAPI.detach(s.id);
            } else {
              const settings = await SettingsAPI.get();
              const bots = settings.telegramBots || [];
              if (bots.length === 1) {
                await TelegramAPI.attach(s.id, bots[0].id);
              } else if (bots.length > 1) {
                setAvailableBots(bots);
                setShowBotMenu(true);
              }
            }
          };
          const handleBotSelect = async (botId: string) => {
            setShowBotMenu(false);
            const s = session();
            if (s) await TelegramAPI.attach(s.id, botId);
          };
          const handleClose = (e: MouseEvent) => {
            e.stopPropagation();
            const s = session();
            if (s) SessionAPI.destroy(s.id);
          };

          return (
            <div
              class="replica-item"
              classList={{ active: session()?.id === sessionsStore.activeId }}
              data-ac-testid={rowTestId()}
              onClick={() => handleReplicaClick(replica, wg)}
              onContextMenu={(e) => {
                const s = session();
                if (s) handleReplicaContextMenu(e, s);
              }}
              title={replica.path}
            >
              <div class={`session-item-status ${dotClass()}`} />
              <div class="replica-item-info">
                <Show when={taskTitle}>
                  <span class="coord-task-title" title={taskTitle ?? undefined}>{taskTitle}</span>
                </Show>
                <span class="replica-item-name">{replica.originProject ? `${replica.name}@${replica.originProject}` : replica.name}</span>
                <div class="ac-discovery-badges" data-ac-testid={badgesTestId()}>
                  <Show when={runningPeers && runningPeers()!.length > 0}>
                    <For each={runningPeers!()}>
                      {(peer) => (
                        <span
                          class="ac-discovery-badge running-peer"
                          title={`${wg.name}/${peer.name}`}
                        >
                          {peer.name} RUNNING
                        </span>
                      )}
                    </For>
                  </Show>
                  <Show when={isCoord() && repoBadges().length > 0}>
                    <For each={repoBadges()}>
                      {(repo, index) => (
                        <span
                          class="ac-discovery-badge branch"
                          title={repo.sourcePath}
                          data-ac-testid={repoBadgeTestId(repo.label, index())}
                        >
                          {formatReplicaRepoBadgeLabel(repo)}
                        </span>
                      )}
                    </For>
                  </Show>
                  <Show when={liveAgentLabel()}>
                    <span class="ac-discovery-badge agent">{liveAgentLabel()}</span>
                  </Show>
                  <Show when={isCoord()}>
                    <span class="ac-discovery-badge coord">coordinator</span>
                  </Show>
                  <Show when={extraBadge}>
                    <span class="ac-discovery-badge team">{extraBadge}</span>
                  </Show>
                </div>
              </div>
              <Show when={isLive()}>
                <Show when={isRecording()}>
                  <button class="session-item-mic-cancel" onClick={handleCancelRecording} title="Cancel recording">&#x2715;</button>
                </Show>
                <button
                  class={`session-item-mic ${isRecording() ? "recording" : ""} ${isProcessing() ? "processing" : ""} ${voiceRecorder.micError() ? "error" : ""} ${!settingsStore.voiceEnabled ? "disabled" : ""}`}
                  onClick={handleMicClick}
                  title={!settingsStore.voiceEnabled ? "Enable voice-to-text in Settings and set a Gemini API key to use this." : isRecording() ? "Stop recording" : isProcessing() ? "Transcribing..." : voiceRecorder.micError() ? voiceRecorder.micError()! : "Voice to text"}
                >&#x1F399;</button>
                <button class="session-item-explorer" onClick={handleOpenExplorer} title="Open folder in explorer">&#x1F4C2;</button>
                <button
                  class="session-item-detach"
                  classList={{ attached: isDetached() }}
                  onClick={handleDetach}
                  title={isDetached() ? "Re-attach to main window" : "Open in new window"}
                  innerHTML={isDetached() ? "&#x2934;" : "&#x29C9;"}
                />
                <Show when={bridge()}>
                  <div class="session-item-bridge-dot" style={{ background: bridge()!.color }} title={`Telegram: ${bridge()!.botLabel}`} />
                </Show>
                <button
                  class={`session-item-telegram ${bridge() ? "active" : ""}`}
                  onClick={handleTelegramClick}
                  title={bridge() ? "Detach Telegram" : "Attach Telegram"}
                  style={bridge() ? { color: bridge()!.color } : {}}
                ><TelegramIcon /></button>
                <Show when={showBotMenu()}>
                  <div class="session-item-bot-menu" onClick={(e) => e.stopPropagation()}>
                    <For each={availableBots()}>
                      {(bot) => (
                        <button class="session-item-bot-option" onClick={() => handleBotSelect(bot.id)}>
                          <span class="settings-color-dot" style={{ background: bot.color }} />
                          {bot.label}
                        </button>
                      )}
                    </For>
                  </div>
                </Show>
                <button class="session-item-close" onClick={handleClose} title="Close session">&#x2715;</button>
              </Show>
            </div>
          );
        };

        const renderWorkgroupSubgroup = (wg: AcWorkgroup, rowContext: string) => {
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
                  <Show when={wg.taskTitle?.trim() || stripFrontmatter(wg.taskTitle ?? "").trim()}>
                    {(text) => <span class="ac-wg-task">{text()}</span>}
                  </Show>
                </div>
              </div>
              <Show when={!wgCollapsed()}>
                <For each={wg.agents}>
                  {(replica) => renderReplicaItem(replica, wg, undefined, undefined, undefined, rowContext)}
                </For>
              </Show>
            </div>
          );
        };

        return (
          <div class="project-panel">
            <button
              class="project-header"
              title={proj.path}
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
                  <button
                    class="session-context-option"
                    classList={{ "context-option-disabled": !hasLoopTargets() }}
                    disabled={!hasLoopTargets()}
                    onClick={() => {
                      if (!hasLoopTargets()) return;
                      setShowCtxMenu(false);
                      setShowNewLoop(true);
                    }}
                    data-ac-testid={`loop.action.new.${projectAutomationId()}.projectMenu`}
                  >
                    New Loop
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
            {showNewLoop() && (
              <Portal>
                <NewLoopModal
                  projectPath={proj.path}
                  workgroups={proj.workgroups}
                  onClose={() => setShowNewLoop(false)}
                />
              </Portal>
            )}
            {editingLoop() && (
              <Portal>
                <EditLoopModal
                  projectPath={proj.path}
                  workgroups={proj.workgroups}
                  loop={editingLoop()!}
                  onClose={() => setEditingLoop(null)}
                />
              </Portal>
            )}

            <Show when={!collapsed()}>
              <div class="project-content">
                {/* Coordinator Quick-Access — shown by styles that enable it via CSS */}
                {(() => {
                  return (
                    <Show when={coordinatorItems().length > 0}>
                      <div class="coord-quick-access">
                        <For each={coordinatorItems()}>
                          {(item) => {
                            const runningPeers = createMemo(() =>
                              item.wg.agents.filter((peer) => {
                                if (peer.name === item.replica.name) return false;
                                const dot = replicaDotClass(item.wg, peer);
                                return dot === "running" || dot === "active";
                              })
                            );
                            return renderReplicaItem(item.replica, item.wg, item.wg.name, runningPeers, item.wg.taskTitle, "quick");
                          }}
                        </For>
                      </div>
                    </Show>
                  );
                })()}
                {/* Selected Workgroup */}
                {(() => {
                  const [selectedCollapsed, setSelectedCollapsed] = createSignal(false);

                  return (
                    <Show when={sessionsStore.showCategories || sessionsStore.alwaysShowSelectedWorkgroup}>
                      <div class="ac-wg-group">
                        <div
                          class="ac-wg-header ac-wg-header--collapsible"
                          onClick={() => setSelectedCollapsed((c) => !c)}
                        >
                          <span class="ac-discovery-chevron" classList={{ collapsed: selectedCollapsed() }}>
                            &#x25BE;
                          </span>
                          <div class="ac-wg-header-text">
                            <span class="ac-wg-name">Selected Workgroup</span>
                          </div>
                          <span class="ac-team-count">{selectedWorkgroup() ? 1 : 0}</span>
                        </div>
                        <Show when={!selectedCollapsed()}>
                          <Show when={selectedWorkgroup()} fallback={<div class="ac-empty-hint">No selected workgroup</div>}>
                            <For each={[selectedWorkgroup()!]}>
                              {(wg) => renderWorkgroupSubgroup(wg, "selected")}
                            </For>
                          </Show>
                        </Show>
                      </div>
                    </Show>
                  );
                })()}
                {/* Workgroups */}
                {(() => {
                  const [wgsCollapsed, setWgsCollapsed] = createSignal(false);

                  const handleWorkgroupsHeaderContextMenu = (e: MouseEvent) => {
                    e.preventDefault();
                    e.stopPropagation();
                    cleanupCtx();
                    setShowCtxMenu(false);
                    setTeamCtxMenu(null);
                    setWgCtxMenu(null);
                    setAgentCtxMenu(null);
                    setAgentsHeaderCtxMenu(null);
                    setTeamsHeaderCtxMenu(null);
                    setReplicaCtxMenu(null);
                    setLoopCtxMenu(null);
                    setLoopsHeaderCtxMenu(null);
                    setWorkgroupsHeaderCtxMenu({ x: e.clientX, y: e.clientY });
                    const dismiss = (ev?: Event) => {
                      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
                      setWorkgroupsHeaderCtxMenu(null);
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
                    <Show when={sessionsStore.showCategories}>
                    <div class="ac-wg-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        onClick={() => setWgsCollapsed((c) => !c)}
                        onContextMenu={handleWorkgroupsHeaderContextMenu}
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
                          fallback={<div class="ac-empty-hint">No workgroups</div>}
                        >
                          <For each={proj.workgroups}>
                            {(wg) => renderWorkgroupSubgroup(wg, "workgroups")}
                          </For>
                        </Show>
                      </Show>
                    </div>
                    </Show>

                    {/* Workgroups header context menu */}
                    {workgroupsHeaderCtxMenu() && (
                      <Portal>
                        <div
                          class="session-context-menu"
                          style={{ left: `${workgroupsHeaderCtxMenu()!.x}px`, top: `${workgroupsHeaderCtxMenu()!.y}px` }}
                          onClick={(e) => e.stopPropagation()}
                        >
                          <button
                            class="session-context-option"
                            classList={{ "context-option-disabled": !hasTeams() }}
                            disabled={!hasTeams()}
                            onClick={() => {
                              if (!hasTeams()) return;
                              setWorkgroupsHeaderCtxMenu(null);
                              setShowNewWorkgroup(true);
                            }}
                          >
                            New Workgroup
                          </button>
                        </div>
                      </Portal>
                    )}
                    </>
                  );
                })()}
                {/* Loops */}
                {(() => {
                  const [loopsCollapsed, setLoopsCollapsed] = createSignal(false);

                  const handleLoopsHeaderContextMenu = (e: MouseEvent) => {
                    e.preventDefault();
                    e.stopPropagation();
                    cleanupCtx();
                    setShowCtxMenu(false);
                    setTeamCtxMenu(null);
                    setWgCtxMenu(null);
                    setAgentCtxMenu(null);
                    setAgentsHeaderCtxMenu(null);
                    setWorkgroupsHeaderCtxMenu(null);
                    setLoopCtxMenu(null);
                    setTeamsHeaderCtxMenu(null);
                    setReplicaCtxMenu(null);
                    setLoopsHeaderCtxMenu({ x: e.clientX, y: e.clientY });
                    const dismiss = (ev?: Event) => {
                      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
                      setLoopsHeaderCtxMenu(null);
                      cleanupCtx();
                    };
                    dismissCtx = dismiss;
                    setTimeout(() => {
                      window.addEventListener("click", dismiss);
                      window.addEventListener("contextmenu", dismiss);
                      window.addEventListener("keydown", dismiss);
                    });
                  };

                  const loopTestId = (loop: AcLoopSummary) =>
                    `loop.row.${projectAutomationId()}.${automationIdPart(loop.id)}`;

                  return (
                    <>
                    <Show when={sessionsStore.showCategories}>
                    <div class="ac-wg-group ac-loop-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        onClick={() => setLoopsCollapsed((c) => !c)}
                        onContextMenu={handleLoopsHeaderContextMenu}
                        data-ac-testid={`project.loops.header.${projectAutomationId()}`}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: loopsCollapsed() }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Loops</span>
                        </div>
                        <span class="ac-team-count">{proj.loops.length}</span>
                      </div>
                      <Show when={!loopsCollapsed()}>
                        <Show
                          when={proj.loops.length > 0}
                          fallback={<div class="ac-empty-hint">No loops</div>}
                        >
                          <For each={proj.loops}>
                            {(loop) => (
                              <div
                                class="ac-loop-row"
                                classList={{
                                  "ac-loop-row-disabled": !loop.enabled,
                                  "ac-loop-row-pending": !!loop.pendingDueAt,
                                  "ac-loop-row-missed": loop.lastResult?.kind === "missedWhileClosed",
                                }}
                                onClick={() => setEditingLoop(loop)}
                                onContextMenu={(e) => handleLoopContextMenu(e, loop)}
                                title={loop.promptPreview}
                                data-ac-testid={loopTestId(loop)}
                                data-ac-state={[
                                  loop.enabled ? "enabled" : "loop-disabled",
                                  loop.pendingDueAt ? "pending" : "",
                                ].filter(Boolean).join(" ")}
                              >
                                <div class="ac-loop-main">
                                  <span class="ac-loop-name">{loop.name}</span>
                                  <span class="ac-loop-target">{loop.workgroup}</span>
                                </div>
                                <div class="ac-loop-meta">
                                  <span>{loop.expr}</span>
                                  <Show when={loop.pendingDueAt}>
                                    <span class="ac-discovery-badge pending">pending</span>
                                  </Show>
                                  <Show when={!loop.enabled}>
                                    <span class="ac-discovery-badge disabled">disabled</span>
                                  </Show>
                                </div>
                                <div class="ac-loop-status">
                                  {loop.lastResult?.message ?? (loop.nextDueAt ? `Next: ${new Date(loop.nextDueAt).toLocaleString()}` : "No runs yet")}
                                </div>
                              </div>
                            )}
                          </For>
                        </Show>
                      </Show>
                    </div>
                    </Show>

                    {/* Loops header context menu */}
                    {loopsHeaderCtxMenu() && (
                      <Portal>
                        <div
                          class="session-context-menu"
                          style={{ left: `${loopsHeaderCtxMenu()!.x}px`, top: `${loopsHeaderCtxMenu()!.y}px` }}
                          onClick={(e) => e.stopPropagation()}
                        >
                          <button
                            class="session-context-option"
                            classList={{ "context-option-disabled": !hasLoopTargets() }}
                            disabled={!hasLoopTargets()}
                            onClick={() => {
                              if (!hasLoopTargets()) return;
                              setLoopsHeaderCtxMenu(null);
                              setShowNewLoop(true);
                            }}
                            data-ac-testid={`loop.action.new.${projectAutomationId()}`}
                          >
                            New Loop
                          </button>
                        </div>
                      </Portal>
                    )}
                    </>
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
                    setWorkgroupsHeaderCtxMenu(null);
                    setTeamsHeaderCtxMenu(null);
                    setReplicaCtxMenu(null);
                    setLoopCtxMenu(null);
                    setLoopsHeaderCtxMenu(null);
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
                    setWorkgroupsHeaderCtxMenu(null);
                    setTeamsHeaderCtxMenu(null);
                    setReplicaCtxMenu(null);
                    setLoopCtxMenu(null);
                    setLoopsHeaderCtxMenu(null);
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
                    <Show when={sessionsStore.showCategories}>
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
                          fallback={<div class="ac-empty-hint">No agents</div>}
                        >
                          <For each={proj.agents}>
                            {(agent) => {
                              const session = () => sessionsStore.findSessionByName(agent.name);
                              return (
                                <Show
                                  when={session()}
                                  fallback={
                                    <div
                                      class="replica-item"
                                      onClick={() => handleAgentClick(agent)}
                                      onContextMenu={(e) => handleAgentContextMenu(e, agent)}
                                      title={agent.path}
                                    >
                                      <div class="session-item-status offline" />
                                      <div class="replica-item-info">
                                        <span class="replica-item-name">
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
                    </Show>

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
                        <div class="modal-overlay">
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

                  const handleTeamsHeaderContextMenu = (e: MouseEvent) => {
                    e.preventDefault();
                    e.stopPropagation();
                    cleanupCtx();
                    setShowCtxMenu(false);
                    setTeamCtxMenu(null);
                    setWgCtxMenu(null);
                    setAgentCtxMenu(null);
                    setAgentsHeaderCtxMenu(null);
                    setWorkgroupsHeaderCtxMenu(null);
                    setReplicaCtxMenu(null);
                    setLoopCtxMenu(null);
                    setLoopsHeaderCtxMenu(null);
                    setTeamsHeaderCtxMenu({ x: e.clientX, y: e.clientY });
                    const dismiss = (ev?: Event) => {
                      if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
                      setTeamsHeaderCtxMenu(null);
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
                    <Show when={sessionsStore.showCategories}>
                    <div class="ac-wg-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        onClick={() => setTeamsCollapsed((c) => !c)}
                        onContextMenu={handleTeamsHeaderContextMenu}
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
                          fallback={<div class="ac-empty-hint">No teams</div>}
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
                    </Show>

                    {/* Teams header context menu */}
                    {teamsHeaderCtxMenu() && (
                      <Portal>
                        <div
                          class="session-context-menu"
                          style={{ left: `${teamsHeaderCtxMenu()!.x}px`, top: `${teamsHeaderCtxMenu()!.y}px` }}
                          onClick={(e) => e.stopPropagation()}
                        >
                          <button
                            class="session-context-option"
                            onClick={() => {
                              setTeamsHeaderCtxMenu(null);
                              setShowNewTeam(true);
                            }}
                          >
                            New Team
                          </button>
                        </div>
                      </Portal>
                    )}
                    </>
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

            {/* Loop context menu */}
            {loopCtxMenu() && (
              <Portal>
                <div
                  class="session-context-menu"
                  style={{ left: `${loopCtxMenu()!.x}px`, top: `${loopCtxMenu()!.y}px` }}
                  onClick={(e) => e.stopPropagation()}
                >
                  <button
                    class="session-context-option"
                    disabled={loopActionInProgress() === `${loopCtxMenu()!.loop.id}:run`}
                    onClick={async () => {
                      const menu = loopCtxMenu();
                      setLoopCtxMenu(null);
                      cleanupCtx();
                      if (!menu) return;
                      await runLoopAction(menu.loop, "run", async () => {
                        await LoopAPI.runNow(proj.path, menu.loop.id);
                      });
                    }}
                    data-ac-testid={`loop.action.runNow.${projectAutomationId()}.${automationIdPart(loopCtxMenu()!.loop.id)}`}
                  >
                    Run Now
                  </button>
                  <button
                    class="session-context-option"
                    onClick={() => {
                      const menu = loopCtxMenu();
                      setLoopCtxMenu(null);
                      cleanupCtx();
                      if (menu) setEditingLoop(menu.loop);
                    }}
                    data-ac-testid={`loop.action.edit.${projectAutomationId()}.${automationIdPart(loopCtxMenu()!.loop.id)}`}
                  >
                    Edit
                  </button>
                  <button
                    class="session-context-option"
                    disabled={loopActionInProgress() === `${loopCtxMenu()!.loop.id}:toggle`}
                    onClick={async () => {
                      const menu = loopCtxMenu();
                      setLoopCtxMenu(null);
                      cleanupCtx();
                      if (!menu) return;
                      await runLoopAction(menu.loop, "toggle", async () => {
                        await LoopAPI.setEnabled(proj.path, menu.loop.id, !menu.loop.enabled);
                      });
                    }}
                    data-ac-testid={`loop.action.toggle.${projectAutomationId()}.${automationIdPart(loopCtxMenu()!.loop.id)}`}
                  >
                    {loopCtxMenu()!.loop.enabled ? "Disable" : "Enable"}
                  </button>
                  <div class="context-separator" />
                  <button
                    class="session-context-option context-option-danger"
                    disabled={loopActionInProgress() === `${loopCtxMenu()!.loop.id}:delete`}
                    onClick={async () => {
                      const menu = loopCtxMenu();
                      setLoopCtxMenu(null);
                      cleanupCtx();
                      if (!menu) return;
                      const confirmed = window.confirm(`Delete Loop '${menu.loop.name}'?`);
                      if (!confirmed) return;
                      await runLoopAction(menu.loop, "delete", async () => {
                        await LoopAPI.delete(proj.path, menu.loop.id);
                      });
                    }}
                    data-ac-testid={`loop.action.delete.${projectAutomationId()}.${automationIdPart(loopCtxMenu()!.loop.id)}`}
                  >
                    Delete Loop
                  </button>
                </div>
              </Portal>
            )}

            {/* Replica context menu */}
            {replicaCtxMenu() && (
              <Portal>
                <div
                  class="session-context-menu"
                  ref={replicaCtxMenuEl}
                  style={{ left: `${replicaCtxMenu()!.x}px`, top: `${replicaCtxMenu()!.y}px` }}
                  onClick={(e) => e.stopPropagation()}
                >
                  <button
                    class="session-context-option context-option-danger"
                    onClick={async () => {
                      const menu = replicaCtxMenu();
                      if (menu) {
                        await restartReplicaSession(menu.sessionId);
                      }
                    }}
                  >
                    Restart Session
                  </button>
                  <button
                    class="session-context-option"
                    onClick={() => {
                      const menu = replicaCtxMenu();
                      setReplicaCtxMenu(null);
                      cleanupCtx();
                      if (menu) {
                        setReplicaCodingAgentTarget({
                          sessionId: menu.sessionId,
                          sessionName: menu.sessionName,
                        });
                      }
                    }}
                  >
                    Coding Agent
                  </button>
                  <div class="context-separator" />
                  <button
                    class="session-context-option"
                    onClick={() => {
                      const menu = replicaCtxMenu();
                      if (menu) toggleReplicaDetach(menu.sessionId);
                    }}
                  >
                    {sessionsStore.isDetached(replicaCtxMenu()!.sessionId) ? "Re-attach to main" : "Open in new window"}
                  </button>
                </div>
              </Portal>
            )}
            {replicaCodingAgentTarget() && (
              <Portal>
                <AgentPickerModal
                  sessionName={replicaCodingAgentTarget()!.sessionName}
                  onSelect={async (agent) => {
                    const target = replicaCodingAgentTarget();
                    setReplicaCodingAgentTarget(null);
                    if (target) {
                      await restartReplicaSession(target.sessionId, agent.id);
                    }
                  }}
                  onClose={() => setReplicaCodingAgentTarget(null)}
                />
              </Portal>
            )}

            {/* Delete WG confirmation */}
            {deletingWg() && (
              <Portal>
                <div class="modal-overlay">
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
                      <Show when={wgBlockers()}>
                        {(r) => {
                          const normalized = () => normalizeBlockerReport(r());
                          const liveSessions = () => normalized().liveSessions;
                          const externalProcesses = () => normalized().externalProcesses;
                          const ignoredExited = () => normalized().ignoredExited;
                          const rawDeleteError = () => normalized().rawDeleteError;
                          const rmError = () => normalized().restartManagerError?.message;
                          return (
                            <div style={{
                              "background": "var(--danger, #c0392b)",
                              "color": "#fff",
                              "padding": "10px 12px",
                              "border-radius": "6px",
                              "margin-top": "10px",
                              "font-size": "12px",
                              "line-height": "1.5",
                            }}>
                              <strong>Cannot delete:</strong> Windows reported the workgroup is locked.
                              <Show when={liveSessions().length > 0}>
                                <div style={{ "margin-top": "6px" }}><strong>Live AC sessions</strong></div>
                                <ul style={{ margin: "4px 0 6px 16px", padding: "0" }}>
                                  <For each={liveSessions()}>
                                    {(s) => <li>{s.agentName} <span style={{ opacity: 0.75 }}>({s.cwd})</span></li>}
                                  </For>
                                </ul>
                              </Show>
                              <Show when={externalProcesses().length > 0}>
                                <div style={{ "margin-top": "6px" }}><strong>External processes</strong></div>
                                <ul style={{ margin: "4px 0 6px 16px", padding: "0" }}>
                                  <For each={externalProcesses()}>
                                    {(p) => (
                                      <li>
                                        {p.name} (PID {p.pid})
                                        <Show when={p.cwd}>
                                          {(cwd) => (
                                            <div style={{ "font-size": "11px", opacity: 0.85 }}>
                                              CWD: {cwd()}
                                            </div>
                                          )}
                                        </Show>
                                        <Show when={p.files.length > 0}>
                                          <ul style={{ margin: "2px 0 0 16px", padding: "0", "font-size": "11px", opacity: 0.85 }}>
                                            <For each={p.files}>{(f) => <li>{f}</li>}</For>
                                          </ul>
                                        </Show>
                                      </li>
                                    )}
                                  </For>
                                </ul>
                              </Show>
                              <Show when={liveSessions().length === 0 && ignoredExited().length > 0}>
                                <div style={{ "margin-top": "6px", opacity: 0.9 }}>
                                  Ignored {ignoredExited().length} exited AC session record{ignoredExited().length === 1 ? "" : "s"}.
                                  These are not treated as blockers.
                                </div>
                              </Show>
                              <Show when={rmError()}>
                                {(message) => (
                                  <div style={{ "margin-top": "6px", opacity: 0.9 }}>
                                    Restart Manager could not identify blockers: <code>{message()}</code>
                                  </div>
                                )}
                              </Show>
                              <Show when={!r().diagnosticAvailable && r().platform !== "windows"}>
                                <div style={{ "margin-top": "6px", opacity: 0.85 }}>
                                  Diagnostic not available on this platform. Raw delete error: <code>{rawDeleteError()}</code>
                                </div>
                              </Show>
                              <Show when={!r().diagnosticAvailable && r().platform === "windows" && rmError()}>
                                <div style={{ "margin-top": "6px", opacity: 0.85 }}>
                                  Blocker identification failed. Raw delete error: <code>{rawDeleteError()}</code>
                                </div>
                              </Show>
                              <Show when={liveSessions().length === 0 && externalProcesses().length === 0}>
                                <div style={{ "margin-top": "6px", opacity: 0.85 }}>
                                  No live AC sessions or external blocker processes were identified. Windows still rejected the delete probe.
                                  Raw delete error: <code>{rawDeleteError()}</code>
                                </div>
                              </Show>
                              <Show
                                when={liveSessions().length > 0 || externalProcesses().length > 0}
                                fallback={<div style={{ "margin-top": "8px" }}>Close any app that may be using files in this workgroup, then click <strong>Retry</strong> below.</div>}
                              >
                                <div style={{ "margin-top": "8px" }}>
                                  Close the listed sessions or quit the listed processes, then click <strong>Retry</strong> below.
                                </div>
                              </Show>
                              <div style={{ "margin-top": "10px", display: "flex", "justify-content": "flex-end" }}>
                                <button
                                  class="new-agent-create-btn"
                                  style={{ "background": "#fff", "color": "var(--danger, #c0392b)", "min-width": "84px" }}
                                  disabled={wgRetryInProgress() || wgDeleteInProgress()}
                                  onClick={retryWgDelete}
                                >
                                  {wgRetryInProgress() ? "Retrying…" : "Retry"}
                                </button>
                              </div>
                            </div>
                          );
                        }}
                      </Show>
                    </div>
                    <div class="new-agent-footer">
                      <button class="new-agent-cancel-btn" onClick={closeWgDeleteModal}>
                        Cancel
                      </button>
                      <button
                        class="new-agent-create-btn"
                        style={{ "background": "var(--danger, #c0392b)" }}
                        disabled={
                          wgDeleteInProgress()
                          || activeReplicas().length > 0
                          || (wgDirtyRepos() && wgConfirmText() !== deletingWg()!.name)
                          || wgBlockers() !== null
                        }
                        onClick={async () => {
                          if (wgDeleteInProgress()) return;
                          if (activeReplicas().length > 0) return;
                          setWgDeleteInProgress(true);
                          const myGen = ++retryGen;
                          const wg = deletingWg()!;
                          const forceDelete = wgDirtyRepos();
                          setWgLastForceUsed(forceDelete);
                          try {
                            await EntityAPI.deleteWorkgroup(proj.path, wg.name, forceDelete);
                            if (myGen !== retryGen) return;
                            await projectStore.reloadProject(proj.path);
                            if (myGen !== retryGen) return;
                          } catch (e: any) {
                            if (myGen !== retryGen) return;
                            console.error("delete_workgroup failed:", e);
                            const msg = typeof e === "string" ? e : e?.message ?? "Failed to delete workgroup";
                            // BLOCKERS: sentinel — render structured blocker list, no force-delete option.
                            if (msg.startsWith("BLOCKERS:")) {
                              try {
                                const report = JSON.parse(msg.slice("BLOCKERS:".length)) as BlockerReport;
                                setWgBlockers(report);
                                setWgDirtyRepos(false);
                                setWgConfirmText("");
                                setWgDeleteError("");
                                setWgDeleteInProgress(false);
                                return;
                              } catch (parseErr) {
                                console.error("Failed to parse BLOCKERS: payload:", parseErr);
                                setWgDeleteError("Workgroup is locked, but the blocker report could not be parsed. Try again.");
                                setWgDeleteInProgress(false);
                                return;
                              }
                            }
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
                <div class="modal-overlay">
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
                        onClick={closeTeamDeleteModal}
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
                          closeTeamDeleteModal();
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
              gitRepos: pending.gitRepos,
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
