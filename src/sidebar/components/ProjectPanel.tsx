import { Component, For, Show, createEffect, createMemo, createSignal, on, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcWorkgroup, AcAgentReplica, AcTeam, AcLoopSummary, Session, SessionRepo, TelegramBotConfig, BlockerReport } from "../../shared/types";
import { SessionAPI, WindowAPI, EntityAPI, LoopAPI, TelegramAPI, SettingsAPI, TaskAPI, ReposAPI, onDiscoveryBranchUpdated, onCoordinatorClockUpdated, onCoordinatorAutoCloseChanged, onCoordinatorManualCloseChanged } from "../../shared/ipc";
import type { SessionRepoInput } from "../../shared/ipc";
import {
  pendingCoordinatorClose,
  setPendingCoordinatorClose,
  confirmPendingCoordinatorClose,
  requestCoordinatorClose,
  registerCoordinatorCloseModalHost,
} from "../stores/coordinator-close";
import { isTauri } from "../../shared/platform";
import {
  githubBranchUrl,
  githubRepoUrl,
  parseGithubRemote,
  type GithubRepoRef,
} from "../../shared/github-url";
import { stripFrontmatter } from "../../shared/markdown";
import { launchErrorMessage } from "../../shared/launch-errors";
import { focusOnMount } from "../../shared/focus-on-mount";
import RaiseHandIcon from "./RaiseHandIcon";
import { projectStore } from "../stores/project";
import {
  effectiveAutoClosedAt,
  effectiveLastUserMessageAt,
  effectiveManuallyClosedAt,
  effectiveRepoBranch,
  effectiveRepoBranchByPath,
  effectiveRepoDirtyByPath,
  replicaVolatileStore,
} from "../stores/replica-volatile";
import { normalizeProjectPathForCompare } from "../stores/project-refresh";
import {
  projectCollapseStore,
  projectPanelCollapseKey,
  PROJECT_PANEL_COLLAPSE_KEY_SEP,
} from "../stores/project-collapse";
export {
  PROJECT_PANEL_COLLAPSE_KEY_SEP,
  projectPanelCollapseKey,
  type ProjectPanelCollapseSection,
} from "../stores/project-collapse";
import { sessionsStore } from "../stores/sessions";
import { bridgesStore } from "../stores/bridges";
import { settingsStore } from "../../shared/stores/settings";
import { toastStore } from "../../shared/stores/toasts";
import { voiceRecorder } from "../../shared/voice-recorder";
import { isWgReplicaPath, profileDisplayLabel, sessionProfileBadge, shouldOfferRestartAfterAssign } from "../../shared/profile-utils";
import { clockStore } from "../stores/clock";
import { coordinatorIdleBadge } from "../../shared/coordinator-badge";
import { COORD_IDLE_CLASS } from "./coordinator-badge-class";
import SessionItem from "./SessionItem";
import ProfileOutdatedBadge from "./ProfileOutdatedBadge";
import ContextBadge from "./ContextBadge";
import { contextBadgeConfigured } from "./session-context";
import NewEntityAgentModal from "./NewEntityAgentModal";
import NewTeamModal from "./NewTeamModal";
import NewWorkgroupModal from "./NewWorkgroupModal";
import NewLoopModal from "./NewLoopModal";
import EditLoopModal from "./EditLoopModal";
import AgentPickerModal, { type AgentPickerScopeContext } from "./AgentPickerModal";
import RestartPromptModal from "./RestartPromptModal";
import EditTeamModal from "./EditTeamModal";
import { TelegramIcon } from "./TelegramIcon";
import DetachIcon from "./DetachIcon";
import ReattachIcon from "./ReattachIcon";
import { normalizeBlockerReport } from "./workgroup-delete-diagnostics";
import {
  automationIdPart,
  configuredReplicaRepoBadges,
  formatReplicaRepoBadgeLabel,
  formatReplicaRepoBadgeTitle,
  repoLabelFromPath,
} from "./replica-repo-badges";
import { sessionDotClass } from "./session-status";
import { replicaDotClass } from "./replica-dot";
import {
  findReplicaSession as replicaSession,
  isReplicaWorking,
  replicaSessionName,
} from "./workgroup-session";
import {
  MAX_GROUP_MATCH_ID_LENGTH,
  DEFAULT_NON_STOP_NAME,
  compileGroupRegex,
  groupMatchId,
  nonStopMatchesWorkgroup,
  removeExactGroupToken,
  workgroupGroupsStore,
} from "../stores/workgroup-groups";

interface PendingLaunch {
  path: string;
  sessionName: string;
  gitRepos: SessionRepoInput[];
  currentAgentId?: string;
  currentRequestedProfile?: string | null;
  scopeContext?: AgentPickerScopeContext;
  resumeOnLaunch?: boolean;
}

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

function sessionEffectiveStatusSearchText(session: Session): string {
  const dotClass = sessionDotClass(session);
  return dotClass === "exited" ? sessionStatusSearchText(session.status) : dotClass;
}

function buildGitRepos(replica: AcAgentReplica): SessionRepoInput[] {
  return (replica.repoPaths ?? []).map((p) => {
    return { label: repoLabelFromPath(p), sourcePath: p };
  });
}

function hasValidRepoSourcePath(repo: Pick<SessionRepo, "sourcePath">): boolean {
  return typeof repo.sourcePath === "string" && repo.sourcePath.trim().length > 0;
}

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
const CONTEXT_MENU_CLOSE_GRACE_MS = 250;

function workgroupCollapseId(wg: AcWorkgroup, rowContext: string): string {
  return [
    rowContext,
    normalizeProjectPathForCompare(wg.path || wg.name),
  ].join(PROJECT_PANEL_COLLAPSE_KEY_SEP);
}

export const RESTART_TIMEOUT_MS = 30_000;

function configuredReplicaRepoBadgesLive(
  replica: AcAgentReplica,
  workgroup: Pick<AcWorkgroup, "repoPath">
): SessionRepo[] {
  return configuredReplicaRepoBadges(
    {
      repoPaths: replica.repoPaths,
      repoBranch: effectiveRepoBranch(replica),
      repoBranchByPath: effectiveRepoBranchByPath(replica),
      repoDirtyByPath: effectiveRepoDirtyByPath(replica),
    },
    workgroup
  );
}

function replicaRepoMenuEntries(wg: AcWorkgroup, replica: AcAgentReplica): SessionRepo[] {
  if (!replica.isCoordinator) return [];
  const session = replicaSession(wg, replica);
  const repos = session && session.gitRepos.length > 0
    ? session.gitRepos
    : configuredReplicaRepoBadgesLive(replica, wg);
  return repos.filter(hasValidRepoSourcePath);
}

function isSessionLive(session: Session | undefined): boolean {
  if (!session) return false;
  if (typeof session.status === "object" && "exited" in session.status) return false;
  return true;
}

function pathSeparatorFor(path: string): "\\" | "/" {
  return path.includes("\\") ? "\\" : "/";
}

function trimTrailingPathSeparators(path: string): string {
  return path.replace(/[\\/]+$/, "") || path;
}

function isAbsolutePath(path: string): boolean {
  return /^[A-Za-z]:[\\/]/.test(path) || /^[\\/]{2}[^\\/]+[\\/][^\\/]+/.test(path) || /^[\\/]/.test(path);
}

function normalizePath(path: string, separator: "\\" | "/"): string {
  const trimmed = path.trim();
  const driveMatch = trimmed.match(/^([A-Za-z]:)[\\/]+(.*)$/);
  const uncMatch = driveMatch ? null : trimmed.match(/^[\\/]{2}([^\\/]+)[\\/]([^\\/]+)[\\/]?(.*)$/);
  let root = "";
  let rest = trimmed;

  if (driveMatch) {
    root = `${driveMatch[1]}${separator}`;
    rest = driveMatch[2];
  } else if (uncMatch) {
    root = `${separator}${separator}${uncMatch[1]}${separator}${uncMatch[2]}${separator}`;
    rest = uncMatch[3];
  } else if (/^[\\/]/.test(trimmed)) {
    root = separator;
    rest = trimmed.replace(/^[\\/]+/, "");
  }

  const segments: string[] = [];
  for (const segment of rest.split(/[\\/]+/)) {
    if (!segment || segment === ".") continue;
    if (segment === "..") {
      if (segments.length > 0 && segments[segments.length - 1] !== "..") {
        segments.pop();
      } else if (!root) {
        segments.push(segment);
      }
      continue;
    }
    segments.push(segment);
  }

  const suffix = segments.join(separator);
  if (!root) return suffix || ".";
  return suffix ? `${root}${suffix}` : root;
}

function matrixFolderFromIdentityPath(replicaPath: string, identityPath: string | undefined): string | null {
  const path = identityPath?.trim();
  if (!path) return null;

  const separator = pathSeparatorFor(replicaPath || path);
  const identityTarget = path.match(/(^|[\\/])identity\.json$/i)
    ? path.replace(/[\\/][^\\/]+$/, "")
    : path;
  const absoluteTarget = isAbsolutePath(identityTarget)
    ? identityTarget
    : `${trimTrailingPathSeparators(replicaPath)}${separator}${identityTarget}`;

  return normalizePath(absoluteTarget, separator);
}

function replicaMatrixFolder(replica: AcAgentReplica): string | null {
  return matrixFolderFromIdentityPath(replica.path, replica.identityPath);
}

function MatrixFolderIcon() {
  return (
    <svg
      class="session-context-matrix-icon"
      viewBox="0 0 16 16"
      aria-hidden="true"
    >
      <path
        fill="currentColor"
        d="M1.75 4.25A1.75 1.75 0 0 1 3.5 2.5h3.1c.46 0 .9.18 1.22.5l.9.9h3.78A1.75 1.75 0 0 1 14.25 5.65v5.1a1.75 1.75 0 0 1-1.75 1.75h-9A1.75 1.75 0 0 1 1.75 10.75v-6.5Z"
      />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg class="session-context-trash-icon" viewBox="0 0 16 16" aria-hidden="true">
      <path
        fill="currentColor"
        d="M5.25 2.5h5.5l.5 1.25h2v1.5H2.75v-1.5h2l.5-1.25Zm.75 4h1.25v5H6v-5Zm2.375 0h1.25v5h-1.25v-5Zm2.375 0H12v5h-1.25v-5ZM4.25 5.75h7.5l-.45 7.1A1.75 1.75 0 0 1 9.55 14.5h-3.1a1.75 1.75 0 0 1-1.75-1.65l-.45-7.1Z"
      />
    </svg>
  );
}

const AGENT_DELETE_PARTIAL_REMOVAL_PREFIX =
  "Agent was removed, but hidden cleanup dir(s) remain:";

function formatAgentDeletePartialRemovalWarning(message: string): string {
  const details = message.slice(AGENT_DELETE_PARTIAL_REMOVAL_PREFIX.length).trim();
  return details
    ? `Agent was removed. A hidden cleanup folder remains on disk: ${details}`
    : "Agent was removed. A hidden cleanup folder remains on disk.";
}

function formatAgentDeleteBlockerError(report: BlockerReport): string {
  const normalized = normalizeBlockerReport(report);
  const liveSessions = normalized.liveSessions.map((session) =>
    `${session.agentName} at ${session.cwd}`
  );
  const externalProcesses = normalized.externalProcesses.map((process) => {
    const cwd = process.cwd ? ` at ${process.cwd}` : "";
    const files = process.files.length > 0 ? ` using ${process.files.join(", ")}` : "";
    return `${process.name} (PID ${process.pid})${cwd}${files}`;
  });
  const details: string[] = [];
  if (liveSessions.length > 0) details.push(`Live AC sessions: ${liveSessions.join("; ")}`);
  if (externalProcesses.length > 0) details.push(`External processes: ${externalProcesses.join("; ")}`);
  if (details.length === 0) {
    return `Agent delete is blocked. Raw delete error: ${normalized.rawDeleteError}`;
  }
  return `Agent delete is blocked. ${details.join(" ")} Close the listed sessions or processes, then try again.`;
}

function coordinatorItemKey(item: { replica: AcAgentReplica; wg: AcWorkgroup }): string {
  return `${item.wg.path}\u0000${item.replica.path}`;
}

function getActiveReplicasForWg(wg: AcWorkgroup): AcAgentReplica[] {
  return (wg.agents ?? []).filter(replica => isSessionLive(replicaSession(wg, replica)));
}

function runningCoordinatorPeers(wg: AcWorkgroup, replica: AcAgentReplica): AcAgentReplica[] {
  return (wg.agents ?? []).filter(
    (peer) => peer.name !== replica.name && isReplicaWorking(wg, peer)
  );
}

const ProjectPanel: Component = () => {
  let unlistenBranch: (() => void) | null = null;
  let unlistenClock: (() => void) | null = null;
  let unlistenAutoClose: (() => void) | null = null;
  let unlistenManualClose: (() => void) | null = null;
  onCleanup(registerCoordinatorCloseModalHost());
  onMount(async () => {
    unlistenBranch = await onDiscoveryBranchUpdated((data) => {
      replicaVolatileStore.applyDiscoveryBranchUpdate(
        data.replicaPath,
        data.branch,
        data.repoPaths,
        data.repoBranches,
        data.repoDirty
      );
    });
    unlistenClock = await onCoordinatorClockUpdated((data) => {
      replicaVolatileStore.setLastUserMessageAt(data.replicaPath, data.lastUserMessageAt);
    });
    unlistenAutoClose = await onCoordinatorAutoCloseChanged((data) => {
      replicaVolatileStore.setAutoClosedAt(data.replicaPath, data.autoClosedAt);
    });
    unlistenManualClose = await onCoordinatorManualCloseChanged((data) => {
      replicaVolatileStore.setManuallyClosedAt(data.replicaPath, data.manuallyClosedAt);
    });
  });
  onCleanup(() => {
    unlistenBranch?.();
    unlistenClock?.();
    unlistenAutoClose?.();
    unlistenManualClose?.();
  });

  const [pendingLaunch, setPendingLaunch] = createSignal<PendingLaunch | null>(null);
  const [collapsedByKey, setCollapsedByKey] = createSignal<Record<string, boolean>>({});
  const isPanelCollapsed = (key: string, defaultCollapsed = false) =>
    collapsedByKey()[key] ?? defaultCollapsed;
  const togglePanelCollapsed = (key: string, defaultCollapsed = false) => {
    setCollapsedByKey((prev) => ({
      ...prev,
      [key]: !(prev[key] ?? defaultCollapsed),
    }));
  };

  const isProjectPanelCollapsed = (projectPath: string) =>
    projectCollapseStore.isProjectCollapsed(projectPath);
  const toggleProjectPanelCollapsed = (projectPath: string) =>
    projectCollapseStore.toggleProjectCollapsed(projectPath);

  const [restartPrompt, setRestartPrompt] = createSignal<{
    sessionId: string;
    replicaName: string;
    agentId: string;
    agentLabel: string;
    requestedProfile: string | null;
  } | null>(null);
  const [editingTeamTarget, setEditingTeamTarget] = createSignal<{
    projectPath: string;
    teamName: string;
  } | null>(null);
  const closeEditTeamModal = () => setEditingTeamTarget(null);
  const [restarting, setRestarting] = createSignal(false);
  const [restartError, setRestartError] = createSignal("");

  const applyRestartPrompt = async () => {
    const prompt = restartPrompt();
    if (!prompt || restarting()) return;
    setRestarting(true);
    setRestartError("");
    let timeoutTimer: number | undefined;
    try {
      await Promise.race([
        SessionAPI.restart(prompt.sessionId, {
          agentId: prompt.agentId,
          requestedProfile: prompt.requestedProfile,
        }),
        new Promise<never>((_, reject) => {
          timeoutTimer = window.setTimeout(
            () => reject("Command timeout: restart_session"),
            RESTART_TIMEOUT_MS,
          );
        }),
      ]);
      setRestartPrompt(null);
    } catch (e) {
      console.error("Failed to restart session:", e);
      setRestartError(launchErrorMessage(e));
    } finally {
      window.clearTimeout(timeoutTimer);
      setRestarting(false);
    }
  };

  const dismissRestartPrompt = () => {
    if (restarting()) return;
    setRestartError("");
    setRestartPrompt(null);
  };

  const [newAgentTarget, setNewAgentTarget] = createSignal<{ projectPath: string } | null>(null);
  const [newTeamTarget, setNewTeamTarget] = createSignal<{ projectPath: string } | null>(null);
  const [newWorkgroupTarget, setNewWorkgroupTarget] = createSignal<{ projectPath: string } | null>(null);
  const [newLoopTarget, setNewLoopTarget] = createSignal<{ projectPath: string } | null>(null);
  const [editingLoopTarget, setEditingLoopTarget] = createSignal<{ projectPath: string; loopId: string } | null>(null);
  const [replicaCodingAgentTarget, setReplicaCodingAgentTarget] = createSignal<{ sessionId: string; sessionName: string } | null>(null);
  const [inactiveCodingAgentTarget, setInactiveCodingAgentTarget] = createSignal<{ projectPath: string; wgPath: string; replicaPath: string } | null>(null);

  const findProjectByPath = (projectPath: string | null | undefined) => {
    if (!projectPath) return undefined;
    const normalized = normalizeProjectPathForCompare(projectPath);
    return projectStore.projects.find(
      (p) => normalizeProjectPathForCompare(p.path) === normalized,
    );
  };

  const inactiveCodingAgentResolved = createMemo(() => {
    const target = inactiveCodingAgentTarget();
    if (!target) return null;
    const proj = findProjectByPath(target.projectPath);
    const wg = proj?.workgroups.find((w) => w.path === target.wgPath);
    const replica = wg?.agents.find((r) => r.path === target.replicaPath);
    return proj && wg && replica ? { proj, wg, replica } : null;
  });

  const editingLoopResolved = createMemo(() => {
    const target = editingLoopTarget();
    if (!target) return null;
    const proj = findProjectByPath(target.projectPath);
    const loop = proj?.loops.find((l) => l.id === target.loopId);
    return proj && loop ? { proj, loop } : null;
  });

  const restartReplicaSessionCore = async (
    sessionId: string,
    agentId?: string,
    requestedProfile?: string | null,
  ) => {
    let timeoutTimer: number | undefined;
    try {
      await Promise.race([
        SessionAPI.restart(
          sessionId,
          agentId ? { agentId, requestedProfile } : undefined,
        ),
        new Promise<never>((_, reject) => {
          timeoutTimer = window.setTimeout(
            () => reject("Command timeout: restart_session"),
            RESTART_TIMEOUT_MS,
          );
        }),
      ]);
    } catch (e) {
      console.error("Failed to restart session:", e); // keep for logs
      toastStore.error(launchErrorMessage(e));
    } finally {
      window.clearTimeout(timeoutTimer);
    }
  };

  const handleReplicaClick = async (replica: AcAgentReplica, wg: AcWorkgroup) => {
    const existing = replicaSession(wg, replica);
    if (existing) {
      if (!isSessionLive(existing)) {
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

    const gitRepos = buildGitRepos(replica);
    const resumeOnLaunch = !!(
      effectiveAutoClosedAt(replica) || effectiveManuallyClosedAt(replica)
    );

    setPendingLaunch({
      path: replica.path,
      sessionName: replicaSessionName(wg, replica),
      gitRepos,
      currentAgentId: replica.currentCodingAgentId ?? replica.preferredAgentId,
      currentRequestedProfile: replica.currentProfile ?? null,
      scopeContext: replicaScopeContext(wg, replica),
      resumeOnLaunch,
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
        const [showCtxMenu, setShowCtxMenu] = createSignal(false);
        const [ctxMenuPos, setCtxMenuPos] = createSignal({ x: 0, y: 0 });
        const [teamCtxMenu, setTeamCtxMenu] = createSignal<{ team: AcTeam; x: number; y: number } | null>(null);
        const [deletingTeam, setDeletingTeam] = createSignal<AcTeam | null>(null);
        const [deleteError, setDeleteError] = createSignal("");
        const [deleteInProgress, setDeleteInProgress] = createSignal(false);
        const [wgCtxMenu, setWgCtxMenu] = createSignal<{ wg: AcWorkgroup; x: number; y: number } | null>(null);
        const [replicaCtxMenu, setReplicaCtxMenu] = createSignal<
          | { kind: "active"; sessionId: string; sessionName: string; wg: AcWorkgroup; replica: AcAgentReplica; exited: boolean; x: number; y: number }
          | { kind: "inactive"; wg: AcWorkgroup; replica: AcAgentReplica; x: number; y: number }
          | null
        >(null);
        const [replicaTelegramBotMenu, setReplicaTelegramBotMenu] = createSignal<{
          epoch: number;
          sessionId: string;
          bots: TelegramBotConfig[];
        } | null>(null);
        let replicaCtxMenuEpoch = 0;
        // #1536 - inline TASK-title editor state (shared by both menu branches;
        // only one menu is ever open). titleEdit pins the raw wg.path + the
        // resolved live-session id at click time, so guards compare strings,
        // never menu/wg object identity (positionReplicaCtxMenu rewrites the
        // menu object on every reclamp; store events replace wg objects).
        const [titleEdit, setTitleEdit] = createSignal<{ wgPath: string; sessionId: string | null } | null>(null);
        const [titleDraft, setTitleDraft] = createSignal("");
        const [titleBusy, setTitleBusy] = createSignal(false);
        const [titleError, setTitleError] = createSignal<string | null>(null);
        // Monotonic invocation epoch (#1536 BLOCKER-2 token): every
        // startReplicaTitleEdit captures the current value after setting state;
        // resetTitleEditState() bumps it, so menu close, menu replacement, and
        // cancel invalidate every in-flight getTitle continuation SYNCHRONOUSLY
        // (no microtask window). A stale continuation must never touch shared
        // editor state that a newer editor owns.
        let titleEditEpoch = 0;

        const resetTitleEditState = () => {
          titleEditEpoch += 1;
          setTitleEdit(null);
          setTitleDraft("");
          setTitleBusy(false);
          setTitleError(null);
        };
        // #1536 BLOCKER-1 net 1: the reset effect fires on menu CLOSE (the null
        // transition). Menu REPLACEMENT (never through null, #943) is covered by
        // the resetTitleEditState() calls in both menu-open handlers; the render
        // guard in the editor JSX is the structural third net.
        createEffect(() => {
          if (!replicaCtxMenu()) resetTitleEditState();
        });
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
        const [groupCreateWgPath, setGroupCreateWgPath] = createSignal<string | null>(null);
        const [groupCreateName, setGroupCreateName] = createSignal("");
        const [groupMenuError, setGroupMenuError] = createSignal("");
        const [groupFlyoutOpen, setGroupFlyoutOpen] = createSignal(false);
        const [groupFlyoutPos, setGroupFlyoutPos] = createSignal({ x: 0, y: 0 });
        let groupFlyoutEl: HTMLDivElement | undefined;
        let groupFlyoutAnchorEl: HTMLElement | undefined;
        let groupFlyoutCloseTimer: number | undefined;

        const [repoFlyout, setRepoFlyout] = createSignal<{ index: number; sourcePath: string } | null>(null);
        const [repoFlyoutPos, setRepoFlyoutPos] = createSignal({ x: 0, y: 0 });
        let repoFlyoutEl: HTMLDivElement | undefined;
        let repoFlyoutAnchorEl: HTMLElement | undefined;
        let repoFlyoutCloseTimer: number | undefined;
        let suppressRepoFocusOpen = false;

        const [repoRemotes, setRepoRemotes] = createSignal<Record<string, GithubRepoRef | null>>({});
        let repoRemotesGen = 0;

        const browseSupported = () => isTauri;

        const resolveRepoRemotes = (repos: SessionRepo[]) => {
          const gen = ++repoRemotesGen;
          setRepoRemotes({});
          if (!browseSupported()) return;
          const paths = Array.from(
            new Set(repos.map((repo) => repo.sourcePath).filter((path) => !!path))
          );
          for (const path of paths) {
            void ReposAPI.gitRemoteUrl(path)
              .then((remote) => {
                if (gen !== repoRemotesGen) return;
                setRepoRemotes((prev) => ({ ...prev, [path]: parseGithubRemote(remote) }));
              })
              .catch((err) => {
                if (gen !== repoRemotesGen) return;
                console.debug("[repo-browse] git_remote_url failed:", err);
                setRepoRemotes((prev) => ({ ...prev, [path]: null }));
              });
          }
        };

        type RepoBrowseItem = { id: "main" | "branch"; label: string; url: string };

        const repoBrowseItems = (repo: SessionRepo | null): RepoBrowseItem[] => {
          if (!browseSupported() || !repo) return [];
          const ref = repoRemotes()[repo.sourcePath];
          if (!ref) return []; // undefined = resolving, null = no GitHub remote
          const items: RepoBrowseItem[] = [
            { id: "main", label: "Browse Main", url: githubRepoUrl(ref) },
          ];
          const branch = repo.branch?.trim();
          if (branch && branch !== "main" && branch !== "master" && branch !== "HEAD") {
            const url = githubBranchUrl(ref, branch);
            if (url) items.push({ id: "branch", label: "Browse Branch", url });
          }
          return items;
        };
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
            const msg = typeof e === "string" ? e : e?.message ?? "Failed to delete room";
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
                setWgDeleteError("Room is still locked, but the blocker report could not be parsed. Try again.");
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
          if (filterInputEl) focusOnMount(filterInputEl, { select: true });
        };
        const toggleFilter = () => {
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
        const cancelGroupFlyoutClose = () => {
          if (groupFlyoutCloseTimer === undefined) return;
          window.clearTimeout(groupFlyoutCloseTimer);
          groupFlyoutCloseTimer = undefined;
        };
        const closeGroupFlyout = () => {
          cancelGroupFlyoutClose();
          setGroupFlyoutOpen(false);
          groupFlyoutAnchorEl = undefined;
          groupFlyoutEl = undefined;
        };
        const groupFlyoutHasError = () =>
          !!groupMenuError() || !!workgroupGroupsStore.error(proj.path);
        const scheduleGroupFlyoutClose = () => {
          cancelGroupFlyoutClose();
          if (groupFlyoutHasError()) return;
          groupFlyoutCloseTimer = window.setTimeout(() => {
            groupFlyoutCloseTimer = undefined;
            if (groupFlyoutHasError()) return;
            closeGroupFlyout();
          }, 180);
        };
        const resetGroupCreateState = () => {
          setGroupCreateWgPath(null);
          setGroupCreateName("");
          setGroupMenuError("");
        };
        const resetGroupMenuState = () => {
          resetGroupCreateState();
          closeGroupFlyout();
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
        const sessionSearchText = (session: Session | undefined) => {
          if (!session) return "";
          return joinSearchText(
            session.name,
            session.agentLabel,
            sessionEffectiveStatusSearchText(session)
          );
        };
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
        const resolveReplicaAgentLabel = (
          session: Session | undefined,
          replica: AcAgentReplica
        ): string | null => {
          if (session) return liveAgentLabel(session);
          const agentId = replica.currentCodingAgentId ?? replica.preferredAgentId;
          if (!agentId) return null;
          return settingsStore.current?.agents?.find((a) => a.id === agentId)?.label ?? null;
        };
        const resolveReplicaProfileBadge = (
          session: Session | undefined,
          replica: AcAgentReplica
        ): string | null => {
          if (session) return sessionProfileBadge(session);
          return replica.currentProfile ?? null;
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
            : configuredReplicaRepoBadgesLive(replica, wg);
          return joinSearchText(
            taskTitle,
            stripFrontmatter(taskTitle ?? ""),
            replica.originProject ? `${replica.name}@${replica.originProject}` : replica.name,
            replica.isCoordinator
              ? repos.map((repo) => formatReplicaRepoBadgeLabel(repo)).join(" ")
              : null,
            resolveReplicaAgentLabel(session, replica),
            resolveReplicaProfileBadge(session, replica),
            replica.isCoordinator ? "orchestrator" : null,
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
            ? "Selected Room"
            : rowContext === "workgroups"
              ? "Rooms"
              : undefined;
          if (!filterActive() || matchesFilterText(sectionLabel) || workgroupOwnMatches(wg, sectionLabel)) {
            return wg.agents;
          }
          return wg.agents.filter((replica) => replicaMatches(replica, wg));
        };
        const agentMatches = (agent: { name: string; path: string; preferredAgentId?: string }) => {
          const session = sessionsStore.findSessionByName(agent.name);
          const repoText = sessionRepoSearchText(session);
          const codingAgentLabel = liveAgentLabel(session);
          // #1730 - the profile badge is passed unconditionally. This call used to
          // gate it on `!!codingAgentLabel || repoText !== ""`, a second copy of the
          // meta-strip gate SessionItem.tsx carried before #1730. That gate is gone,
          // so every session row now carries this badge's data, and the filter has
          // to find it. Do not re-add a gate here: replicaSearchText above never had
          // one, and a voice transient hiding the strip is not a rule about the data.
          return matchesFilterText(
            agentDisplayName(agent.name),
            sessionSearchText(session),
            codingAgentLabel,
            repoText,
            session ? sessionProfileBadge(session) : null
          );
        };
        const teamMemberMatches = (team: AcTeam, agentName: string) =>
          matchesFilterText(teamMemberDisplayLabel(agentName), agentName === team.coordinator ? "orchestrator" : null);
        const teamOwnMatches = (team: AcTeam) => matchesFilterText(team.name);
        const teamMatches = (team: AcTeam) =>
          teamOwnMatches(team) || team.agents.some((agentName) => teamMemberMatches(team, agentName));
        const filteredTeamMembers = (team: AcTeam) => {
          if (!filterActive() || matchesFilterText("Teams") || teamOwnMatches(team)) return team.agents;
          return team.agents.filter((agentName) => teamMemberMatches(team, agentName));
        };
        const groupsConfig = () => workgroupGroupsStore.config(proj.path);
        const selectedGroup = () => workgroupGroupsStore.selection(proj.path);
        const compiledGroups = createMemo(() =>
          groupsConfig().groups.map((group) => ({ group, regex: compileGroupRegex(group) }))
        );
        const canTestGroupMatchId = (wg: AcWorkgroup) =>
          groupMatchId(wg).length <= MAX_GROUP_MATCH_ID_LENGTH;
        const groupMatchesWorkgroup = (wg: AcWorkgroup, groupId: string) => {
          if (!canTestGroupMatchId(wg)) return false;
          const compiled = compiledGroups().find((entry) => entry.group.id === groupId);
          return !!compiled?.regex?.test(groupMatchId(wg));
        };
        const workgroupMatchesAnyGroup = (wg: AcWorkgroup) => {
          const nonStop = groupsConfig().nonStop;
          return (
            (canTestGroupMatchId(wg) &&
              compiledGroups().some((entry) => entry.regex?.test(groupMatchId(wg)))) ||
            (!!nonStop && nonStopMatchesWorkgroup(nonStop, wg))
          );
        };
        const groupPredicate = (wg: AcWorkgroup) => {
          const selected = selectedGroup();
          if (selected.kind === "all") return true;
          if (selected.kind === "ungrouped") return !workgroupMatchesAnyGroup(wg);
          if (selected.kind === "nonstop") {
            const ns = groupsConfig().nonStop;
            return !!ns && nonStopMatchesWorkgroup(ns, wg);
          }
          return groupMatchesWorkgroup(wg, selected.id);
        };
        const groupVisibleWorkgroups = createMemo(() => proj.workgroups.filter(groupPredicate));
        const filteredWorkgroups = createMemo(() => {
          const base = groupVisibleWorkgroups();
          if (!filterActive() || matchesFilterText("Rooms")) return base;
          return base.filter((wg) => workgroupMatches(wg, "Rooms"));
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
          if (!wg || !groupPredicate(wg)) return false;
          return !filterActive() || matchesFilterText("Selected Room") || workgroupMatches(wg, "Selected Room");
        };
        const loopStatusText = (loop: AcLoopSummary) =>
          loop.lastResult?.message ?? (loop.nextDueAt ? `Next: ${new Date(loop.nextDueAt).toLocaleString()}` : "No runs yet");
        const loopSearchText = (loop: AcLoopSummary) =>
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

        let replicaCtxMenuCloseTimer: number | undefined;
        const cancelReplicaCtxMenuClose = () => {
          if (replicaCtxMenuCloseTimer === undefined) return;
          window.clearTimeout(replicaCtxMenuCloseTimer);
          replicaCtxMenuCloseTimer = undefined;
        };

        const advanceReplicaCtxMenuEpoch = () => {
          replicaCtxMenuEpoch += 1;
          setReplicaTelegramBotMenu(null);
          return replicaCtxMenuEpoch;
        };

        const cleanupCtx = () => {
          advanceReplicaCtxMenuEpoch();
          cancelReplicaCtxMenuClose();
          if (dismissCtx) {
            window.removeEventListener("click", dismissCtx);
            window.removeEventListener("contextmenu", dismissCtx);
            window.removeEventListener("keydown", dismissCtx as any);
            dismissCtx = null;
          }
        };

        onCleanup(cleanupCtx);

        const closeReplicaCtxMenu = () => {
          setReplicaCtxMenu(null);
          cleanupCtx();
        };
        const groupErrorPinned = () => groupFlyoutOpen() && groupFlyoutHasError();
        const scheduleReplicaCtxMenuClose = () => {
          cancelReplicaCtxMenuClose();
          if (groupErrorPinned()) return;
          replicaCtxMenuCloseTimer = window.setTimeout(() => {
            replicaCtxMenuCloseTimer = undefined;
            if (groupErrorPinned()) return;
            closeReplicaCtxMenu();
          }, CONTEXT_MENU_CLOSE_GRACE_MS);
        };

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

        const reclampReplicaCtxMenu = (expected?: { epoch: number; sessionId: string }) => {
          if (!replicaCtxMenu()) return;
          const clamp = () => {
            const menu = replicaCtxMenu();
            if (!menu) return;
            if (
              expected &&
              (replicaCtxMenuEpoch !== expected.epoch ||
                menu.kind !== "active" ||
                menu.sessionId !== expected.sessionId)
            ) {
              return;
            }
            positionReplicaCtxMenu(menu.x, menu.y);
          };
          if (typeof window.requestAnimationFrame === "function") {
            window.requestAnimationFrame(clamp);
            return;
          }
          window.setTimeout(clamp, 0);
        };

        const positionGroupFlyout = (anchor: HTMLElement) => {
          const rect = anchor.getBoundingClientRect();
          const width = groupFlyoutEl?.getBoundingClientRect().width ?? 220;
          const height = groupFlyoutEl?.getBoundingClientRect().height ?? 180;
          let x = rect.right + 4;
          if (x + width + CONTEXT_MENU_VIEWPORT_MARGIN > window.innerWidth) {
            x = rect.left - width - 4;
          }
          const maxX = Math.max(
            CONTEXT_MENU_VIEWPORT_MARGIN,
            window.innerWidth - width - CONTEXT_MENU_VIEWPORT_MARGIN
          );
          const maxY = Math.max(
            CONTEXT_MENU_VIEWPORT_MARGIN,
            window.innerHeight - height - CONTEXT_MENU_VIEWPORT_MARGIN
          );
          setGroupFlyoutPos({
            x: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, x), maxX),
            y: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, rect.top), maxY),
          });
        };

        const reclampGroupFlyout = () => {
          const anchor = groupFlyoutAnchorEl;
          if (!anchor || !groupFlyoutOpen()) return;
          const clamp = () => positionGroupFlyout(anchor);
          if (typeof window.requestAnimationFrame === "function") {
            window.requestAnimationFrame(clamp);
            return;
          }
          window.setTimeout(clamp, 0);
        };

        const openGroupFlyout = (anchor: HTMLElement) => {
          closeRepoFlyout(); // #943 - only one flyout at a time
          cancelGroupFlyoutClose();
          groupFlyoutAnchorEl = anchor;
          positionGroupFlyout(anchor);
          setGroupFlyoutOpen(true);
          reclampGroupFlyout();
        };

        const activateGroupFlyout = (anchor: HTMLElement) => {
          closeRepoFlyout(); // #943 - the early-return path below skips openGroupFlyout
          if (groupFlyoutOpen() && groupFlyoutAnchorEl === anchor) {
            cancelGroupFlyoutClose();
            positionGroupFlyout(anchor);
            reclampGroupFlyout();
            return;
          }
          openGroupFlyout(anchor);
        };
        onCleanup(cancelGroupFlyoutClose);

        const cancelRepoFlyoutClose = () => {
          if (repoFlyoutCloseTimer === undefined) return;
          window.clearTimeout(repoFlyoutCloseTimer);
          repoFlyoutCloseTimer = undefined;
        };
        const closeRepoFlyout = () => {
          cancelRepoFlyoutClose();
          setRepoFlyout(null);
          repoFlyoutAnchorEl = undefined;
          repoFlyoutEl = undefined;
        };
        const scheduleRepoFlyoutClose = () => {
          cancelRepoFlyoutClose();
          repoFlyoutCloseTimer = window.setTimeout(() => {
            repoFlyoutCloseTimer = undefined;
            closeRepoFlyout();
          }, 180);
        };

        const positionRepoFlyout = (anchor: HTMLElement) => {
          const rect = anchor.getBoundingClientRect();
          const width = repoFlyoutEl?.getBoundingClientRect().width ?? 220;
          const height = repoFlyoutEl?.getBoundingClientRect().height ?? 88;
          let x = rect.right + 4;
          if (x + width + CONTEXT_MENU_VIEWPORT_MARGIN > window.innerWidth) {
            x = rect.left - width - 4;
          }
          const maxX = Math.max(
            CONTEXT_MENU_VIEWPORT_MARGIN,
            window.innerWidth - width - CONTEXT_MENU_VIEWPORT_MARGIN
          );
          const maxY = Math.max(
            CONTEXT_MENU_VIEWPORT_MARGIN,
            window.innerHeight - height - CONTEXT_MENU_VIEWPORT_MARGIN
          );
          setRepoFlyoutPos({
            x: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, x), maxX),
            y: Math.min(Math.max(CONTEXT_MENU_VIEWPORT_MARGIN, rect.top), maxY),
          });
        };

        const reclampRepoFlyout = () => {
          const anchor = repoFlyoutAnchorEl;
          if (!anchor?.isConnected || !repoFlyout()) return;
          const clamp = () => {
            if (anchor !== repoFlyoutAnchorEl || !anchor.isConnected || !repoFlyout()) return;
            positionRepoFlyout(anchor);
          };
          if (typeof window.requestAnimationFrame === "function") {
            window.requestAnimationFrame(clamp);
            return;
          }
          window.setTimeout(clamp, 0);
        };

        const openRepoFlyout = (index: number, repo: SessionRepo, anchor: HTMLElement) => {
          closeGroupFlyout(); // mutual exclusion: both flyouts are position:fixed
          cancelRepoFlyoutClose();
          repoFlyoutAnchorEl = anchor;
          positionRepoFlyout(anchor);
          setRepoFlyout({ index, sourcePath: repo.sourcePath });
          reclampRepoFlyout();
        };

        const focusFirstRepoFlyoutItem = () => {
          queueMicrotask(() => repoFlyoutEl?.querySelector("button")?.focus());
        };

        const focusRepoTriggerQuietly = (anchor: HTMLElement | undefined) => {
          if (!anchor?.isConnected) return;
          suppressRepoFocusOpen = true;
          anchor.focus();
          queueMicrotask(() => {
            suppressRepoFocusOpen = false;
          });
        };

        const openRepoBrowse = async (url: string) => {
          closeRepoFlyout();
          closeReplicaCtxMenu();
          try {
            await WindowAPI.openExternal(url);
          } catch (e) {
            console.error("Failed to open repo in browser:", e);
          }
        };

        createEffect(() => {
          if (replicaCtxMenu()) return;
          closeRepoFlyout();
          repoRemotesGen++;
          setRepoRemotes({});
        });

        onCleanup(cancelRepoFlyoutClose);

        const restartReplicaSession = async (
          sessionId: string,
          agentId?: string,
          requestedProfile?: string | null,
        ) => {
          closeReplicaCtxMenu();
          await restartReplicaSessionCore(sessionId, agentId, requestedProfile);
        };

        const toggleReplicaDetach = async (sessionId: string) => {
          closeReplicaCtxMenu();
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

        const activeReplicaMenu = () => {
          const m = replicaCtxMenu();
          return m && m.kind === "active" ? m : null;
        };
        const inactiveReplicaMenu = () => {
          const m = replicaCtxMenu();
          return m && m.kind === "inactive" ? m : null;
        };
        const activeReplicaMenuSession = () => {
          const menu = activeReplicaMenu();
          return menu
            ? sessionsStore.sessions.find((session) => session.id === menu.sessionId)
            : undefined;
        };
        const activeReplicaMenuBridge = () => {
          const menu = activeReplicaMenu();
          return menu ? bridgesStore.getBridge(menu.sessionId) : undefined;
        };

        type ReplicaTelegramInvocation = {
          epoch: number;
          sessionId: string;
          startedLive: boolean;
          startingBridge: NonNullable<ReturnType<typeof bridgesStore.getBridge>> | null;
        };

        const currentReplicaTelegramInvocation = (
          token: ReplicaTelegramInvocation,
        ): Session | null => {
          if (!token.startedLive || replicaCtxMenuEpoch !== token.epoch) return null;
          const menu = activeReplicaMenu();
          if (!menu || menu.sessionId !== token.sessionId) return null;
          const session = sessionsStore.sessions.find(
            (candidate) => candidate.id === token.sessionId,
          );
          if (!session || !isSessionLive(session)) return null;
          const bridge = bridgesStore.getBridge(token.sessionId) ?? null;
          return bridge === token.startingBridge ? session : null;
        };

        const handleReplicaTelegramAction = async (
          event: MouseEvent,
          sessionId: string,
        ) => {
          event.stopPropagation();
          if (activeReplicaMenu()?.sessionId !== sessionId) return;

          const session = sessionsStore.sessions.find((candidate) => candidate.id === sessionId);
          const startingBridge = bridgesStore.getBridge(sessionId) ?? null;
          const startedLive = !!session && isSessionLive(session);
          const epoch = advanceReplicaCtxMenuEpoch();
          const token: ReplicaTelegramInvocation = {
            epoch,
            sessionId,
            startedLive,
            startingBridge,
          };

          if (!session || !startedLive) {
            closeReplicaCtxMenu();
            return;
          }
          if (startingBridge) {
            closeReplicaCtxMenu();
            await TelegramAPI.detach(sessionId);
            return;
          }

          let settings: Awaited<ReturnType<typeof SettingsAPI.get>>;
          try {
            settings = await SettingsAPI.get();
          } catch (error) {
            if (currentReplicaTelegramInvocation(token)) throw error;
            return;
          }

          if (!currentReplicaTelegramInvocation(token)) return;
          const bots = settings.telegramBots || [];
          if (bots.length === 0) {
            closeReplicaCtxMenu();
            return;
          }
          if (bots.length === 1) {
            closeReplicaCtxMenu();
            await TelegramAPI.attach(sessionId, bots[0].id);
            return;
          }

          setReplicaTelegramBotMenu({ epoch, sessionId, bots });
          reclampReplicaCtxMenu({ epoch, sessionId });
        };

        const handleReplicaTelegramBotSelect = async (
          event: MouseEvent,
          sessionId: string,
          botId: string,
          epoch: number,
        ) => {
          event.stopPropagation();
          const menu = activeReplicaMenu();
          const choices = replicaTelegramBotMenu();
          const session = sessionsStore.sessions.find((candidate) => candidate.id === sessionId);
          if (
            replicaCtxMenuEpoch !== epoch ||
            !menu ||
            menu.sessionId !== sessionId ||
            !choices ||
            choices.epoch !== epoch ||
            choices.sessionId !== sessionId ||
            !session ||
            !isSessionLive(session) ||
            (bridgesStore.getBridge(sessionId) ?? null) !== null
          ) {
            return;
          }

          const targetSessionId = sessionId;
          const targetBotId = botId;
          closeReplicaCtxMenu();
          const currentSession = sessionsStore.sessions.find(
            (candidate) => candidate.id === targetSessionId,
          );
          if (!currentSession || !isSessionLive(currentSession)) return;
          await TelegramAPI.attach(targetSessionId, targetBotId);
        };

        const handleReplicaContextClose = (event: MouseEvent, sessionId: string) => {
          event.stopPropagation();
          if (activeReplicaMenu()?.sessionId !== sessionId) return;
          const targetSessionId = sessionId;
          closeReplicaCtxMenu();
          const session = sessionsStore.sessions.find(
            (candidate) => candidate.id === targetSessionId,
          );
          if (session) void requestCoordinatorClose(session);
        };

        const resolveWorkgroupSessionId = (wg: AcWorkgroup): string | null => {
          for (const peer of wg.agents) {
            const s = replicaSession(wg, peer);
            if (s && !s.id.startsWith("inactive-")) return s.id;
          }
          return null;
        };

        const groupAlreadyMatches = (wg: AcWorkgroup, groupId: string) =>
          groupMatchesWorkgroup(wg, groupId);

        const toggleExistingGroup = async (wg: AcWorkgroup, groupId: string) => {
          setGroupMenuError("");
          try {
            if (groupAlreadyMatches(wg, groupId)) {
              await workgroupGroupsStore.removeWorkgroupFromGroup(proj.path, groupId, wg.name);
            } else {
              await workgroupGroupsStore.addWorkgroupToGroup(proj.path, groupId, wg.name);
            }
          } catch (error) {
            setGroupMenuError(error instanceof Error ? error.message : String(error));
            reclampGroupFlyout();
            reclampReplicaCtxMenu();
          }
        };

        const nonStopChecked = (wg: AcWorkgroup) => {
          const ns = groupsConfig().nonStop;
          return !!ns && nonStopMatchesWorkgroup(ns, wg);
        };
        const toggleNonStop = async (wg: AcWorkgroup) => {
          setGroupMenuError("");
          try {
            if (nonStopChecked(wg)) {
              await workgroupGroupsStore.removeWorkgroupFromNonStop(proj.path, wg.name);
            } else {
              await workgroupGroupsStore.addWorkgroupToNonStop(proj.path, wg.name);
            }
          } catch (error) {
            setGroupMenuError(error instanceof Error ? error.message : String(error));
            reclampGroupFlyout();
            reclampReplicaCtxMenu();
          }
        };

        const createGroupFromMenu = async (wg: AcWorkgroup) => {
          const name = groupCreateName().trim();
          if (!name) {
            setGroupMenuError("Group name cannot be blank.");
            reclampGroupFlyout();
            reclampReplicaCtxMenu();
            return;
          }
          setGroupMenuError("");
          try {
            await workgroupGroupsStore.createGroupForWorkgroup(proj.path, name, wg.name);
            resetGroupCreateState();
            reclampGroupFlyout();
            reclampReplicaCtxMenu();
          } catch (error) {
            setGroupMenuError(error instanceof Error ? error.message : String(error));
            reclampGroupFlyout();
            reclampReplicaCtxMenu();
          }
        };

        const renderAddToGroupFlyout = (wg: AcWorkgroup) => (
          <Portal>
            <Show when={groupFlyoutOpen()}>
              <div
                class="session-context-flyout"
                ref={groupFlyoutEl}
                style={{ left: `${groupFlyoutPos().x}px`, top: `${groupFlyoutPos().y}px` }}
                onMouseEnter={() => {
                  cancelGroupFlyoutClose();
                  cancelReplicaCtxMenuClose(); // #977 - still inside the menu
                }}
                onMouseLeave={() => {
                  scheduleGroupFlyoutClose();
                  scheduleReplicaCtxMenuClose(); // #977 - re-armed; the menu cancels it on re-entry
                }}
                onClick={(e) => e.stopPropagation()}
                onContextMenu={(e) => e.stopPropagation()}
                data-ac-testid={`replica.${automationIdPart(wg.name)}.groups.flyout`}
              >
                {/* #777: built-in Non-stop slot, pinned above the user groups. */}
                <button
                  class="session-context-option session-context-group-option session-context-group-option-nonstop"
                  title={`Watch this room in the ${DEFAULT_NON_STOP_NAME} group`}
                  onClick={() => void toggleNonStop(wg)}
                  data-ac-testid={`replica.${automationIdPart(wg.name)}.groups.nonstop`}
                >
                  <span class="session-context-option-check">
                    {nonStopChecked(wg) ? "✓" : ""}
                  </span>
                  <span>{DEFAULT_NON_STOP_NAME}</span>
                </button>
                <For each={groupsConfig().groups}>
                  {(group) => {
                    const valid = () => !!compileGroupRegex(group);
                    const selected = () => groupAlreadyMatches(wg, group.id);
                    const removable = () => removeExactGroupToken(group.regex, wg.name) !== null;
                    const customMembership = () => selected() && !removable();
                    const disabled = () => !valid() || customMembership();
                    const title = () => {
                      if (!valid()) return "Fix this group's regex before adding a room";
                      if (customMembership()) {
                        return "Membership comes from a custom regex. Use Edit groups to change it.";
                      }
                      return selected() ? "Remove this room from the group" : group.regex;
                    };
                    return (
                      <button
                        class="session-context-option session-context-group-option"
                        classList={{ "context-option-disabled": disabled() }}
                        disabled={disabled()}
                        title={title()}
                        onClick={() => {
                          if (disabled()) return;
                          void toggleExistingGroup(wg, group.id);
                        }}
                        data-ac-testid={`replica.${automationIdPart(wg.name)}.groups.${automationIdPart(group.id)}`}
                      >
                        <span class="session-context-option-check">
                          {selected() ? "\u2713" : ""}
                        </span>
                        <span>{group.name}</span>
                      </button>
                    );
                  }}
                </For>
                <Show when={groupsConfig().groups.length === 0}>
                  <div class="session-context-note">No groups yet</div>
                </Show>
                <Show when={workgroupGroupsStore.error(proj.path)}>
                  {(error) => <div class="session-context-error">{error()}</div>}
                </Show>
                <Show when={groupMenuError()}>
                  <div class="session-context-error" data-ac-testid="replica.groups.error">
                    {groupMenuError()}
                  </div>
                </Show>
                <Show
                  when={groupCreateWgPath() === wg.path}
                  fallback={
                    <button
                      class="session-context-option"
                      onClick={() => {
                        setGroupCreateWgPath(wg.path);
                        setGroupCreateName("");
                        setGroupMenuError("");
                        reclampGroupFlyout();
                      }}
                      data-ac-testid={`replica.${automationIdPart(wg.name)}.groups.create`}
                    >
                      Create new group
                    </button>
                  }
                >
                  <div class="session-context-inline-create">
                    {/* #746 — deliberately unconditional: a discovery-driven
                        re-render disposes this subtree (focus falls to <body>
                        regardless), so re-focusing the fresh input preserves
                        typing continuity; the value lives in groupCreateName. */}
                    <input
                      class="session-context-inline-input"
                      ref={(el) => focusOnMount(el)}
                      value={groupCreateName()}
                      onInput={(e) => setGroupCreateName(e.currentTarget.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          void createGroupFromMenu(wg);
                        }
                      }}
                      placeholder="Group name"
                      data-ac-testid="replica.groups.create.input"
                    />
                    <button
                      class="session-context-option"
                      onClick={() => void createGroupFromMenu(wg)}
                      data-ac-testid="replica.groups.create.save"
                    >
                      Create
                    </button>
                  </div>
                </Show>
              </div>
            </Show>
          </Portal>
        );

        const renderAddToGroupItem = (wg: AcWorkgroup, replica: AcAgentReplica) => (
          <Show when={replica.isCoordinator}>
            <div class="context-separator" />
            <button
              class="session-context-option session-context-submenu-trigger"
              onMouseEnter={(e) => openGroupFlyout(e.currentTarget)}
              onMouseLeave={scheduleGroupFlyoutClose}
              onFocus={(e) => openGroupFlyout(e.currentTarget)}
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                activateGroupFlyout(e.currentTarget);
              }}
              onKeyDown={(e) => {
                if (e.key !== "Enter" && e.key !== " ") return;
                e.preventDefault();
                e.stopPropagation();
                activateGroupFlyout(e.currentTarget);
              }}
              data-ac-testid={`replica.${automationIdPart(wg.name)}.groups.trigger`}
            >
              <span class="session-context-option-icon" aria-hidden="true">&#x1F465;</span>
              <span>Add to Group</span>
              <span class="session-context-submenu-arrow">&rsaquo;</span>
            </button>
            {renderAddToGroupFlyout(wg)}
          </Show>
        );

        const renderRepoBrowseFlyout = (repos: () => SessionRepo[], testIdPrefix: () => string) => {
          const liveRepo = (): SessionRepo | null => {
            const fly = repoFlyout();
            if (!fly) return null;
            const candidate = repos()[fly.index];
            return candidate && candidate.sourcePath === fly.sourcePath ? candidate : null;
          };

          createEffect(
            on(
              () => (repoFlyout() ? liveRepo() : null),
              (repo) => {
                if (repoFlyout() && !repo) closeRepoFlyout();
              },
              { defer: true }
            )
          );

          return (
            <Portal>
              <Show when={repoFlyout() && liveRepo()}>
                {(repo) => (
                  <div
                    class="session-context-flyout"
                    ref={repoFlyoutEl}
                    style={{ left: `${repoFlyoutPos().x}px`, top: `${repoFlyoutPos().y}px` }}
                    onMouseEnter={() => {
                      cancelRepoFlyoutClose();
                      cancelReplicaCtxMenuClose(); // #977 - still inside the menu
                    }}
                    onMouseLeave={() => {
                      scheduleRepoFlyoutClose();
                      scheduleReplicaCtxMenuClose(); // #977 - re-armed; the menu cancels it on re-entry
                    }}
                    onClick={(e) => e.stopPropagation()}
                    onContextMenu={(e) => e.stopPropagation()}
                    onKeyDown={(e) => {
                      if (e.key !== "Escape") return;
                      e.preventDefault();
                      e.stopPropagation(); // close the submenu only, keep the menu
                      const anchor = repoFlyoutAnchorEl;
                      closeRepoFlyout();
                      focusRepoTriggerQuietly(anchor);
                    }}
                    data-ac-testid={`${testIdPrefix()}.${repoFlyout()?.index ?? 0}.browse.flyout`}
                  >
                    <For each={repoBrowseItems(repo())}>
                      {(item) => (
                        <button
                          class="session-context-option"
                          title={item.url}
                          onClick={() => void openRepoBrowse(item.url)}
                          data-ac-testid={`${testIdPrefix()}.${repoFlyout()?.index ?? 0}.browse.${item.id}`}
                          data-ac-role="menuitem"
                        >
                          {item.label}
                        </button>
                      )}
                    </For>
                  </div>
                )}
              </Show>
            </Portal>
          );
        };

        const renderRepoMenuEntries = (repos: () => SessionRepo[], testIdPrefix: () => string) => (
          <>
            <Show when={repos().length > 0}>
              <For each={repos()}>
                {(repo, index) => {
                  const browseItems = () => repoBrowseItems(repo);
                  return (
                    <button
                      class="session-context-option session-context-repo-option"
                      title={repo.sourcePath}
                      onClick={() => void openRepoFolder(repo.sourcePath)}
                      onMouseEnter={(e) => {
                        if (browseItems().length > 0) {
                          openRepoFlyout(index(), repo, e.currentTarget);
                        } else {
                          closeRepoFlyout();
                        }
                      }}
                      onMouseLeave={scheduleRepoFlyoutClose}
                      onFocus={(e) => {
                        if (suppressRepoFocusOpen) return; // Escape refocus, see F.5
                        if (browseItems().length > 0) openRepoFlyout(index(), repo, e.currentTarget);
                      }}
                      onKeyDown={(e) => {
                        if (e.key === "ArrowRight" && browseItems().length > 0) {
                          e.preventDefault();
                          e.stopPropagation();
                          openRepoFlyout(index(), repo, e.currentTarget);
                          focusFirstRepoFlyoutItem();
                          return;
                        }
                        if (e.key === "Escape" && repoFlyout()) {
                          e.preventDefault();
                          e.stopPropagation(); // close the submenu only, keep the menu
                          closeRepoFlyout();
                        }
                      }}
                      data-ac-testid={`${testIdPrefix()}.${index()}`}
                      data-ac-role="menuitem"
                    >
                      <span class="session-context-option-icon" aria-hidden="true">
                        <svg
                          class="session-context-repo-icon"
                          viewBox="0 0 16 16"
                          aria-hidden="true"
                        >
                          <path
                            fill="currentColor"
                            d="M1.75 4.25A1.75 1.75 0 0 1 3.5 2.5h3.1c.46 0 .9.18 1.22.5l.9.9h3.78A1.75 1.75 0 0 1 14.25 5.65v5.1a1.75 1.75 0 0 1-1.75 1.75h-9A1.75 1.75 0 0 1 1.75 10.75v-6.5Z"
                          />
                        </svg>
                      </span>
                      <span class="session-context-repo-label">{repo.label}</span>
                      <Show when={browseItems().length > 0}>
                        <span
                          class="session-context-submenu-arrow"
                          data-ac-testid={`${testIdPrefix()}.${index()}.browse.arrow`}
                        >
                          &rsaquo;
                        </span>
                      </Show>
                    </button>
                  );
                }}
              </For>
            </Show>
            {/* Deliberately OUTSIDE the entry-count <Show>: a transient empty
                list must not unmount the Portal while repoFlyout() is non-null. */}
            {renderRepoBrowseFlyout(repos, testIdPrefix)}
          </>
        );

        const clearReplicaTaskTitle = async (wg: AcWorkgroup) => {
          closeReplicaCtxMenu();
          const sessionId = resolveWorkgroupSessionId(wg);
          try {
            if (sessionId) {
              await TaskAPI.clean(sessionId);
            } else {
              await TaskAPI.cleanAt(wg.path);
            }
          } catch (e) {
            console.error("Failed to clear task title:", e);
          }
        };

        // #1536 - liveness-aware session resolution for the EDITOR only. The
        // broom keeps the existing resolveWorkgroupSessionId (first non-inactive
        // id, live or not). An exited/dropped session would make the
        // session-based commands reject with "session not found", so the editor
        // falls through to the path-based task_set_title_at for those workgroups.
        const resolveLiveWorkgroupSessionId = (wg: AcWorkgroup): string | null => {
          for (const peer of wg.agents) {
            const s = replicaSession(wg, peer);
            if (s && !s.id.startsWith("inactive-") && isSessionLive(s)) return s.id;
          }
          return null;
        };

        const startReplicaTitleEdit = async (wg: AcWorkgroup) => {
          const sessionId = resolveLiveWorkgroupSessionId(wg);
          setTitleError(null);
          setTitleDraft(wg.taskTitle ?? "");
          setTitleEdit({ wgPath: wg.path, sessionId });
          // Invocation token (#1536 BLOCKER-2). Each start bumps the epoch; any
          // later resetTitleEditState() (menu close, replacement, cancel) or
          // newer start bumps it again, so this invocation detects staleness
          // with zero microtask window - including same-wg-same-session
          // double-clicks.
          const epoch = ++titleEditEpoch;
          const stillCurrent = () =>
            titleEditEpoch === epoch &&
            !!titleEdit() &&
            titleEdit()!.wgPath === wg.path &&
            titleEdit()!.sessionId === sessionId;
          if (sessionId) {
            setTitleBusy(true);
            try {
              const fromBackend = await TaskAPI.getTitle(sessionId);
              // Stale: bail BEFORE any shared-state mutation.
              if (!stillCurrent()) return;
              if (fromBackend !== null && fromBackend !== undefined) {
                setTitleDraft(fromBackend);
              }
            } catch (err) {
              // Stale: never reset a newer editor's state.
              if (!stillCurrent()) return;
              resetTitleEditState();
              setTitleError(String(err));
              return;
            } finally {
              // Busy belongs to the CURRENT invocation only; a stale continuation
              // must not clear a newer invocation's busy flag.
              if (stillCurrent()) setTitleBusy(false);
            }
          }
          if (!stillCurrent()) return;
          // The editor grows the menu: re-clamp so it stays inside the viewport.
          reclampReplicaCtxMenu();
        };

        const saveReplicaTitle = async () => {
          const target = titleEdit();
          if (!target) return;
          const title = titleDraft().trim();
          if (!title) {
            setTitleError("Title cannot be empty.");
            return;
          }
          setTitleBusy(true);
          setTitleError(null);
          try {
            if (target.sessionId) {
              await TaskAPI.setTitle(target.sessionId, title);
            } else {
              await TaskAPI.setTitleAt(target.wgPath, title);
            }
            // Close only if the same workgroup's menu is still open (raw path
            // equality - store events replace wg objects, so reference identity
            // would fail right after the save event lands).
            if (replicaCtxMenu() && replicaCtxMenu()!.wg.path === target.wgPath) {
              closeReplicaCtxMenu();
            }
          } catch (e) {
            console.error("Failed to edit task title:", e);
            setTitleError(String(e));
          } finally {
            setTitleBusy(false);
          }
        };

        const cancelReplicaTitleEdit = () => {
          resetTitleEditState();
        };

        const openMatrixFolder = async (path: string) => {
          setAgentCtxMenu(null);
          closeReplicaCtxMenu();
          try {
            await WindowAPI.openInExplorer(path);
          } catch (e) {
            console.error("Failed to open Matrix folder:", e);
          }
        };

        const openReplicaFolder = async (path: string) => {
          closeReplicaCtxMenu();
          try {
            await WindowAPI.openInExplorer(path);
          } catch (e) {
            console.error("Failed to open Replica folder:", e);
          }
        };

        const openRepoFolder = async (path: string) => {
          closeReplicaCtxMenu();
          try {
            await WindowAPI.openInExplorer(path);
          } catch (e) {
            console.error("Failed to open repo folder:", e);
          }
        };

        const handleProjectContextMenu = (e: MouseEvent) => {
          if (e.target instanceof Element && e.target.closest(".project-filter-row")) return;
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
        const coordinatorsCollapsedKey = projectPanelCollapseKey(proj.path, "coordinators");
        const selectedWorkgroupCollapsedKey = projectPanelCollapseKey(proj.path, "selected-workgroup");
        const workgroupsCollapsedKey = projectPanelCollapseKey(proj.path, "workgroups");
        const loopsCollapsedKey = projectPanelCollapseKey(proj.path, "loops");
        const agentsCollapsedKey = projectPanelCollapseKey(proj.path, "agents");
        const teamsCollapsedKey = projectPanelCollapseKey(proj.path, "teams");
        const hasLoopTargets = () =>
          proj.workgroups.some((wg) => wg.agents.some((agent) => agent.isCoordinator));
        const coordinatorPairCache = new Map<string, { replica: AcAgentReplica; wg: AcWorkgroup }>();
        const naturalCoordinatorItems = createMemo(() => {
          const result: { replica: AcAgentReplica; wg: AcWorkgroup }[] = [];
          for (const wg of groupVisibleWorkgroups()) {
            for (const replica of wg.agents) {
              if (replica.isCoordinator) {
                const key = coordinatorItemKey({ replica, wg });
                const cached = coordinatorPairCache.get(key);
                if (cached && cached.replica === replica && cached.wg === wg) {
                  result.push(cached);
                } else {
                  const pair = { replica, wg };
                  coordinatorPairCache.set(key, pair);
                  result.push(pair);
                }
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

        const handleArchiveProject = async () => {
          setShowCtxMenu(false);
          try {
            await projectStore.archiveProject(proj.path);
          } catch (error) {
            toastStore.error(typeof error === "string" ? error : String(error));
          }
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

        const handleReplicaContextMenu = (e: MouseEvent, session: Session, wg: AcWorkgroup, replica: AcAgentReplica) => {
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
          resetGroupMenuState();
          resetTitleEditState(); // #1536 - opening any menu closes any in-flight editor
          closeRepoFlyout(); // #943 - the A->B switch never nulls replicaCtxMenu()
          setReplicaCtxMenu({
            kind: "active",
            sessionId: session.id,
            sessionName: session.name,
            wg,
            replica,
            exited: !isSessionLive(session),
            x: e.clientX,
            y: e.clientY,
          });
          resolveRepoRemotes(replicaRepoMenuEntries(wg, replica)); // #943 - one call per repo path
          const dismiss = (ev?: Event) => {
            if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
            closeReplicaCtxMenu();
          };
          dismissCtx = dismiss;
          setTimeout(() => {
            positionReplicaCtxMenu(e.clientX, e.clientY);
            window.addEventListener("click", dismiss);
            window.addEventListener("contextmenu", dismiss);
            window.addEventListener("keydown", dismiss as any);
          });
        };

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
          resetGroupMenuState();
          resetTitleEditState(); // #1536 - opening any menu closes any in-flight editor
          closeRepoFlyout(); // #943 - the A->B switch never nulls replicaCtxMenu()
          setReplicaCtxMenu({ kind: "inactive", wg, replica, x: e.clientX, y: e.clientY });
          resolveRepoRemotes(replicaRepoMenuEntries(wg, replica)); // #943 - one call per repo path
          const dismiss = (ev?: Event) => {
            if (ev instanceof KeyboardEvent && ev.key !== "Escape") return;
            closeReplicaCtxMenu();
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
          const communication = createMemo(() => session()?.communication ?? null);
          const showRaiseHand = createMemo(() =>
            isCoord() &&
            !!taskTitle &&
            communication()?.kind === "raiseHand" &&
            communication()?.visible === true
          );
          const showBlockedMenu = createMemo(() =>
            communication()?.kind === "blockedMenu" && communication()?.visible === true
          );
          const repoBadges = createMemo(() => {
            const s = session();
            return s && s.gitRepos.length > 0
              ? s.gitRepos
              : configuredReplicaRepoBadgesLive(replica, wg);
          });
          const idleBadge = createMemo(() =>
            isCoord()
              ? coordinatorIdleBadge(
                  effectiveLastUserMessageAt(replica),
                  clockStore.nowMs,
                  settingsStore.current
                )
              : null
          );
          const autoClosed = createMemo(
            () => isCoord() && !!effectiveAutoClosedAt(replica) && !isSessionLive(session())
          );
          const manuallyClosed = createMemo(
            () => isCoord() && !!effectiveManuallyClosedAt(replica) && !isSessionLive(session())
          );
          const idleBadgeTitle = () =>
            "Time this team has been idle. Resets when you message the orchestrator or any member is active (persists across restarts)." +
            (settingsStore.current?.coordinatorAutoCloseEnabled
              ? " The team auto-closes at the configured limit."
              : "");
          const rowTestId = () =>
            `replica.row.${automationIdPart(rowContext)}.${automationIdPart(wg.name)}.${automationIdPart(replica.name)}`;
          const communicationSlotTestId = () => `${rowTestId()}.communicationSlot`;
          const badgesTestId = () =>
            `replica.badges.${automationIdPart(rowContext)}.${automationIdPart(wg.name)}.${automationIdPart(replica.name)}`;
          const repoBadgeTestId = (label: string, index: number) =>
            `replica.repoBadge.${automationIdPart(rowContext)}.${automationIdPart(wg.name)}.${automationIdPart(replica.name)}.${index}.${automationIdPart(label)}`;
          const liveAgentLabel = () => resolveReplicaAgentLabel(session(), replica);
          const profileBadge = () => resolveReplicaProfileBadge(session(), replica);
          const ctxVisible = () =>
            contextBadgeConfigured(settingsStore.current?.agents, session()?.agentId);
          const ctxPercent = () => {
            const s = session();
            return s ? sessionsStore.contextPercentBySessionId[s.id] : undefined;
          };
          const profileBadgeTitle = () => {
            const s = session();
            const cfg = settingsStore.current?.codingAgentProfiles;
            if (!cfg) return undefined;
            if (s) {
              const letter = s.effectiveProfile || s.requestedProfile;
              if (!letter) return undefined;
              return profileDisplayLabel(cfg, settingsStore.current?.agents ?? [], s.agentId, letter);
            }
            const letter = replica.currentProfile;
            if (!letter) return undefined;
            const agentId = replica.currentCodingAgentId ?? replica.preferredAgentId;
            return profileDisplayLabel(cfg, settingsStore.current?.agents ?? [], agentId, letter);
          };
          const isLive = () => isSessionLive(session());
          createEffect(() => {
            const current = session();
            if (current && !isSessionLive(current)) {
              voiceRecorder.revokeSession(current.id);
            }
          });
          const bridge = () => { const s = session(); return s ? bridgesStore.getBridge(s.id) : undefined; };
          const isRecording = () => { const s = session(); return s ? voiceRecorder.recordingSessionId() === s.id : false; };

          const handleCancelRecording = (e: MouseEvent) => {
            e.stopPropagation();
            voiceRecorder.cancel();
          };

          return (
            <div
              class="replica-item"
              classList={{ active: session()?.id === sessionsStore.activeId }}
              data-ac-testid={rowTestId()}
              onClick={() => handleReplicaClick(replica, wg)}
              onContextMenu={(e) => {
                const s = session();
                if (s) {
                  handleReplicaContextMenu(e, s, wg, replica);
                } else {
                  handleReplicaInactiveContextMenu(e, wg, replica);
                }
              }}
              title={replica.path}
            >
              <div class={`session-item-status ${dotClass()}`} />
              <div class="replica-item-info">
                <Show when={taskTitle}>
                  <div class="coord-task-line">
                    <span class="coord-task-title" title={taskTitle ?? undefined}>{taskTitle}</span>
                    <Show when={showRaiseHand()}>
                      <span
                        class="coord-communication-slot"
                        data-kind="raiseHand"
                        data-ac-testid={communicationSlotTestId()}
                        title="Raised hand"
                        aria-label="Raised hand"
                      >
                        <RaiseHandIcon class="coord-communication-icon" />
                      </span>
                    </Show>
                  </div>
                </Show>
                <div class="ac-discovery-badges" data-ac-testid={badgesTestId()}>
                  <Show when={showBlockedMenu()}>
                    <span
                      class="coord-communication-slot coord-communication-slot--blocked-menu"
                      data-kind="blockedMenu"
                      data-ac-testid={communicationSlotTestId()}
                      title={communication()?.message ?? "Interactive menu requires user input"}
                      aria-label="Interactive menu requires user input"
                    >
                      <RaiseHandIcon class="coord-communication-icon" />
                    </span>
                  </Show>
                  {/* #592 - drift indicator for a WG replica session. Mirrors the
                      SessionItem badge: the backend marks profileOutdated in
                      list_sessions when the loaded cell != current config; clicking
                      relaunches via the existing replica restart (re-stamps the hash
                      and clears the flag). stopPropagation keeps the row from
                      selecting the session under the click. */}
                  <Show when={session()?.profileOutdated}>
                    <ProfileOutdatedBadge
                      testId={`replica.outdated.${automationIdPart(rowContext)}.${automationIdPart(wg.name)}.${automationIdPart(replica.name)}`}
                      onReload={() => {
                        const s = session();
                        if (s) void restartReplicaSession(s.id);
                      }}
                    />
                  </Show>
                  {/* #552/#580/#1730: the mutually exclusive trio, positions 2, 3 and 4 of this
                      strip. AUTO-CLOSED and MANUALLY-CLOSED lead it and the coordinator idle
                      (minutes) badge now trails them; before #1730 the idle badge led the whole
                      row. The #580 XOR gate is unchanged: at most one of the three ever renders,
                      and the three stay contiguous, so a closed pill and the counter can never
                      appear together. They no longer lead the strip: the blocked-menu alert slot
                      and the drift badge come first. */}
                  <Show when={autoClosed() && !manuallyClosed()}>
                    <span
                      class="ac-discovery-badge coord-autoclosed"
                      title="This team was auto-closed after inactivity. Reopen it to clear."
                    >
                      AUTO-CLOSED
                    </span>
                  </Show>
                  {/* #588 MANUALLY-CLOSED pill: same coord-autoclosed style as
                      AUTO-CLOSED (pixel-identical, only the label differs). Manual
                      wins the XOR — the AUTO-CLOSED gate above is `&& !manuallyClosed()`. */}
                  <Show when={manuallyClosed()}>
                    <span
                      class="ac-discovery-badge coord-autoclosed"
                      title="This team's orchestrator was closed manually. Reopen it to clear."
                    >
                      MANUALLY-CLOSED
                    </span>
                  </Show>
                  <Show when={!autoClosed() && !manuallyClosed() && idleBadge()}>
                    {(b) => (
                      <span
                        class={`ac-discovery-badge coord-idle ${COORD_IDLE_CLASS[b().level]}`}
                        title={idleBadgeTitle()}
                      >
                        {b().label}
                      </span>
                    )}
                  </Show>
                  <span
                    class="agent-name-chip"
                    title={replica.originProject ? `${replica.name}@${replica.originProject}` : replica.name}
                  >
                    {replica.name}
                  </span>
                  <Show when={isCoord()}>
                    <span class="ac-discovery-badge coord">orchestrator</span>
                  </Show>
                  <Show when={liveAgentLabel()}>
                    <span class="ac-discovery-badge agent">{liveAgentLabel()}</span>
                  </Show>
                  <Show when={profileBadge()}>
                    {(badge) => <span class="profile-badge" title={profileBadgeTitle()}>{badge()}</span>}
                  </Show>
                  <Show when={ctxVisible()}>
                    <ContextBadge
                      percent={ctxPercent()}
                      testId={`replica.contextBadge.${automationIdPart(rowContext)}.${automationIdPart(wg.name)}.${automationIdPart(replica.name)}`}
                    />
                  </Show>
                  <Show when={extraBadge}>
                    <span class="ac-discovery-badge team">{extraBadge}</span>
                  </Show>
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
                          class={`ac-discovery-badge branch${repo.dirty === true ? " dirty" : ""}`}
                          title={formatReplicaRepoBadgeTitle(repo)}
                          data-ac-testid={repoBadgeTestId(repo.label, index())}
                        >
                          {formatReplicaRepoBadgeLabel(repo)}
                        </span>
                      )}
                    </For>
                  </Show>
                </div>
              </div>
              <Show when={isLive()}>
                <Show when={isRecording()}>
                  <button class="session-item-mic-cancel" onClick={handleCancelRecording} title="Cancel recording">&#x2715;</button>
                </Show>
                <Show when={bridge()}>
                  <span
                    class="session-item-bridge-icon"
                    style={{ color: bridge()!.color }}
                    title={`Telegram: ${bridge()!.botLabel}`}
                  >
                    <TelegramIcon />
                  </span>
                </Show>
              </Show>
            </div>
          );
        };

        const renderWorkgroupSubgroup = (wg: AcWorkgroup, rowContext: string) => {
          const wgCollapsedKey = projectPanelCollapseKey(
            proj.path,
            "workgroup",
            workgroupCollapseId(wg, rowContext)
          );
          const wgCollapsed = () => isPanelCollapsed(wgCollapsedKey);
          return (
            <div class="ac-wg-subgroup">
              <div
                class="ac-wg-header ac-wg-header--collapsible"
                title={wg.path}
                onClick={() => togglePanelCollapsed(wgCollapsedKey)}
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
            <div
              class="project-header"
              classList={{ open: filterOpen(), active: filterActive(), invalid: !!filterError() }}
              title={proj.path}
              onContextMenu={handleProjectContextMenu}
            >
              <button
                type="button"
                class="project-header-main"
                title={proj.path}
                aria-expanded={!isProjectPanelCollapsed(proj.path)}
                onClick={() => toggleProjectPanelCollapsed(proj.path)}
              >
                <span class="ac-discovery-chevron" classList={{ collapsed: isProjectPanelCollapsed(proj.path) }}>
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
                    placeholder="room-2.*"
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
                    onClick={() => { setShowCtxMenu(false); setNewAgentTarget({ projectPath: proj.path }); }}
                  >
                    New Agent
                  </button>
                  <button
                    class="session-context-option"
                    onClick={() => { setShowCtxMenu(false); setNewTeamTarget({ projectPath: proj.path }); }}
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
                      setNewWorkgroupTarget({ projectPath: proj.path });
                    }}
                  >
                    New Room
                  </button>
                  <button
                    class="session-context-option"
                    classList={{ "context-option-disabled": !hasLoopTargets() }}
                    disabled={!hasLoopTargets()}
                    onClick={() => {
                      if (!hasLoopTargets()) return;
                      setShowCtxMenu(false);
                      setNewLoopTarget({ projectPath: proj.path });
                    }}
                    data-ac-testid={`loop.action.new.${projectAutomationId()}.projectMenu`}
                  >
                    New Loop
                  </button>
                  <div class="context-separator" />
                  <button
                    class="session-context-option"
                    onClick={() => void handleArchiveProject()}
                    data-ac-testid={`project.action.archive.${projectAutomationId()}`}
                  >
                    Archive Project
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

            {/* #710: entity-creation + edit-loop modals moved out of this row to
                the stable ProjectPanel scope (after the projects <For>) so a
                background refresh that re-creates the row no longer disposes an
                open modal. See the hoisted render blocks below. */}

            <Show when={!isProjectPanelCollapsed(proj.path)}>
              <div class="project-content">
                {/* Coordinators — own collapsible section; shown by styles that enable it via CSS */}
                {(() => {
                  return (
                    <Show when={filteredCoordinatorItems().length > 0}>
                      <div class="coord-quick-access-group">
                        <div
                          class="ac-wg-header ac-wg-header--collapsible"
                          onClick={() => togglePanelCollapsed(coordinatorsCollapsedKey)}
                        >
                          <span class="ac-discovery-chevron" classList={{ collapsed: isPanelCollapsed(coordinatorsCollapsedKey) }}>
                            &#x25BE;
                          </span>
                          <div class="ac-wg-header-text">
                            <span class="ac-wg-name">Orchestrators</span>
                          </div>
                          <span class="ac-team-count">{filteredCoordinatorItems().length}</span>
                        </div>
                        <Show when={!isPanelCollapsed(coordinatorsCollapsedKey)}>
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
                      </div>
                    </Show>
                  );
                })()}
                {/* Selected Workgroup */}
                {(() => {
                  return (
                    <Show when={(sessionsStore.showCategories || sessionsStore.alwaysShowSelectedWorkgroup) && selectedWorkgroupVisible()}>
                      <div class="ac-wg-group">
                        <div
                          class="ac-wg-header ac-wg-header--collapsible"
                          onClick={() => togglePanelCollapsed(selectedWorkgroupCollapsedKey)}
                        >
                          <span class="ac-discovery-chevron" classList={{ collapsed: isPanelCollapsed(selectedWorkgroupCollapsedKey) }}>
                            &#x25BE;
                          </span>
                          <div class="ac-wg-header-text">
                            <span class="ac-wg-name">Selected Room</span>
                          </div>
                          <span class="ac-team-count">{selectedWorkgroup() ? 1 : 0}</span>
                        </div>
                        <Show when={!isPanelCollapsed(selectedWorkgroupCollapsedKey)}>
                          <Show when={selectedWorkgroup()} fallback={<div class="ac-empty-hint">No selected room</div>}>
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
                    <Show when={sessionsStore.showCategories && (!filterActive() || matchesFilterText("Rooms") || filteredWorkgroups().length > 0)}>
                    <div class="ac-wg-group">
                      <div
                        class="ac-wg-header ac-wg-header--collapsible"
                        onClick={() => togglePanelCollapsed(workgroupsCollapsedKey)}
                        onContextMenu={handleWorkgroupsHeaderContextMenu}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: isPanelCollapsed(workgroupsCollapsedKey) }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Rooms</span>
                        </div>
                        <span class="ac-team-count">{filteredWorkgroups().length}</span>
                      </div>
                      <Show when={!isPanelCollapsed(workgroupsCollapsedKey)}>
                        <Show
                          when={filteredWorkgroups().length > 0}
                          fallback={<div class="ac-empty-hint">No rooms</div>}
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
                              setNewWorkgroupTarget({ projectPath: proj.path });
                            }}
                          >
                            New Room
                          </button>
                        </div>
                      </Portal>
                    )}
                    </>
                  );
                })()}
                {/* Loops */}
                {(() => {
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
                        onClick={() => togglePanelCollapsed(loopsCollapsedKey)}
                        onContextMenu={handleLoopsHeaderContextMenu}
                        data-ac-testid={`project.loops.header.${projectAutomationId()}`}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: isPanelCollapsed(loopsCollapsedKey) }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Loops</span>
                        </div>
                        <span class="ac-team-count">{filteredLoops().length}</span>
                      </div>
                      <Show when={!isPanelCollapsed(loopsCollapsedKey)}>
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
                                onClick={() => setEditingLoopTarget({ projectPath: proj.path, loopId: loop.id })}
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
                              setNewLoopTarget({ projectPath: proj.path });
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
                        onClick={() => togglePanelCollapsed(agentsCollapsedKey)}
                        onContextMenu={handleAgentsHeaderContextMenu}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: isPanelCollapsed(agentsCollapsedKey) }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Agents</span>
                        </div>
                      </div>
                      <Show when={!isPanelCollapsed(agentsCollapsedKey)}>
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
                                      extraContextAction={{
                                        label: "Delete",
                                        class: "context-option-danger",
                                        icon: <TrashIcon />,
                                        testId: `agent.action.delete.${automationIdPart(agent.path)}`,
                                        onSelect: () => setDeletingAgent({ name: agent.name, path: agent.path }),
                                      }}
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
                              if (menu) void openMatrixFolder(menu.agent.path);
                            }}
                            title={agentCtxMenu()!.agent.path}
                          >
                            <MatrixFolderIcon /> Open Matrix folder
                          </button>
                          <button
                            class="session-context-option context-option-danger"
                            onClick={() => {
                              const menu = agentCtxMenu();
                              if (menu) setDeletingAgent({ name: menu.agent.name, path: menu.agent.path });
                              setAgentCtxMenu(null);
                            }}
                            data-ac-testid={`agent.action.delete.${automationIdPart(agentCtxMenu()!.agent.path)}`}
                            data-ac-role="menuitem"
                          >
                            <TrashIcon /> Delete
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
                              setNewAgentTarget({ projectPath: proj.path });
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
                          <div
                            class="agent-modal"
                            style={{ "max-width": "360px" }}
                            data-ac-testid={`agent.delete.dialog.${automationIdPart(deletingAgent()!.path)}`}
                          >
                            <div class="agent-modal-header">
                              <span class="agent-modal-title">Delete Agent</span>
                            </div>
                            <div class="new-agent-form">
                              <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
                                Delete agent <strong>{deletingAgent()!.name.slice(deletingAgent()!.name.lastIndexOf("/") + 1)}</strong>? This will remove the agent matrix, its room replicas, and team assignments. This action cannot be undone.
                              </p>
                              <Show when={agentDeleteError()}>
                                <div
                                  class="new-agent-error"
                                  data-ac-testid={`agent.delete.error.${automationIdPart(deletingAgent()!.path)}`}
                                >
                                  {agentDeleteError()}
                                </div>
                              </Show>
                            </div>
                            <div class="new-agent-footer">
                              <button
                                class="new-agent-cancel-btn"
                                onClick={closeAgentDeleteModal}
                                data-ac-testid={`agent.delete.cancel.${automationIdPart(deletingAgent()!.path)}`}
                              >
                                Cancel
                              </button>
                              <button
                                class="new-agent-create-btn"
                                style={{ "background": "var(--danger, #c0392b)" }}
                                disabled={agentDeleteInProgress()}
                                data-ac-testid={`agent.delete.confirm.${automationIdPart(deletingAgent()!.path)}`}
                                onClick={async () => {
                                  if (agentDeleteInProgress()) return;
                                  setAgentDeleteInProgress(true);
                                  const agent = deletingAgent()!;
                                  try {
                                    await EntityAPI.deleteAgentMatrix(proj.path, agent.path);
                                    await projectStore.reloadProject(proj.path);
                                  } catch (e: any) {
                                    console.error("delete_agent_matrix failed:", e);
                                    const msg = typeof e === "string" ? e : e?.message ?? "Failed to delete agent";
                                    if (msg.startsWith(AGENT_DELETE_PARTIAL_REMOVAL_PREFIX)) {
                                      toastStore.info(formatAgentDeletePartialRemovalWarning(msg), { durationMs: null });
                                      closeAgentDeleteModal();
                                      await projectStore.reloadProject(proj.path);
                                      return;
                                    }
                                    if (msg.startsWith("BLOCKERS:")) {
                                      try {
                                        const report = JSON.parse(msg.slice("BLOCKERS:".length)) as BlockerReport;
                                        setAgentDeleteError(formatAgentDeleteBlockerError(report));
                                      } catch (parseErr) {
                                        console.error("Failed to parse BLOCKERS: payload for agent delete:", parseErr);
                                        setAgentDeleteError("Agent is locked, but the blocker report could not be parsed. Try again.");
                                      }
                                    } else {
                                      setAgentDeleteError(msg);
                                    }
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
                        onClick={() => togglePanelCollapsed(teamsCollapsedKey)}
                        onContextMenu={handleTeamsHeaderContextMenu}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: isPanelCollapsed(teamsCollapsedKey) }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Teams</span>
                        </div>
                      </div>
                      <Show when={!isPanelCollapsed(teamsCollapsedKey)}>
                        <Show
                          when={filteredTeams().length > 0}
                          fallback={<div class="ac-empty-hint">No teams</div>}
                        >
                          <For each={filteredTeams()}>
                            {(team) => {
                              const teamCollapsedKey = projectPanelCollapseKey(proj.path, "team", team.name);
                              const teamCollapsed = () => isPanelCollapsed(teamCollapsedKey, true);
                              const visibleTeamMembers = () => filteredTeamMembers(team);
                              return (
                                <div class="ac-team-group">
                                  <div
                                    class="ac-team-header"
                                    onClick={() => togglePanelCollapsed(teamCollapsedKey, true)}
                                    onContextMenu={(e) => handleTeamContextMenu(e, team)}
                                  >
                                    <span class="ac-discovery-chevron" classList={{ collapsed: teamCollapsed() }}>
                                      &#x25BE;
                                    </span>
                                    <span class="ac-team-name">{team.name}</span>
                                    <span class="ac-team-count">{visibleTeamMembers().length}</span>
                                  </div>
                                  <Show when={!teamCollapsed()}>
                                    <div class="ac-team-members">
                                      <For each={visibleTeamMembers()}>
                                        {(agentName) => {
                                          return (
                                            <div class="ac-team-member" title={agentName}>
                                              <span class="ac-team-member-name">{teamMemberDisplayLabel(agentName)}</span>
                                              <Show when={agentName === team.coordinator}>
                                                <span class="ac-discovery-badge coord">orchestrator</span>
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
                              setNewTeamTarget({ projectPath: proj.path });
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
                      if (menu) {
                        setEditingTeamTarget({
                          projectPath: proj.path,
                          teamName: menu.team.name,
                        });
                      }
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
                    Delete Room
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
                      if (menu) setEditingLoopTarget({ projectPath: proj.path, loopId: menu.loop.id });
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
                  onMouseEnter={cancelReplicaCtxMenuClose}
                  onMouseLeave={() => {
                    scheduleRepoFlyoutClose();
                    // #1536 - the editor keeps the menu open while the user is
                    // interacting with it (mirrors the groupErrorPinned guard
                    // inside scheduleReplicaCtxMenuClose).
                    if (titleEdit()) {
                      cancelReplicaCtxMenuClose();
                      return;
                    }
                    scheduleReplicaCtxMenuClose(); // #977
                  }}
                >
                  <Show when={activeReplicaMenu()}>
                    {(menu) => {
                      const broomDisabled = () => isTaskClean(menu().wg.taskTitle);
                      const broomTitle = () =>
                        broomDisabled() ? "Nothing to clear" : "Clear task title";
                      const matrixFolder = () => replicaMatrixFolder(menu().replica);
                      const repoEntries = () => replicaRepoMenuEntries(menu().wg, menu().replica);
                      const liveTelegramSession = () => {
                        const session = activeReplicaMenuSession();
                        return session && isSessionLive(session) ? session : null;
                      };
                      const telegramBridge = () => activeReplicaMenuBridge();
                      const telegramChoices = () => {
                        const choices = replicaTelegramBotMenu();
                        const session = liveTelegramSession();
                        const currentMenu = activeReplicaMenu();
                        if (
                          !choices ||
                          choices.epoch !== replicaCtxMenuEpoch ||
                          choices.sessionId !== menu().sessionId ||
                          currentMenu?.sessionId !== choices.sessionId ||
                          !session ||
                          telegramBridge()
                        ) {
                          return null;
                        }
                        return choices;
                      };
                      return (
                      <>
                        <button
                          class="session-context-option context-option-danger"
                          onClick={async () => {
                            await restartReplicaSession(menu().sessionId);
                          }}
                        >
                          <span class="session-context-option-icon" aria-hidden="true">&#x21BA;</span> Restart Session
                        </button>
                        <button
                          class="session-context-option"
                          onClick={() => {
                            const sessionId = menu().sessionId;
                            const sessionName = menu().sessionName;
                            closeReplicaCtxMenu();
                            setReplicaCodingAgentTarget({ sessionId, sessionName });
                          }}
                        >
                          <span class="session-context-option-icon" aria-hidden="true">&#x1F916;</span> Coding Agent
                        </button>
                        <button
                          class="session-context-option"
                          title={menu().replica.path}
                          onClick={() => void openReplicaFolder(menu().replica.path)}
                        >
                          <span class="session-context-option-icon" aria-hidden="true">&#x1F4C2;</span> Open Replica's Folder
                        </button>
                        {renderRepoMenuEntries(repoEntries, () => `replica.${menu().sessionId}.menu.repo`)}
                        <Show when={matrixFolder()}>
                          {(path) => (
                            <button
                              class="session-context-option"
                              title={path()}
                              onClick={() => void openMatrixFolder(path())}
                            >
                              <span class="session-context-option-icon" aria-hidden="true"><MatrixFolderIcon /></span> Open Matrix folder
                            </button>
                          )}
                        </Show>
                        <Show when={activeReplicaMenuSession()}>
                          <button
                            class="session-context-option context-option-danger"
                            onClick={(event) =>
                              handleReplicaContextClose(event, menu().sessionId)
                            }
                          >
                            <span class="session-context-option-icon" aria-hidden="true">&#x2715;</span> Close Session
                          </button>
                        </Show>
                        <div class="context-separator" />
                        <button
                          class="session-context-option"
                          onClick={() => toggleReplicaDetach(menu().sessionId)}
                        >
                          {/* #987, #1708 - the same icon pair the session row's detach button uses. */}
                          <span class="session-context-option-icon" aria-hidden="true">
                            {sessionsStore.isDetached(menu().sessionId) ? (
                              <ReattachIcon class="session-context-detach-icon" />
                            ) : (
                              <DetachIcon class="session-context-detach-icon" />
                            )}
                          </span>{" "}
                          {sessionsStore.isDetached(menu().sessionId) ? "Re-attach session" : "Detach session"}
                        </button>
                        <Show when={liveTelegramSession()}>
                          <button
                            class="session-context-option"
                            onClick={(event) =>
                              void handleReplicaTelegramAction(event, menu().sessionId)
                            }
                          >
                            <span
                              class="session-context-option-icon"
                              aria-hidden="true"
                              style={telegramBridge() ? { color: telegramBridge()!.color } : { color: "#0088cc" }}
                            >
                              <TelegramIcon />
                            </span>{" "}
                            {telegramBridge() ? "Detach Telegram" : "Attach Telegram"}
                          </button>
                        </Show>
                        <Show when={telegramChoices()}>
                          {(choices) => (
                            <For each={choices().bots}>
                              {(bot) => (
                                <button
                                  class="session-context-option"
                                  onClick={(event) =>
                                    void handleReplicaTelegramBotSelect(
                                      event,
                                      choices().sessionId,
                                      bot.id,
                                      choices().epoch,
                                    )
                                  }
                                >
                                  <span class="session-context-option-icon" aria-hidden="true">
                                    <span
                                      class="settings-color-dot"
                                      style={{ background: bot.color }}
                                    />
                                  </span>{" "}
                                  {bot.label}
                                </button>
                              )}
                            </For>
                          )}
                        </Show>
                        {renderAddToGroupItem(menu().wg, menu().replica)}
                        <div class="context-separator" />
                        <button
                          class="session-context-option"
                          title="Edit TASK title"
                          onClick={(e) => {
                            e.stopPropagation();
                            void startReplicaTitleEdit(menu().wg);
                          }}
                        >
                          <span class="session-context-option-icon session-context-task-icon" aria-hidden="true">&#x270E;</span> Edit TASK title
                        </button>
                        <Show when={titleEdit() && titleEdit()!.wgPath === menu().wg.path}>
                          <div
                            class="session-context-title-edit"
                            onClick={(e) => e.stopPropagation()}
                          >
                            <input
                              ref={(el) => requestAnimationFrame(() => { el.focus(); el.select(); })}
                              class="session-context-title-input"
                              value={titleDraft()}
                              onInput={(e) => setTitleDraft(e.currentTarget.value)}
                              onKeyDown={(e) => {
                                // Strictly required: keydown is not covered by the
                                // container's onClick, and the window keydown dismiss
                                // fires on Escape. Escape must cancel the editor, not
                                // close the whole menu.
                                e.stopPropagation();
                                if (e.key === "Enter") {
                                  e.preventDefault();
                                  if (!titleBusy()) void saveReplicaTitle();
                                } else if (e.key === "Escape") {
                                  e.preventDefault();
                                  cancelReplicaTitleEdit();
                                }
                              }}
                              placeholder="Title"
                              disabled={titleBusy()}
                            />
                            <button
                              class="session-context-title-btn save"
                              onClick={(e) => { e.stopPropagation(); void saveReplicaTitle(); }}
                              disabled={titleBusy() || !titleDraft().trim()}
                              type="button"
                            >
                              Save
                            </button>
                            <button
                              class="session-context-title-btn cancel"
                              onClick={(e) => { e.stopPropagation(); cancelReplicaTitleEdit(); }}
                              disabled={titleBusy()}
                              type="button"
                            >
                              Cancel
                            </button>
                          </div>
                        </Show>
                        <Show when={titleError()}>
                          <div class="session-context-title-error">{titleError()}</div>
                        </Show>
                        <button
                          class="session-context-option"
                          classList={{ "context-option-disabled": broomDisabled() }}
                          disabled={broomDisabled()}
                          title={broomTitle()}
                          onClick={() => void clearReplicaTaskTitle(menu().wg)}
                        >
                          <span class="session-context-option-icon" aria-hidden="true">&#x1F9F9;</span> Clear task title
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
                      const matrixFolder = () => replicaMatrixFolder(menu().replica);
                      const repoEntries = () => replicaRepoMenuEntries(menu().wg, menu().replica);
                      return (
                        <>
                          <button
                            class="session-context-option"
                            onClick={() => {
                              const wg = menu().wg;
                              const replica = menu().replica;
                              closeReplicaCtxMenu();
                              setInactiveCodingAgentTarget({
                                projectPath: proj.path,
                                wgPath: wg.path,
                                replicaPath: replica.path,
                              });
                            }}
                          >
                            <span class="session-context-option-icon" aria-hidden="true">&#x1F916;</span> Coding Agent
                          </button>
                          <button
                            class="session-context-option"
                            title={menu().replica.path}
                            onClick={() => void openReplicaFolder(menu().replica.path)}
                          >
                            <span class="session-context-option-icon" aria-hidden="true">&#x1F4C2;</span> Open Replica's Folder
                          </button>
                          {renderRepoMenuEntries(repoEntries, () => "replica.inactive.menu.repo")}
                          <Show when={matrixFolder()}>
                            {(path) => (
                              <button
                                class="session-context-option"
                                title={path()}
                                onClick={() => void openMatrixFolder(path())}
                              >
                                <span class="session-context-option-icon" aria-hidden="true"><MatrixFolderIcon /></span> Open Matrix folder
                              </button>
                            )}
                          </Show>
                          {renderAddToGroupItem(menu().wg, menu().replica)}
                          <button
                            class="session-context-option"
                            title="Edit TASK title"
                            onClick={(e) => {
                              e.stopPropagation();
                              void startReplicaTitleEdit(menu().wg);
                            }}
                          >
                            <span class="session-context-option-icon session-context-task-icon" aria-hidden="true">&#x270E;</span> Edit TASK title
                          </button>
                          <Show when={titleEdit() && titleEdit()!.wgPath === menu().wg.path}>
                            <div
                              class="session-context-title-edit"
                              onClick={(e) => e.stopPropagation()}
                            >
                              <input
                                ref={(el) => requestAnimationFrame(() => { el.focus(); el.select(); })}
                                class="session-context-title-input"
                                value={titleDraft()}
                                onInput={(e) => setTitleDraft(e.currentTarget.value)}
                                onKeyDown={(e) => {
                                  // Strictly required: keydown is not covered by the
                                  // container's onClick, and the window keydown dismiss
                                  // fires on Escape. Escape must cancel the editor, not
                                  // close the whole menu.
                                  e.stopPropagation();
                                  if (e.key === "Enter") {
                                    e.preventDefault();
                                    if (!titleBusy()) void saveReplicaTitle();
                                  } else if (e.key === "Escape") {
                                    e.preventDefault();
                                    cancelReplicaTitleEdit();
                                  }
                                }}
                                placeholder="Title"
                                disabled={titleBusy()}
                              />
                              <button
                                class="session-context-title-btn save"
                                onClick={(e) => { e.stopPropagation(); void saveReplicaTitle(); }}
                                disabled={titleBusy() || !titleDraft().trim()}
                                type="button"
                              >
                                Save
                              </button>
                              <button
                                class="session-context-title-btn cancel"
                                onClick={(e) => { e.stopPropagation(); cancelReplicaTitleEdit(); }}
                                disabled={titleBusy()}
                                type="button"
                              >
                                Cancel
                              </button>
                            </div>
                          </Show>
                          <Show when={titleError()}>
                            <div class="session-context-title-error">{titleError()}</div>
                          </Show>
                          <button
                            class="session-context-option"
                            classList={{ "context-option-disabled": broomDisabled() }}
                            disabled={broomDisabled()}
                            title={broomTitle()}
                            onClick={() => void clearReplicaTaskTitle(menu().wg)}
                          >
                            <span class="session-context-option-icon" aria-hidden="true">&#x1F9F9;</span> Clear task title
                          </button>
                        </>
                      );
                    }}
                  </Show>
                </div>
              </Portal>
            )}
            {/* #710: live + inactive Coding Agent pickers moved out of this row
                to the stable ProjectPanel scope (after the projects <For>) so a
                background refresh that re-creates the row no longer disposes an
                open picker. See the hoisted render blocks below. */}

            {/* Delete WG confirmation */}
            {deletingWg() && (
              <Portal>
                <div class="modal-overlay">
                  <div class="agent-modal" style={{ "max-width": "360px" }}>
                    <div class="agent-modal-header">
                      <span class="agent-modal-title">Delete Room</span>
                    </div>
                    <div class="new-agent-form">
                      <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
                        Delete room <strong>{deletingWg()!.name}</strong>? This will remove the room directory and all its contents. This action cannot be undone.
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
                              <strong>Cannot delete:</strong> Windows reported the room is locked.
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
                                fallback={<div style={{ "margin-top": "8px" }}>Close any app that may be using files in this room, then click <strong>Retry</strong> below.</div>}
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
                            const msg = typeof e === "string" ? e : e?.message ?? "Failed to delete room";
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
                                setWgDeleteError("Room is locked, but the blocker report could not be parsed. Try again.");
                                setWgDeleteInProgress(false);
                                return;
                              }
                            }
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
                        Delete team <strong>{deletingTeam()!.name}</strong>? This will remove the team configuration and all associated rooms. This action cannot be undone.
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

    {/* #710: entity-creation / edit-loop modals + coding-agent pickers hoisted
        out of the projects <For> so a background discovery refresh (which
        replaces row objects and disposes their local signals) cannot tear them
        down mid-interaction. Each resolves its live project / loop / session
        data from the stable identity carried in its target signal. Mirrors the
        editingTeamTarget / pendingLaunch precedent below. */}
    {/* Gated on findProjectByPath (like the New Workgroup / New Loop / Edit Loop
        blocks below) — not just the raw target signal — so the modal auto-closes
        if its project is removed, matching the pre-#710 row-disposal behavior and
        keeping all five entity-creation modals consistent. */}
    <Show when={findProjectByPath(newAgentTarget()?.projectPath)}>
      {(proj) => (
        <Portal>
          <NewEntityAgentModal
            projectPath={proj().path}
            onClose={() => setNewAgentTarget(null)}
          />
        </Portal>
      )}
    </Show>
    <Show when={findProjectByPath(newTeamTarget()?.projectPath)}>
      {(proj) => (
        <Portal>
          <NewTeamModal
            projectPath={proj().path}
            onClose={() => setNewTeamTarget(null)}
          />
        </Portal>
      )}
    </Show>
    <Show when={findProjectByPath(newWorkgroupTarget()?.projectPath)}>
      {(proj) => (
        <Portal>
          <NewWorkgroupModal
            projectPath={proj().path}
            teams={proj().teams}
            onClose={() => setNewWorkgroupTarget(null)}
          />
        </Portal>
      )}
    </Show>
    <Show when={findProjectByPath(newLoopTarget()?.projectPath)}>
      {(proj) => (
        <Portal>
          <NewLoopModal
            projectPath={proj().path}
            workgroups={proj().workgroups}
            onClose={() => setNewLoopTarget(null)}
          />
        </Portal>
      )}
    </Show>
    <Show when={editingLoopResolved()}>
      {(resolved) => (
        <Portal>
          <EditLoopModal
            projectPath={resolved().proj.path}
            workgroups={resolved().proj.workgroups}
            loop={resolved().loop}
            onClose={() => setEditingLoopTarget(null)}
          />
        </Portal>
      )}
    </Show>
    {/* Coding Agent picker for a live (green/red) replica session. Resolves all
        session data live by stable sessionId, so a refresh updates it in place. */}
    {replicaCodingAgentTarget() && (
      <Portal>
        <AgentPickerModal
          sessionName={replicaCodingAgentTarget()!.sessionName}
          agentPath={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.workingDirectory}
          currentAgentId={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.agentId}
          explicitCurrentAgentId={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.agentId}
          currentRequestedProfile={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.requestedProfile}
          scopeContext={deriveScopeContextFromSession(
            sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId),
            replicaCodingAgentTarget()!.sessionName,
          )}
          disableRedundantReplicaAssign
          targetProfileOutdated={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.profileOutdated}
          onSelect={async (selection) => {
            const target = replicaCodingAgentTarget();
            setReplicaCodingAgentTarget(null);
            if (!target) return;
            const session = sessionsStore.sessions.find((s) => s.id === target.sessionId);
            if (!isWgReplicaPath(session?.workingDirectory)) {
              await restartReplicaSessionCore(
                target.sessionId,
                selection.agent.id,
                selection.requestedProfile,
              );
              return;
            }
            if (shouldOfferRestartAfterAssign(selection, session)) {
              const slash = target.sessionName.lastIndexOf("/");
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
    {/* Coding Agent picker for a gray/red (not-running) replica — pick what
        launches before first launch / relaunch, without starting the agent
        (#545). For a WG replica the picker writes the selection through the
        backend. #710: the live wg/replica are re-resolved by path so a refresh
        keeps the picker open with fresh data. */}
    <Show when={inactiveCodingAgentResolved()}>
      {(resolved) => (
        <Portal>
          <AgentPickerModal
            sessionName={replicaSessionName(resolved().wg, resolved().replica)}
            agentPath={resolved().replica.path}
            currentAgentId={resolved().replica.currentCodingAgentId ?? resolved().replica.preferredAgentId}
            explicitCurrentAgentId={resolved().replica.currentCodingAgentId ?? null}
            currentRequestedProfile={resolved().replica.currentProfile ?? null}
            scopeContext={replicaScopeContext(resolved().wg, resolved().replica)}
            disableRedundantReplicaAssign
            onSelect={async () => {
              const projectPath = resolved().proj.path;
              setInactiveCodingAgentTarget(null);
              await projectStore.reloadProject(projectPath);
            }}
            onClose={() => setInactiveCodingAgentTarget(null)}
          />
        </Portal>
      )}
    </Show>

    {/* #669: hoisted out of the project row so project refreshes that replace
        row objects do not dispose the edit modal or its unsaved local state. */}
    {editingTeamTarget() && (
      <Portal>
        <EditTeamModal
          projectPath={editingTeamTarget()!.projectPath}
          teamName={editingTeamTarget()!.teamName}
          onClose={closeEditTeamModal}
        />
      </Portal>
    )}

    {/* #588 Coordinator manual-close confirmation. Hoisted to the stable
        ProjectPanel root (outside the projects <For>, like pendingLaunch) so a
        background discovery refresh that re-creates each <For> row cannot tear it
        down mid-decision. Driven by the module-level pendingCoordinatorClose
        signal set by requestCoordinatorClose(ById) when the team is busy. */}
    {pendingCoordinatorClose() && (
      <Portal>
        <div class="modal-overlay" data-ac-testid="coordinatorClose.modal">
          <div class="agent-modal" style={{ "max-width": "380px" }}>
            <div class="agent-modal-header">
              <span class="agent-modal-title">Close orchestrator?</span>
            </div>
            <div class="new-agent-form">
              <p style={{ margin: "0", "line-height": "1.5", opacity: 0.85 }}>
                <strong>{pendingCoordinatorClose()!.workingCount}</strong> team agent
                {pendingCoordinatorClose()!.workingCount === 1 ? " is" : "s are"} still working.
                Closing <strong>{pendingCoordinatorClose()!.name}</strong> will also close the team.
              </p>
            </div>
            <div class="new-agent-footer">
              <button
                class="new-agent-cancel-btn"
                data-ac-testid="coordinatorClose.cancel"
                onClick={() => setPendingCoordinatorClose(null)}
              >
                Cancel
              </button>
              <button
                class="new-agent-create-btn"
                style={{ "background": "var(--danger, #c0392b)" }}
                data-ac-testid="coordinatorClose.confirm"
                onClick={() => void confirmPendingCoordinatorClose()}
              >
                Close team
              </button>
            </div>
          </div>
        </div>
      </Portal>
    )}

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
              skipAutoResume: pending.resumeOnLaunch ? false : undefined,
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
