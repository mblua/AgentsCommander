import { Component, For, Show, createEffect, createMemo, createSignal, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcWorkgroup, AcAgentReplica, AcTeam, AcLoopSummary, Session, TelegramBotConfig, BlockerReport } from "../../shared/types";
import { SessionAPI, WindowAPI, EntityAPI, LoopAPI, TelegramAPI, SettingsAPI, TaskAPI, onDiscoveryBranchUpdated, emitOpenSettings } from "../../shared/ipc";
import type { SessionRepoInput } from "../../shared/ipc";
import { isTauri } from "../../shared/platform";
import { stripFrontmatter } from "../../shared/markdown";
import { launchErrorMessage } from "../../shared/launch-errors";
import { projectStore } from "../stores/project";
import { sessionsStore } from "../stores/sessions";
import { bridgesStore } from "../stores/bridges";
import { settingsStore } from "../../shared/stores/settings";
import { voiceRecorder } from "../../shared/voice-recorder";
import { isWgReplicaPath, sessionProfileBadge, shouldOfferRestartAfterAssign } from "../../shared/profile-utils";
import SessionItem from "./SessionItem";
import NewEntityAgentModal from "./NewEntityAgentModal";
import NewTeamModal from "./NewTeamModal";
import NewWorkgroupModal from "./NewWorkgroupModal";
import NewLoopModal from "./NewLoopModal";
import EditLoopModal from "./EditLoopModal";
import AgentPickerModal, { type AgentPickerScopeContext } from "./AgentPickerModal";
import RestartPromptModal from "./RestartPromptModal";
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
  currentAgentId?: string;
  currentRequestedProfile?: string | null;
  scopeContext?: AgentPickerScopeContext;
}

/** #545: the broom has nothing to clear when the workgroup task title is
 *  empty/missing or the literal Clean sentinel. Title-only approximation of
 *  the backend's structural clean check; exact-match + case-sensitive on
 *  purpose (mirrors the backend `title: 'Clean'` sentinel). See plan G2/G4. */
export const isTaskClean = (t?: string | null): boolean =>
  !t?.trim() || t.trim() === "Clean";

function joinSearchText(...parts: Array<string | null | undefined | false>): string {
  return parts
    .filter((part): part is string => typeof part === "string" && part.trim().length > 0)
    .join(" ");
}

function agentDisplayName(name: string): string {
  const normalized = name.replace(/\\/g, "/");
  return normalized
    .slice(normalized.lastIndexOf("/") + 1)
    .replace(/^__?agent_/, "");
}

function teamMemberDisplayLabel(agentName: string): string {
  const parts = agentName.replace(/\\/g, "/").split("/");
  const agent = parts[parts.length - 1].replace(/^__?agent_/, "");
  const project = parts[0];
  return project && project !== agent ? `${agent}@${project}` : agent;
}

function sessionStatusSearchText(status: Session["status"]): string {
  return typeof status === "string" ? status : `exited ${status.exited}`;
}

/** Build the gitRepos list for a replica. Order = replica.repoPaths order (invariant §3.1.2). */
function buildGitRepos(replica: AcAgentReplica): SessionRepoInput[] {
  return (replica.repoPaths ?? []).map((p) => {
    return { label: repoLabelFromPath(p), sourcePath: p };
  });
}

/**
 * Build the AgentPicker scope context for a launching WG replica (#384 Frontend §5).
 * Broad-scope assignment is only offered when the launch resolves to a real WG
 * replica; the backend re-enumerates and is authoritative either way.
 */
function replicaScopeContext(wg: AcWorkgroup, replica: AcAgentReplica): AgentPickerScopeContext {
  return {
    workgroupPath: wg.path,
    workgroupName: wg.name,
    targetReplicaPath: replica.path,
    targetReplicaName: replica.name,
    currentCodingAgentId: replica.currentCodingAgentId ?? null,
    currentProfile: replica.currentProfile ?? null,
  };
}

/** Derive scope context from a live replica session for the right-click "Coding
 *  Agent" action. Falls back to a single-path (no broad scope) context when the
 *  session is not a WG replica. */
function deriveScopeContextFromSession(
  session: Session | undefined,
  sessionName: string,
): AgentPickerScopeContext | undefined {
  if (!session) return undefined;
  const replicaPath = session.workingDirectory;
  if (!isWgReplicaPath(replicaPath)) {
    return {
      targetReplicaPath: replicaPath,
      currentCodingAgentId: session.agentId,
      currentProfile: session.requestedProfile,
    };
  }
  // Workgroup dir = parent of the replica dir, preserving the original separators.
  const dirMatch = replicaPath.match(/^(.*)[\\/][^\\/]+[\\/]?$/);
  const slash = sessionName.indexOf("/");
  const wgName = slash >= 0 ? sessionName.slice(0, slash) : "";
  const replicaName = slash >= 0 ? sessionName.slice(slash + 1) : sessionName;
  return {
    workgroupPath: dirMatch?.[1] ?? "",
    workgroupName: wgName,
    targetReplicaPath: replicaPath,
    targetReplicaName: replicaName,
    currentCodingAgentId: session.agentId,
    currentProfile: session.requestedProfile,
  };
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

function runningCoordinatorPeers(wg: AcWorkgroup, replica: AcAgentReplica): AcAgentReplica[] {
  return (wg.agents ?? []).filter((peer) => {
    if (peer.name === replica.name) return false;
    const dot = replicaDotClass(wg, peer);
    return dot === "running" || dot === "active";
  });
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

  // #537: post-assign "Restart now?" prompt. Hoisted to the stable ProjectPanel
  // scope (NOT inside the projects <For>) so a background replica-list refresh,
  // which replaces project object references and so re-creates each <For> row
  // (disposing its local signals), cannot tear the modal down before the user
  // answers. Mirrors the pendingLaunch picker below, hoisted for the same reason.
  // The prompt closes only on Later / Restart now / overlay click.
  const [restartPrompt, setRestartPrompt] = createSignal<{
    sessionId: string;
    replicaName: string;
    agentId: string;
    agentLabel: string;
    requestedProfile: string | null;
  } | null>(null);
  // #573: in-flight + error state for the prompt's restart. The old code did a
  // consume-and-clear (setRestartPrompt(null) before the async settled) with a
  // bare `.catch(console.error)`, so a failed restart vanished silently and the
  // user thought the new agent applied while the old one kept running. We now
  // keep the modal open, surface the failure, and let the user retry — mirroring
  // AgentPickerModal.apply() and Resource Monitor's confirmKill.
  const [restarting, setRestarting] = createSignal(false);
  const [restartError, setRestartError] = createSignal("");

  // Restart the live session on the newly-assigned agent (same SessionAPI.restart
  // the Restart button uses; honors currentCodingAgent, 0b03ad7). The `restarting`
  // guard replaces the old early setRestartPrompt(null) as the double-fire guard:
  // re-entry is refused while a restart is in flight. On success the modal closes;
  // on failure it stays open with the error so the user can retry.
  const applyRestartPrompt = async () => {
    const prompt = restartPrompt();
    if (!prompt || restarting()) return;
    setRestarting(true);
    setRestartError("");
    try {
      await SessionAPI.restart(prompt.sessionId, {
        agentId: prompt.agentId,
        requestedProfile: prompt.requestedProfile,
      });
      setRestartPrompt(null);
    } catch (e) {
      console.error("Failed to restart session:", e);
      setRestartError(launchErrorMessage(e));
    } finally {
      setRestarting(false);
    }
  };

  // Close the prompt, clearing any error. Refused while a restart is in flight so
  // neither the Later button nor an overlay click can tear the modal down before
  // the restart settles (the buttons are also disabled via `busy`).
  const dismissRestartPrompt = () => {
    if (restarting()) return;
    setRestartError("");
    setRestartPrompt(null);
  };

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

    setPendingLaunch({
      path: replica.path,
      sessionName: replicaSessionName(wg, replica),
      gitRepos,
      currentAgentId: replica.currentCodingAgentId ?? replica.preferredAgentId,
      currentRequestedProfile: replica.currentProfile ?? null,
      scopeContext: replicaScopeContext(wg, replica),
    });
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

    setPendingLaunch({
      path: agent.path,
      sessionName: agent.name,
      gitRepos: [],
      currentAgentId: agent.preferredAgentId,
    });
  };

  return (
    <>
    {/* Empty-tree status: visible to the user (deferred Round-1 G11 chip) and
        to UI-automation. `data-ac-state` + `data-ac-detail` make a swallowed
        boot/load failure observable without devtools — loading (stuck) vs
        error (with message) vs empty (did initFromSettings run? how many paths?). */}
    <Show when={projectStore.projects.length === 0}>
      <div
        class="project-load-status"
        data-ac-testid="project.loadStatus"
        data-ac-role="status"
        data-ac-state={
          projectStore.isLoading ? "loading" : projectStore.lastLoadError ? "error" : "empty"
        }
        data-ac-detail={
          `init:${projectStore.initState.attempted ? projectStore.initState.pathCount : "no"}` +
          ` err:${projectStore.lastLoadError ?? "none"}`
        }
      >
        <Show
          when={projectStore.isLoading}
          fallback={
            <Show
              when={projectStore.lastLoadError}
              fallback={<span class="ac-empty-hint">No projects loaded</span>}
            >
              <span class="ac-empty-hint">
                Failed to load project: {projectStore.lastLoadError}
              </span>
            </Show>
          }
        >
          <span class="ac-empty-hint">Loading projects…</span>
        </Show>
      </div>
    </Show>
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
        const [replicaCtxMenu, setReplicaCtxMenu] = createSignal<
          | { kind: "active"; sessionId: string; sessionName: string; wg: AcWorkgroup; exited: boolean; x: number; y: number }
          | { kind: "inactive"; wg: AcWorkgroup; replica: AcAgentReplica; x: number; y: number }
          | null
        >(null);
        const [replicaCodingAgentTarget, setReplicaCodingAgentTarget] = createSignal<{ sessionId: string; sessionName: string } | null>(null);
        // Coding Agent picker target for a gray/red (not-running) replica — #545.
        const [inactiveCodingAgentTarget, setInactiveCodingAgentTarget] = createSignal<{ wg: AcWorkgroup; replica: AcAgentReplica } | null>(null);
        const [deletingWg, setDeletingWg] = createSignal<AcWorkgroup | null>(null);
        const [wgDeleteError, setWgDeleteError] = createSignal("");
        const [wgDeleteInProgress, setWgDeleteInProgress] = createSignal(false);
        const [wgDirtyRepos, setWgDirtyRepos] = createSignal(false);
        const [wgConfirmText, setWgConfirmText] = createSignal("");
        const [wgBlockers, setWgBlockers] = createSignal<BlockerReport | null>(null);
        const [wgRetryInProgress, setWgRetryInProgress] = createSignal(false);
        const [wgLastForceUsed, setWgLastForceUsed] = createSignal(false);
        let retryGen = 0;
        const [filterOpen, setFilterOpen] = createSignal(false);
        const [filterPattern, setFilterPattern] = createSignal("");
        let filterInputEl: HTMLInputElement | undefined;
        const [agentCtxMenu, setAgentCtxMenu] = createSignal<{ agent: { name: string; path: string; preferredAgentId?: string }; x: number; y: number } | null>(null);
        const [agentsHeaderCtxMenu, setAgentsHeaderCtxMenu] = createSignal<{ x: number; y: number } | null>(null);
        const [workgroupsHeaderCtxMenu, setWorkgroupsHeaderCtxMenu] = createSignal<{ x: number; y: number } | null>(null);
        const [loopCtxMenu, setLoopCtxMenu] = createSignal<{ loop: AcLoopSummary; x: number; y: number } | null>(null);
        const [loopsHeaderCtxMenu, setLoopsHeaderCtxMenu] = createSignal<{ x: number; y: number } | null>(null);
        const [loopActionInProgress, setLoopActionInProgress] = createSignal<string | null>(null);
        const [deletingLoop, setDeletingLoop] = createSignal<AcLoopSummary | null>(null);
        const [loopDeleteError, setLoopDeleteError] = createSignal("");
        const currentLoopDeleteInProgress = () => {
          const loop = deletingLoop();
          return !!loop && loopActionInProgress() === `${loop.id}:delete`;
        };
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
        const closeLoopDeleteModal = () => {
          if (currentLoopDeleteInProgress()) return;
          setLoopDeleteError("");
          setDeletingLoop(null);
        };
        createEffect(() => {
          if (!deletingAgent() && !deletingWg() && !deletingTeam() && !deletingLoop()) return;
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
            if (deletingLoop()) {
              closeLoopDeleteModal();
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
        const filterState = createMemo(() => {
          const pattern = filterPattern().trim();
          if (!pattern) return { pattern, regex: null, error: "" };
          try {
            return { pattern, regex: new RegExp(pattern, "i"), error: "" };
          } catch {
            return { pattern, regex: null, error: "Invalid regex" };
          }
        });
        const filterActive = () => filterState().regex !== null;
        const filterError = () => filterState().error;
        const focusFilterInput = () => {
          const focus = () => {
            filterInputEl?.focus();
            filterInputEl?.select();
          };
          if (typeof window.requestAnimationFrame === "function") {
            window.requestAnimationFrame(focus);
            return;
          }
          window.setTimeout(focus, 0);
        };
        const toggleFilter = () => {
          // Magnifier acts as a toggle. Closing also drops any in-flight
          // pattern so we never leave an active-but-hidden filter applied
          // (the input is the only surface that reveals one is running).
          if (filterOpen()) {
            setFilterOpen(false);
            setFilterPattern("");
            return;
          }
          setFilterOpen(true);
          focusFilterInput();
        };
        const clearFilter = () => {
          setFilterPattern("");
          focusFilterInput();
        };
        const handleFilterKeyDown = (e: KeyboardEvent) => {
          if (e.key !== "Escape") return;
          e.stopPropagation();
          if (filterPattern()) {
            setFilterPattern("");
            focusFilterInput();
            return;
          }
          setFilterOpen(false);
        };
        const matchesFilterText = (...parts: Array<string | null | undefined | false>) => {
          const regex = filterState().regex;
          if (!regex) return true;
          return regex.test(joinSearchText(...parts));
        };
        // Search text for the session fields visible on EVERY row that shows this
        // session (shared by replica rows and agent SessionItems): name, agent
        // label, and the status dot's state. Conditionally-rendered badges are
        // contributed by the per-row callers under the gate that matches their
        // render — repo/branch (replicaSearchText / sessionRepoSearchText) and the
        // profile badge — so "what you match == what you see" (#515 bug 1).
        // `shell` is dropped: it is never shown on any row. status/waiting/pending
        // stay — they are the visible status dot's state, so dropping them would
        // hide a row whose dot the user can see.
        const sessionSearchText = (session: Session | undefined) => {
          if (!session) return "";
          return joinSearchText(
            session.name,
            session.agentLabel,
            sessionStatusSearchText(session.status),
            session.waitingForInput ? "waiting" : null,
            session.pendingReview ? "pending" : null
          );
        };
        // Repo/branch chips on an agent SessionItem render only for a coordinator
        // session with repos (SessionItem gate `isCoordinator && !inactive &&
        // gitRepos.length`). Gate the search text identically so a non-coordinator
        // agent row is never surfaced by a branch it isn't showing (#515 bug 1).
        const sessionRepoSearchText = (session: Session | undefined) => {
          if (!session || !session.isCoordinator || session.id.startsWith("inactive-")) return "";
          return session.gitRepos.map((repo) => formatReplicaRepoBadgeLabel(repo)).join(" ");
        };
        const liveAgentLabel = (session: Session | undefined) => {
          if (!session) return null;
          if (session.agentLabel) return session.agentLabel;
          if (!session.agentId) return null;
          return settingsStore.current?.agents?.find((a) => a.id === session.agentId)?.label ?? null;
        };
        const replicaSearchText = (
          replica: AcAgentReplica,
          wg: AcWorkgroup,
          extraBadge?: string,
          taskTitle?: string | null
        ) => {
          const session = replicaSession(wg, replica);
          const repos = session && session.gitRepos.length > 0
            ? session.gitRepos
            : configuredReplicaRepoBadges(replica, wg);
          return joinSearchText(
            taskTitle,
            stripFrontmatter(taskTitle ?? ""),
            replica.originProject ? `${replica.name}@${replica.originProject}` : replica.name,
            // Repo/branch badges render only for coordinators (renderReplicaItem
            // gate `isCoord() && repoBadges().length>0`). Match the same gate so a
            // non-coordinator row is never surfaced by a badge it isn't showing
            // (#515 bug 1).
            replica.isCoordinator
              ? repos.map((repo) => formatReplicaRepoBadgeLabel(repo)).join(" ")
              : null,
            liveAgentLabel(session),
            // A replica row renders the profile badge unconditionally whenever the
            // session has one (renderReplicaItem) → always matchable here (#515 bug 2).
            session ? sessionProfileBadge(session) : null,
            replica.isCoordinator ? "coordinator" : null,
            extraBadge,
            sessionSearchText(session)
          );
        };
        const replicaMatches = (
          replica: AcAgentReplica,
          wg: AcWorkgroup,
          extraBadge?: string,
          taskTitle?: string | null
        ) => matchesFilterText(replicaSearchText(replica, wg, extraBadge, taskTitle));
        const workgroupOwnMatches = (wg: AcWorkgroup, sectionLabel?: string) =>
          matchesFilterText(sectionLabel, wg.name, wg.taskTitle, stripFrontmatter(wg.taskTitle ?? ""));
        const workgroupMatches = (wg: AcWorkgroup, sectionLabel?: string) =>
          workgroupOwnMatches(wg, sectionLabel) ||
          wg.agents.some((replica) => replicaMatches(replica, wg));
        const filteredReplicasForWorkgroup = (wg: AcWorkgroup, rowContext: string) => {
          const sectionLabel = rowContext === "selected"
            ? "Selected Workgroup"
            : rowContext === "workgroups"
              ? "Workgroups"
              : undefined;
          if (!filterActive() || matchesFilterText(sectionLabel) || workgroupOwnMatches(wg, sectionLabel)) {
            return wg.agents;
          }
          return wg.agents.filter((replica) => replicaMatches(replica, wg));
        };
        const agentMatches = (agent: { name: string; path: string; preferredAgentId?: string }) => {
          const session = sessionsStore.findSessionByName(agent.name);
          const repoText = sessionRepoSearchText(session);
          // The coding-agent badge shows the RESOLVED label (session.agentLabel,
          // else the agentId→settings lookup). sessionSearchText only carries the
          // raw agentLabel, so a session with a null agentLabel but a resolvable
          // agentId would render the badge yet stay unmatchable. Include the
          // resolved label here so the coding-agent badge is matchable wherever it
          // renders — the headline #515 ask (filter agents by their coding agent).
          // It is null exactly when no label resolves, i.e. when the badge is also
          // hidden, so this never matches a badge that isn't shown.
          const codingAgentLabel = liveAgentLabel(session);
          // On an agent SessionItem the profile badge lives inside the meta block,
          // which renders only when an agent label resolves OR a coordinator's
          // repos show (SessionItem outer <Show>). Mirror that gate so the filter
          // never surfaces an agent by a profile badge it isn't showing (#515 bug
          // 1) — unlike replica rows, which render it unconditionally.
          const metaVisible = !!codingAgentLabel || repoText !== "";
          return matchesFilterText(
            agentDisplayName(agent.name),
            sessionSearchText(session),
            codingAgentLabel,
            repoText,
            metaVisible && session ? sessionProfileBadge(session) : null
          );
        };
        const teamMemberMatches = (team: AcTeam, agentName: string) =>
          matchesFilterText(teamMemberDisplayLabel(agentName), agentName === team.coordinator ? "coordinator" : null);
        const teamOwnMatches = (team: AcTeam) => matchesFilterText(team.name);
        const teamMatches = (team: AcTeam) =>
          teamOwnMatches(team) || team.agents.some((agentName) => teamMemberMatches(team, agentName));
        const filteredTeamMembers = (team: AcTeam) => {
          if (!filterActive() || matchesFilterText("Teams") || teamOwnMatches(team)) return team.agents;
          return team.agents.filter((agentName) => teamMemberMatches(team, agentName));
        };
        // Memoized so each section reads one cached pass per (filter, store)
        // change instead of rebuilding search text + re-running regex.test on
        // every access (these are read 3-4× per render). Dependencies are all
        // reactive (filterPattern signal, sessionsStore, projectStore via proj),
        // so sections still update live as the user types (#515 bug 3).
        // NOTE: filteredCoordinatorItems is defined further down, right after the
        // coordinatorItems memo it reads — createMemo runs eagerly, so it must
        // follow that declaration (not lead it) to avoid a temporal dead zone.
        const filteredWorkgroups = createMemo(() => {
          if (!filterActive() || matchesFilterText("Workgroups")) return proj.workgroups;
          return proj.workgroups.filter((wg) => workgroupMatches(wg, "Workgroups"));
        });
        const filteredAgents = createMemo(() => {
          if (!filterActive() || matchesFilterText("Agents")) return proj.agents;
          return proj.agents.filter((agent) => agentMatches(agent));
        });
        const filteredTeams = createMemo(() => {
          if (!filterActive() || matchesFilterText("Teams")) return proj.teams;
          return proj.teams.filter((team) => teamMatches(team));
        });
        const selectedWorkgroupVisible = () => {
          const wg = selectedWorkgroup();
          return !filterActive() || matchesFilterText("Selected Workgroup") || (wg ? workgroupMatches(wg, "Selected Workgroup") : false);
        };
        // Status line shown on each loop row — shared with the filter search
        // text so what the regex matches always equals what the user sees.
        const loopStatusText = (loop: AcLoopSummary) =>
          loop.lastResult?.message ?? (loop.nextDueAt ? `Next: ${new Date(loop.nextDueAt).toLocaleString()}` : "No runs yet");
        const loopSearchText = (loop: AcLoopSummary) =>
          // promptPreview is intentionally NOT matched: it renders only as the
          // row's hover `title` (not visible text), so matching it would surface a
          // loop with no on-screen reason — the case the comment above warned
          // about (#515 bug 1).
          joinSearchText(
            loop.name,
            loop.workgroup,
            loop.expr,
            loopStatusText(loop),
            loop.enabled ? null : "disabled",
            loop.pendingDueAt ? "pending" : null
          );
        const loopMatches = (loop: AcLoopSummary) => matchesFilterText(loopSearchText(loop));
        const filteredLoops = createMemo(() => {
          if (!filterActive() || matchesFilterText("Loops")) return proj.loops;
          return proj.loops.filter((loop) => loopMatches(loop));
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

        const restartReplicaSession = async (
          sessionId: string,
          agentId?: string,
          requestedProfile?: string | null,
        ) => {
          setReplicaCtxMenu(null);
          cleanupCtx();
          try {
            await SessionAPI.restart(
              sessionId,
              agentId ? { agentId, requestedProfile } : undefined,
            );
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

        // Narrow the replica context-menu union for the active vs gray/red
        // (#545) render branches.
        const activeReplicaMenu = () => {
          const m = replicaCtxMenu();
          return m && m.kind === "active" ? m : null;
        };
        const inactiveReplicaMenu = () => {
          const m = replicaCtxMenu();
          return m && m.kind === "inactive" ? m : null;
        };

        // Resolve any real (non-placeholder) session under this workgroup. Every
        // replica in a workgroup shares one TASK.md and task_clean resolves it
        // from the session cwd, so any live/exited sibling session clears the same
        // title. This lets a never-launched (gray) replica reuse the existing
        // session-id-based broom with no backend change (#545).
        const resolveWorkgroupSessionId = (wg: AcWorkgroup): string | null => {
          for (const peer of wg.agents) {
            const s = replicaSession(wg, peer);
            if (s && !s.id.startsWith("inactive-")) return s.id;
          }
          return null;
        };

        // Broom (clear task title) for a gray/red replica — reuses the
        // active-agent TaskAPI.clean. The backend emits workgroup_task_updated,
        // which sidebar/App.tsx applies to projectStore, refreshing the sidebar.
        const clearReplicaTaskTitle = async (wg: AcWorkgroup) => {
          setReplicaCtxMenu(null);
          cleanupCtx();
          const sessionId = resolveWorkgroupSessionId(wg);
          try {
            if (sessionId) {
              await TaskAPI.clean(sessionId);
            } else {
              // Cold workgroup: no live/exited session resolves the root, so
              // address the wg-* root directly (#545). wg.path is always present
              // (types.ts:431), even for a never-launched workgroup.
              await TaskAPI.cleanAt(wg.path);
            }
          } catch (e) {
            console.error("Failed to clear task title:", e);
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

        // Memoized like the other filtered collections (#515 bug 3); placed here
        // because createMemo is eager and this reads the coordinatorItems memo
        // defined just above.
        const filteredCoordinatorItems = createMemo(() => {
          const items = coordinatorItems();
          if (!filterActive()) return items;
          return items.filter((item) =>
            workgroupOwnMatches(item.wg) ||
            replicaMatches(item.replica, item.wg, item.wg.name, item.wg.taskTitle) ||
            matchesFilterText(runningCoordinatorPeers(item.wg, item.replica).map((peer) => `${peer.name} RUNNING`).join(" "))
          );
        });

        const runLoopAction = async (
          loop: AcLoopSummary,
          action: "run" | "toggle" | "delete",
          task: () => Promise<AcLoopSummary | null>
        ) => {
          const actionKey = `${loop.id}:${action}`;
          if (loopActionInProgress()) return;
          setLoopActionInProgress(actionKey);
          try {
            const updatedLoop = await task();
            if (updatedLoop) {
              projectStore.upsertLoop(proj.path, updatedLoop);
            }
            await projectStore.reloadProject(proj.path);
          } catch (e) {
            console.error(`Loop ${action} failed:`, e);
          } finally {
            setLoopActionInProgress(null);
          }
        };

        const deleteLoop = async (loop: AcLoopSummary) => {
          const actionKey = `${loop.id}:delete`;
          if (loopActionInProgress()) return;
          setLoopActionInProgress(actionKey);
          setLoopDeleteError("");
          try {
            await LoopAPI.delete(proj.path, loop.id);
            projectStore.removeLoop(proj.path, loop.id);
            await projectStore.reloadProject(proj.path);
            setDeletingLoop(null);
          } catch (e: unknown) {
            console.error("delete_loop failed:", e);
            setLoopDeleteError(typeof e === "string" ? e : e instanceof Error ? e.message : "Failed to delete Loop");
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

        const handleReplicaContextMenu = (e: MouseEvent, session: Session, wg: AcWorkgroup) => {
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
            kind: "active",
            sessionId: session.id,
            sessionName: session.name,
            wg,
            // Red/exited replicas get the full menu PLUS the broom; green/live
            // gets the full menu with no broom (#545 rework).
            exited: !isSessionLive(session),
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

        // Gray (never launched) replicas have no session at all, so the active
        // menu's session-id path can't open. Show the minimal not-running menu
        // instead: Coding Agent selector + broom. Red/exited replicas keep a real
        // session and route to the full menu + broom instead (#545 rework).
        const handleReplicaInactiveContextMenu = (e: MouseEvent, wg: AcWorkgroup, replica: AcAgentReplica) => {
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
          setReplicaCtxMenu({ kind: "inactive", wg, replica, x: e.clientX, y: e.clientY });
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
          const profileBadge = () => {
            const s = session();
            return s ? sessionProfileBadge(s) : null;
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
                // Any real session (green/live OR red/exited) gets the full menu;
                // red additionally shows the broom. Only gray (no session) falls
                // into the minimal Coding Agent + broom menu (#545 rework).
                if (s) {
                  handleReplicaContextMenu(e, s, wg);
                } else {
                  handleReplicaInactiveContextMenu(e, wg, replica);
                }
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
                  <Show when={profileBadge()}>
                    {(badge) => <span class="profile-badge">{badge()}</span>}
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
                <For each={filteredReplicasForWorkgroup(wg, rowContext)}>
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
            <div
              class="project-filter-row"
              classList={{ open: filterOpen(), active: filterActive(), invalid: !!filterError() }}
              data-ac-testid="project.regexFilter.row"
            >
              <div class="project-filter-field" classList={{ open: filterOpen() }}>
                <input
                  ref={filterInputEl}
                  class="project-filter-input"
                  value={filterPattern()}
                  placeholder="wg-2.*"
                  aria-label="Sidebar regex filter"
                  aria-invalid={!!filterError()}
                  data-ac-testid="project.regexFilter.input"
                  onInput={(e) => setFilterPattern(e.currentTarget.value)}
                  onKeyDown={handleFilterKeyDown}
                />
                <Show when={filterPattern()}>
                  <button
                    type="button"
                    class="project-filter-clear"
                    title="Clear regex filter"
                    aria-label="Clear regex filter"
                    data-ac-testid="project.regexFilter.clear"
                    onClick={clearFilter}
                  >
                    &#x2715;
                  </button>
                </Show>
              </div>
              <button
                type="button"
                class="project-filter-toggle"
                title={filterOpen() ? "Hide regex filter" : "Filter sidebar (regex)"}
                aria-label={filterOpen() ? "Hide regex filter" : "Filter sidebar (regex)"}
                aria-expanded={filterOpen()}
                data-ac-testid="project.regexFilter.toggle"
                onClick={toggleFilter}
              >
                <svg viewBox="0 0 24 24" aria-hidden="true">
                  <circle cx="11" cy="11" r="7" />
                  <path d="m20 20-4.2-4.2" />
                </svg>
              </button>
            </div>
            <Show when={filterError()}>
              {(error) => (
                <div class="project-filter-error" role="alert" data-ac-testid="project.regexFilter.error">
                  {error()}
                </div>
              )}
            </Show>

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
                    <Show when={filteredCoordinatorItems().length > 0}>
                      <div class="coord-quick-access">
                        <For each={filteredCoordinatorItems()}>
                          {(item) => {
                            const runningPeers = createMemo(() =>
                              runningCoordinatorPeers(item.wg, item.replica)
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
                    <Show when={(sessionsStore.showCategories || sessionsStore.alwaysShowSelectedWorkgroup) && selectedWorkgroupVisible()}>
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
                    <Show when={sessionsStore.showCategories && (!filterActive() || matchesFilterText("Workgroups") || filteredWorkgroups().length > 0)}>
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
                        <span class="ac-team-count">{filteredWorkgroups().length}</span>
                      </div>
                      <Show when={!wgsCollapsed()}>
                        <Show
                          when={filteredWorkgroups().length > 0}
                          fallback={<div class="ac-empty-hint">No workgroups</div>}
                        >
                          <For each={filteredWorkgroups()}>
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
                    <Show when={sessionsStore.showCategories && (!filterActive() || matchesFilterText("Loops") || filteredLoops().length > 0)}>
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
                        <span class="ac-team-count">{filteredLoops().length}</span>
                      </div>
                      <Show when={!loopsCollapsed()}>
                        <Show
                          when={filteredLoops().length > 0}
                          fallback={<div class="ac-empty-hint">No loops</div>}
                        >
                          <For each={filteredLoops()}>
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
                                  {loopStatusText(loop)}
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
                    <Show when={sessionsStore.showCategories && (!filterActive() || matchesFilterText("Agents") || filteredAgents().length > 0)}>
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
                          when={filteredAgents().length > 0}
                          fallback={<div class="ac-empty-hint">No agents</div>}
                        >
                          <For each={filteredAgents()}>
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
                    <Show when={sessionsStore.showCategories && (!filterActive() || matchesFilterText("Teams") || filteredTeams().length > 0)}>
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
                          when={filteredTeams().length > 0}
                          fallback={<div class="ac-empty-hint">No teams</div>}
                        >
                          <For each={filteredTeams()}>
                            {(team) => {
                              const [teamExpanded, setTeamExpanded] = createSignal(false);
                              const visibleTeamMembers = () => filteredTeamMembers(team);
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
                                    <span class="ac-team-count">{visibleTeamMembers().length}</span>
                                  </div>
                                  <Show when={teamExpanded()}>
                                    <div class="ac-team-members">
                                      <For each={visibleTeamMembers()}>
                                        {(agentName) => {
                                          return (
                                            <div class="ac-team-member" title={agentName}>
                                              <span class="ac-team-member-name">{teamMemberDisplayLabel(agentName)}</span>
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
                        const details = await LoopAPI.runNow(proj.path, menu.loop.id);
                        return details.summary;
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
                        const details = await LoopAPI.setEnabled(proj.path, menu.loop.id, !menu.loop.enabled);
                        return details.summary;
                      });
                    }}
                    data-ac-testid={`loop.action.toggle.${projectAutomationId()}.${automationIdPart(loopCtxMenu()!.loop.id)}`}
                  >
                    {loopCtxMenu()!.loop.enabled ? "Disable" : "Enable"}
                  </button>
                  <div class="context-separator" />
                  <button
                    class="session-context-option context-option-danger"
                    disabled={!!loopActionInProgress()}
                    onClick={() => {
                      const menu = loopCtxMenu();
                      setLoopCtxMenu(null);
                      cleanupCtx();
                      if (!menu || loopActionInProgress()) return;
                      setLoopDeleteError("");
                      setDeletingLoop(menu.loop);
                    }}
                    data-ac-testid={`loop.action.delete.${projectAutomationId()}.${automationIdPart(loopCtxMenu()!.loop.id)}`}
                  >
                    Delete Loop
                  </button>
                </div>
              </Portal>
            )}

            {/* Delete Loop confirmation */}
            {deletingLoop() && (
              <Portal>
                <div class="modal-overlay">
                  <div
                    class="agent-modal"
                    style={{ "max-width": "360px" }}
                    data-ac-testid={`loop.delete.dialog.${projectAutomationId()}.${automationIdPart(deletingLoop()!.id)}`}
                  >
                    <div class="agent-modal-header">
                      <span class="agent-modal-title">Delete Loop</span>
                    </div>
                    <div class="new-agent-form">
                      <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
                        Delete Loop <strong>{deletingLoop()!.name}</strong>? This will remove the Loop configuration and scheduled delivery state. This action cannot be undone.
                      </p>
                      <Show when={loopDeleteError()}>
                        <div class="new-agent-error">{loopDeleteError()}</div>
                      </Show>
                    </div>
                    <div class="new-agent-footer">
                      <button
                        class="new-agent-cancel-btn"
                        onClick={closeLoopDeleteModal}
                        disabled={currentLoopDeleteInProgress()}
                        data-ac-testid={`loop.delete.cancel.${projectAutomationId()}.${automationIdPart(deletingLoop()!.id)}`}
                      >
                        Cancel
                      </button>
                      <button
                        class="new-agent-create-btn"
                        style={{ "background": "var(--danger, #c0392b)" }}
                        disabled={!!loopActionInProgress()}
                        onClick={() => {
                          const loop = deletingLoop();
                          if (!loop || loopActionInProgress()) return;
                          void deleteLoop(loop);
                        }}
                        data-ac-testid={`loop.delete.confirm.${projectAutomationId()}.${automationIdPart(deletingLoop()!.id)}`}
                      >
                        {currentLoopDeleteInProgress() ? "Deleting..." : "Delete"}
                      </button>
                    </div>
                  </div>
                </div>
              </Portal>
            )}

            {/* Replica context menu — full menu for green/live AND red/exited
                (red adds the broom); gray (no session) gets Coding Agent + broom (#545) */}
            {replicaCtxMenu() && (
              <Portal>
                <div
                  class="session-context-menu"
                  ref={replicaCtxMenuEl}
                  style={{ left: `${replicaCtxMenu()!.x}px`, top: `${replicaCtxMenu()!.y}px` }}
                  onClick={(e) => e.stopPropagation()}
                >
                  <Show when={activeReplicaMenu()}>
                    {(menu) => {
                      // Broom renders in EVERY replica dot state (#545). Disabled
                      // only when the task title has nothing to clear (empty/missing
                      // or the literal "Clean" sentinel); enabled otherwise.
                      const broomDisabled = () => isTaskClean(menu().wg.taskTitle);
                      const broomTitle = () =>
                        broomDisabled() ? "Nothing to clear" : "Clear task title";
                      return (
                      <>
                        <button
                          class="session-context-option context-option-danger"
                          onClick={async () => {
                            await restartReplicaSession(menu().sessionId);
                          }}
                        >
                          Restart Session
                        </button>
                        <button
                          class="session-context-option"
                          onClick={() => {
                            const sessionId = menu().sessionId;
                            const sessionName = menu().sessionName;
                            setReplicaCtxMenu(null);
                            cleanupCtx();
                            setReplicaCodingAgentTarget({ sessionId, sessionName });
                          }}
                        >
                          Coding Agent
                        </button>
                        <div class="context-separator" />
                        <button
                          class="session-context-option"
                          onClick={() => toggleReplicaDetach(menu().sessionId)}
                        >
                          {sessionsStore.isDetached(menu().sessionId) ? "Re-attach to main" : "Open in new window"}
                        </button>
                        <div class="context-separator" />
                        <button
                          class="session-context-option"
                          classList={{ "context-option-disabled": broomDisabled() }}
                          disabled={broomDisabled()}
                          title={broomTitle()}
                          onClick={() => void clearReplicaTaskTitle(menu().wg)}
                        >
                          &#x1F9F9; Clear task title
                        </button>
                      </>
                      );
                    }}
                  </Show>
                  <Show when={inactiveReplicaMenu()}>
                    {(menu) => {
                      const broomDisabled = () => isTaskClean(menu().wg.taskTitle);
                      const broomTitle = () =>
                        broomDisabled() ? "Nothing to clear" : "Clear task title";
                      return (
                        <>
                          <button
                            class="session-context-option"
                            onClick={() => {
                              const wg = menu().wg;
                              const replica = menu().replica;
                              setReplicaCtxMenu(null);
                              cleanupCtx();
                              setInactiveCodingAgentTarget({ wg, replica });
                            }}
                          >
                            Coding Agent
                          </button>
                          <button
                            class="session-context-option"
                            classList={{ "context-option-disabled": broomDisabled() }}
                            disabled={broomDisabled()}
                            title={broomTitle()}
                            onClick={() => void clearReplicaTaskTitle(menu().wg)}
                          >
                            &#x1F9F9; Clear task title
                          </button>
                        </>
                      );
                    }}
                  </Show>
                </div>
              </Portal>
            )}
            {replicaCodingAgentTarget() && (
              <Portal>
                <AgentPickerModal
                  sessionName={replicaCodingAgentTarget()!.sessionName}
                  agentPath={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.workingDirectory}
                  currentAgentId={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.agentId}
                  currentRequestedProfile={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.requestedProfile}
                  scopeContext={deriveScopeContextFromSession(
                    sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId),
                    replicaCodingAgentTarget()!.sessionName,
                  )}
                  onSelect={async (selection) => {
                    // The picker already persisted the selection through the backend
                    // (config write) for WG replicas. For a non-WG agent session there
                    // is no backend persist path, so apply the change by restarting with
                    // the chosen agent/profile.
                    const target = replicaCodingAgentTarget();
                    setReplicaCodingAgentTarget(null);
                    if (!target) return;
                    const session = sessionsStore.sessions.find((s) => s.id === target.sessionId);
                    if (!isWgReplicaPath(session?.workingDirectory)) {
                      await restartReplicaSession(
                        target.sessionId,
                        selection.agent.id,
                        selection.requestedProfile,
                      );
                      return;
                    }
                    // #537: WG replica was persisted but the live session still runs the
                    // old agent. Offer an immediate restart when there is a live session.
                    if (shouldOfferRestartAfterAssign(selection, session)) {
                      const slash = target.sessionName.lastIndexOf("/");
                      // #573: clear any error left over from a prior failed restart
                      // so a fresh prompt never opens showing a stale message. (No
                      // need to reset `restarting`: dismiss is blocked while it is
                      // true, so the picker can't reopen to reach here mid-flight.)
                      setRestartError("");
                      setRestartPrompt({
                        sessionId: target.sessionId,
                        replicaName: slash >= 0 ? target.sessionName.slice(slash + 1) : target.sessionName,
                        agentId: selection.agent.id,
                        agentLabel: selection.agent.label,
                        requestedProfile: selection.requestedProfile,
                      });
                    }
                  }}
                  onClose={() => setReplicaCodingAgentTarget(null)}
                />
              </Portal>
            )}
            {/* Coding Agent picker for a gray/red replica — pick what launches
                before first launch / relaunch, without starting the agent (#545).
                For a WG replica the picker writes the selection through the backend. */}
            <Show when={inactiveCodingAgentTarget()}>
              {(target) => (
                <Portal>
                  <AgentPickerModal
                    sessionName={replicaSessionName(target().wg, target().replica)}
                    agentPath={target().replica.path}
                    currentAgentId={target().replica.currentCodingAgentId ?? target().replica.preferredAgentId}
                    currentRequestedProfile={target().replica.currentProfile ?? null}
                    scopeContext={replicaScopeContext(target().wg, target().replica)}
                    onSelect={async () => {
                      // WG replica: the picker already wrote the coding-agent
                      // selection via the backend (no restart — the agent isn't
                      // running). Close first, then reload so the chosen agent
                      // shows and is pre-selected at first launch.
                      setInactiveCodingAgentTarget(null);
                      await projectStore.reloadProject(proj.path);
                    }}
                    onClose={() => setInactiveCodingAgentTarget(null)}
                  />
                </Portal>
              )}
            </Show>

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
          agentPath={pendingLaunch()!.path}
          currentAgentId={pendingLaunch()!.currentAgentId}
          currentRequestedProfile={pendingLaunch()!.currentRequestedProfile}
          scopeContext={pendingLaunch()!.scopeContext}
          onSelect={async (selection) => {
            const pending = pendingLaunch()!;
            const newSession = await SessionAPI.create({
              cwd: pending.path,
              sessionName: pending.sessionName,
              agentId: selection.agent.id,
              requestedProfile: selection.requestedProfile,
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

    {/* #537: post-assign "Restart now?" prompt, rendered here at the stable
        ProjectPanel root (outside the projects <For>) so a background discovery
        refresh that replaces project references, re-creating each <For> row,
        cannot unmount it mid-decision. It closes only on Later / Restart now /
        overlay click. */}
    {restartPrompt() && (
      <Portal>
        <RestartPromptModal
          agentLabel={restartPrompt()!.agentLabel}
          replicaName={restartPrompt()!.replicaName}
          error={restartError()}
          busy={restarting()}
          onRestart={applyRestartPrompt}
          onLater={dismissRestartPrompt}
        />
      </Portal>
    )}
    </>
  );
};

export default ProjectPanel;
