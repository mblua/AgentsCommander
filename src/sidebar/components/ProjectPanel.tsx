import { Component, For, Show, createEffect, createMemo, createSignal, onMount, onCleanup } from "solid-js";
import { Portal } from "solid-js/web";
import type { AcWorkgroup, AcAgentReplica, AcTeam, AcLoopSummary, Session, SessionRepo, TelegramBotConfig, BlockerReport } from "../../shared/types";
import { SessionAPI, WindowAPI, EntityAPI, LoopAPI, TelegramAPI, SettingsAPI, TaskAPI, onDiscoveryBranchUpdated, onCoordinatorClockUpdated, onCoordinatorAutoCloseChanged, onCoordinatorManualCloseChanged, emitOpenSettings } from "../../shared/ipc";
import type { SessionRepoInput } from "../../shared/ipc";
import {
  pendingCoordinatorClose,
  setPendingCoordinatorClose,
  confirmPendingCoordinatorClose,
  requestCoordinatorClose,
  registerCoordinatorCloseModalHost,
} from "../stores/coordinator-close";
import { isTauri } from "../../shared/platform";
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
  replicaVolatileStore,
} from "../stores/replica-volatile";
import { normalizeProjectPathForCompare } from "../stores/project-refresh";
import {
  projectCollapseStore,
  projectPanelCollapseKey,
  PROJECT_PANEL_COLLAPSE_KEY_SEP,
} from "../stores/project-collapse";
// #810 - PROJECT_PANEL_COLLAPSE_KEY_SEP, ProjectPanelCollapseSection, and
// projectPanelCollapseKey moved to the project-collapse store (canonical
// home). Re-exported here so any external importer that previously read them
// from ProjectPanel keeps compiling.
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
import SessionItem from "./SessionItem";
import ProfileOutdatedBadge from "./ProfileOutdatedBadge";
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
import { sessionDotClass } from "./session-status";
import {
  findReplicaSession as replicaSession,
  replicaDotClass,
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
  /** #599 R1: reopening a coordinator destroyed by auto/manual close — resume. */
  resumeOnLaunch?: boolean;
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

function sessionEffectiveStatusSearchText(session: Session): string {
  const dotClass = sessionDotClass(session);
  return dotClass === "exited" ? sessionStatusSearchText(session.status) : dotClass;
}

/** Build the gitRepos list for a replica. Order = replica.repoPaths order (invariant §3.1.2). */
function buildGitRepos(replica: AcAgentReplica): SessionRepoInput[] {
  return (replica.repoPaths ?? []).map((p) => {
    return { label: repoLabelFromPath(p), sourcePath: p };
  });
}

function hasValidRepoSourcePath(repo: Pick<SessionRepo, "sourcePath">): boolean {
  return typeof repo.sourcePath === "string" && repo.sourcePath.trim().length > 0;
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

function workgroupCollapseId(wg: AcWorkgroup, rowContext: string): string {
  return [
    rowContext,
    normalizeProjectPathForCompare(wg.path || wg.name),
  ].join(PROJECT_PANEL_COLLAPSE_KEY_SEP);
}

/**
 * #573 (grinch Step-7): upper bound for the post-assign restart await. The Tauri
 * IPC transport (`transport-tauri.ts`) has NO timeout, unlike `WsTransport.invoke`
 * (`transport-ws.ts`), which rejects after 30s with `Command timeout: <cmd>`. If
 * the backend `restart_session` neither resolves nor rejects (session-manager
 * write-lock stall, ConPTY respawn hang, dropped IPC reply), `restarting()` would
 * stay true forever and trap the modal (both buttons disabled + dismiss gated →
 * app-kill required). Racing this timeout lets the desktop modal self-heal exactly
 * as it already does on WS/remote — intentional parity, so mirror the WS value.
 * The WS 30s is a bare literal there (not exported), hence a local named const.
 */
export const RESTART_TIMEOUT_MS = 30_000;

/** #748 — resolve the repo badges' branch through the volatile live layer so a
 *  branch event updates badge text without touching row identity. */
function configuredReplicaRepoBadgesLive(
  replica: AcAgentReplica,
  workgroup: Pick<AcWorkgroup, "repoPath">
): SessionRepo[] {
  return configuredReplicaRepoBadges(
    { repoPaths: replica.repoPaths, repoBranch: effectiveRepoBranch(replica) },
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

/** Check if a session has a live PTY process (not exited, not offline) */
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
  //
  // #748 — these four events write to replicaVolatileStore, NOT projectStore.
  // The old in-place patches rebuilt every project/workgroup reference, and the
  // reference-keyed <For>s below then re-created the whole panel DOM per event
  // — losing any click whose press straddled the swap. The volatile store is
  // fine-grained (keyed by normalized replica path), so only the badge/pill
  // reading the changed field re-runs; row identity is stable.
  let unlistenBranch: (() => void) | null = null;
  let unlistenClock: (() => void) | null = null;
  let unlistenAutoClose: (() => void) | null = null;
  let unlistenManualClose: (() => void) | null = null;
  // #588 register this ProjectPanel as the confirm-modal host for THIS window, so
  // the shared close helper opens the modal here (sidebar / web) but falls back to
  // a plain destroy in a window with no host (the detached terminal webview).
  onCleanup(registerCoordinatorCloseModalHost());
  onMount(async () => {
    unlistenBranch = await onDiscoveryBranchUpdated((data) => {
      replicaVolatileStore.setRepoBranch(data.replicaPath, data.branch);
    });
    // #552 coordinator idle badge + auto-closed pill. Discovery reload
    // supersedes these overrides on any path miss (clearForPaths).
    unlistenClock = await onCoordinatorClockUpdated((data) => {
      replicaVolatileStore.setLastUserMessageAt(data.replicaPath, data.lastUserMessageAt);
    });
    unlistenAutoClose = await onCoordinatorAutoCloseChanged((data) => {
      replicaVolatileStore.setAutoClosedAt(data.replicaPath, data.autoClosedAt);
    });
    // #588 manually-closed pill: same live layer as the auto-close marker.
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

  // #810 - project-level collapse lives in projectCollapseStore so the rail
  // can drive auto-focus (collapse others, expand owner, scroll owner into
  // view). Sub-section keys stay in the local collapsedByKey signal above.
  const isProjectPanelCollapsed = (projectPath: string) =>
    projectCollapseStore.isProjectCollapsed(projectPath);
  const toggleProjectPanelCollapsed = (projectPath: string) =>
    projectCollapseStore.toggleProjectCollapsed(projectPath);

  // #810 (grinch F1) - ref-Map of rendered .project-header elements keyed by
  // the NORMALIZED project path. Lives in the stable ProjectPanel scope (not
  // per-row) so it is not disposed mid-focus. A row's ref callback writes its
  // header here on mount and clears it on cleanup. Replaces the original
  // CSS-attribute-selector approach, which silently no-oped on Windows
  // backslash paths because CSS string tokens consume `\` as an escape char.
  const projectHeaderEls = new Map<string, HTMLElement>();
  const registerProjectHeader = (projectPath: string, el: HTMLElement | null) => {
    const key = normalizeProjectPathForCompare(projectPath);
    if (el) {
      projectHeaderEls.set(key, el);
    } else {
      // Only delete if the registered entry is still the same el; a stale
      // ref from a disposed row may have already been overwritten by the
      // new row's mount.
      if (projectHeaderEls.get(key) === el) {
        projectHeaderEls.delete(key);
      }
    }
  };

  // #810 - one-shot focus: scroll the owner project header into view when
  // the rail requests it. block:"nearest" so an already-visible owner does
  // not jump. The target from the store is already NORMALIZED; the ref-Map
  // is keyed on the same normalized form, so map.get(target) resolves
  // without any CSS selector. Grinch F6: capture target before deferring to
  // the microtask and only consume the focus if the live signal still
  // equals the captured value, so two fast clicks (A then B) cannot have
  // microtask-A clear B's pending target.
  createEffect(() => {
    const target = projectCollapseStore.focusTarget();
    if (!target) return;
    // Defer to next microtask so the expand (setProjectCollapsed false)
    // applied by the rail onClick has propagated and the header/body are
    // mounted before we scroll.
    queueMicrotask(() => {
      const live = projectCollapseStore.focusTarget();
      if (live !== target) return;
      const el = projectHeaderEls.get(target);
      el?.scrollIntoView({ block: "nearest", behavior: "smooth" });
      projectCollapseStore.consumeProjectFocus();
    });
  });

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
  const [editingTeamTarget, setEditingTeamTarget] = createSignal<{
    projectPath: string;
    teamName: string;
  } | null>(null);
  const closeEditTeamModal = () => setEditingTeamTarget(null);
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
    // #573 (grinch Step-7): bound the await with RESTART_TIMEOUT_MS. The Tauri IPC
    // transport never times out, so a wedged backend would leave `restarting()`
    // true forever and trap the modal (buttons disabled + dismiss gated). Racing a
    // timeout guarantees `finally` runs within the bound, surfacing the error
    // inline and re-enabling the modal — matching WsTransport.invoke's self-heal.
    let timeoutTimer: number | undefined;
    try {
      await Promise.race([
        SessionAPI.restart(prompt.sessionId, {
          agentId: prompt.agentId,
          requestedProfile: prompt.requestedProfile,
        }),
        // Mirror WsTransport.invoke's reject (a bare `Command timeout: <cmd>`
        // string) so launchErrorMessage yields identical copy on desktop and WS.
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
      // Timer hygiene: cancel the pending timeout when the restart settles first,
      // so a successful restart can't leave a dangling timer / late rejection.
      window.clearTimeout(timeoutTimer);
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

  // #710: modal-open state hoisted OUT of the projects <For>. A background
  // discovery refresh (reloadProject / branch / clock events) replaces each
  // project object reference, so SolidJS disposes and re-creates every <For>
  // row — and with it any signal declared inside the row callback. A modal
  // whose open-flag lived on the row was therefore torn down mid-interaction
  // (#710, same bug class as #537/#669). These signals live at the stable
  // ProjectPanel scope (like pendingLaunch / restartPrompt / editingTeamTarget)
  // and carry only STABLE identities (projectPath, loop id, session id,
  // wg/replica path); the render blocks after the <For> re-resolve the live
  // project/session/loop data from them, so a refresh updates the modal's data
  // without unmounting it.
  const [newAgentTarget, setNewAgentTarget] = createSignal<{ projectPath: string } | null>(null);
  const [newTeamTarget, setNewTeamTarget] = createSignal<{ projectPath: string } | null>(null);
  const [newWorkgroupTarget, setNewWorkgroupTarget] = createSignal<{ projectPath: string } | null>(null);
  const [newLoopTarget, setNewLoopTarget] = createSignal<{ projectPath: string } | null>(null);
  const [editingLoopTarget, setEditingLoopTarget] = createSignal<{ projectPath: string; loopId: string } | null>(null);
  const [replicaCodingAgentTarget, setReplicaCodingAgentTarget] = createSignal<{ sessionId: string; sessionName: string } | null>(null);
  // Coding Agent picker target for a gray/red (not-running) replica — #545.
  // Carries stable paths (#710); the render block re-resolves the live wg/replica
  // so a refresh that replaces those object refs keeps the picker open.
  const [inactiveCodingAgentTarget, setInactiveCodingAgentTarget] = createSignal<{ projectPath: string; wgPath: string; replicaPath: string } | null>(null);

  // Resolve the live ProjectState for a hoisted modal from the stable path it
  // carries. Every projectStore mutation maps over `projects` keyed by path
  // (project.ts), so the entry stays findable across refreshes; this returns
  // undefined only once the project is actually removed, which collapses the
  // dependent modal (its <Show> turns falsy) — the correct "project gone" close.
  const findProjectByPath = (projectPath: string | null | undefined) => {
    if (!projectPath) return undefined;
    const normalized = normalizeProjectPathForCompare(projectPath);
    return projectStore.projects.find(
      (p) => normalizeProjectPathForCompare(p.path) === normalized,
    );
  };

  // #710: re-resolve the live workgroup + replica for the inactive coding-agent
  // picker from the stable identities in the target. A discovery refresh fully
  // replaces the workgroups/agents arrays (project.ts reloadProject), so the old
  // {wg, replica} object refs went stale and the row disposal closed the modal;
  // matching by path re-finds them. Null (project/wg/replica gone) closes it.
  const inactiveCodingAgentResolved = createMemo(() => {
    const target = inactiveCodingAgentTarget();
    if (!target) return null;
    const proj = findProjectByPath(target.projectPath);
    const wg = proj?.workgroups.find((w) => w.path === target.wgPath);
    const replica = wg?.agents.find((r) => r.path === target.replicaPath);
    return proj && wg && replica ? { proj, wg, replica } : null;
  });

  // #710: re-resolve the live loop for the edit-loop modal by id, so a refresh
  // (which replaces the loops array) feeds the modal fresh data instead of
  // disposing it. Null (loop deleted) closes the modal.
  const editingLoopResolved = createMemo(() => {
    const target = editingLoopTarget();
    if (!target) return null;
    const proj = findProjectByPath(target.projectPath);
    const loop = proj?.loops.find((l) => l.id === target.loopId);
    return proj && loop ? { proj, loop } : null;
  });

  // #710: top-level restart helper for the hoisted coding-agent picker's
  // onSelect. Same bounded restart as the per-row restartReplicaSession below,
  // minus the row-local context-menu cleanup (the menu is already gone by the
  // time the picker resolves), so it can live at the stable ProjectPanel scope.
  // #574 §15.1: bound the await with RESTART_TIMEOUT_MS, mirroring
  // applyRestartPrompt. The Tauri IPC transport has no client-side timeout
  // (transport-tauri.ts), so a wedged backend would never settle this await, the
  // catch would never run, and NO toast would fire on desktop — the exact #574
  // silent-failure class. WsTransport self-heals after 30s; this gives desktop
  // the same guarantee. The bare "Command timeout: restart_session" reject
  // string passes through launchErrorMessage verbatim, so desktop and WS get
  // identical copy.
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
      // Cancel the pending timeout when the restart settles first, so a
      // successful restart can't leave a dangling timer / late rejection.
      window.clearTimeout(timeoutTimer);
    }
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

    // Not instantiated — create session in-place.
    // #599 R1: a coordinator that was auto-closed (#552/#580) or manually closed
    // (#588) is DESTROYED, so its reopen has no live/exited record and lands here
    // instead of the restart-with-resume branch above. Both close markers are set
    // only when AC tore down a coordinator that had been running, so their
    // presence is the discriminator "reopen of an already-run replica" vs
    // "genuinely fresh first launch". Carry a resume intent so the eventual
    // create injects --continue (Claude still disk-gates it via claude_project_exists).
    // #748: read through the volatile layer — a close event that has not been
    // through a discovery reload yet lives only there.
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
        // #710: New Agent/Team/Workgroup/Loop + Edit Loop open-state hoisted to
        // the stable ProjectPanel scope (setNewAgentTarget/…/setEditingLoopTarget
        // above) so a background refresh that re-creates this row can't dispose an
        // open modal. Triggers below pass proj.path (and loop id) into them.
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
        // #710: live + inactive Coding Agent picker open-state hoisted to the
        // stable ProjectPanel scope (replicaCodingAgentTarget /
        // inactiveCodingAgentTarget above). The triggers below set them with
        // stable identities (session id; project/wg/replica paths).
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
          if (filterInputEl) focusOnMount(filterInputEl, { select: true });
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
        // Search text for the session fields visible on EVERY row that shows this
        // session (shared by replica rows and agent SessionItems): name, agent
        // label, and the status dot's state. Conditionally-rendered badges are
        // contributed by the per-row callers under the gate that matches their
        // render — repo/branch (replicaSearchText / sessionRepoSearchText) and the
        // profile badge — so "what you match == what you see" (#515 bug 1).
        // `shell` is dropped: it is never shown on any row. Status text comes
        // from the same effective state as the dot, so stale live flags cannot
        // index an exited row as waiting/pending.
        const sessionSearchText = (session: Session | undefined) => {
          if (!session) return "";
          return joinSearchText(
            session.name,
            session.agentLabel,
            sessionEffectiveStatusSearchText(session)
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
        // #733/#515: single source of truth for a replica row's Coding Agent label
        // and Profile badge, so the row render (renderReplicaItem) and the sidebar
        // filter search text (replicaSearchText) can never diverge — a badge that is
        // visible is always matchable. A live session wins and stays byte-identical
        // to the pre-#733 behavior (reuses liveAgentLabel / sessionProfileBadge); a
        // shut-down replica falls back to its persisted config (currentCodingAgentId
        // ?? preferredAgentId — same precedence as the launch path at :616 — and
        // currentProfile), mirroring repoBadges()' configured fallback.
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
            // Repo/branch badges render only for coordinators (renderReplicaItem
            // gate `isCoord() && repoBadges().length>0`). Match the same gate so a
            // non-coordinator row is never surfaced by a badge it isn't showing
            // (#515 bug 1).
            replica.isCoordinator
              ? repos.map((repo) => formatReplicaRepoBadgeLabel(repo)).join(" ")
              : null,
            resolveReplicaAgentLabel(session, replica),
            // #733/#515: mirror the row badges via the shared resolvers so a dormant
            // replica's persisted Coding Agent label + Profile letter are matchable
            // exactly when they are visible (renderReplicaItem uses the same helpers).
            // A live session keeps sessionProfileBadge's `X->Y` fallback text.
            resolveReplicaProfileBadge(session, replica),
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
        const workgroupMatchesAnyGroup = (wg: AcWorkgroup) =>
          canTestGroupMatchId(wg) &&
          compiledGroups().some((entry) => entry.regex?.test(groupMatchId(wg)));
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
        // Memoized so each section reads one cached pass per (filter, store)
        // change instead of rebuilding search text + re-running regex.test on
        // every access (these are read 3-4× per render). Dependencies are all
        // reactive (filterPattern signal, sessionsStore, projectStore via proj),
        // so sections still update live as the user types (#515 bug 3).
        // NOTE: filteredCoordinatorItems is defined further down, right after the
        // coordinatorItems memo it reads — createMemo runs eagerly, so it must
        // follow that declaration (not lead it) to avoid a temporal dead zone.
        const filteredWorkgroups = createMemo(() => {
          const base = groupVisibleWorkgroups();
          if (!filterActive() || matchesFilterText("Workgroups")) return base;
          return base.filter((wg) => workgroupMatches(wg, "Workgroups"));
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
          return !filterActive() || matchesFilterText("Selected Workgroup") || workgroupMatches(wg, "Selected Workgroup");
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

        const reclampReplicaCtxMenu = () => {
          const menu = replicaCtxMenu();
          if (!menu) return;
          const clamp = () => positionReplicaCtxMenu(menu.x, menu.y);
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
          cancelGroupFlyoutClose();
          groupFlyoutAnchorEl = anchor;
          positionGroupFlyout(anchor);
          setGroupFlyoutOpen(true);
          reclampGroupFlyout();
        };

        const activateGroupFlyout = (anchor: HTMLElement) => {
          if (groupFlyoutOpen() && groupFlyoutAnchorEl === anchor) {
            cancelGroupFlyoutClose();
            positionGroupFlyout(anchor);
            reclampGroupFlyout();
            return;
          }
          openGroupFlyout(anchor);
        };
        onCleanup(cancelGroupFlyoutClose);

        const restartReplicaSession = async (
          sessionId: string,
          agentId?: string,
          requestedProfile?: string | null,
        ) => {
          // Row-local cleanup (the context menu that launched this is per-row),
          // then defer to the hoisted core which owns the bounded restart + toast.
          setReplicaCtxMenu(null);
          cleanupCtx();
          await restartReplicaSessionCore(sessionId, agentId, requestedProfile);
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

        // #777 F2: the built-in Non-stop slot as a flyout checkbox row. Mirrors
        // toggleExistingGroup but targets the slot; if absent, adding materializes it.
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
                onMouseEnter={cancelGroupFlyoutClose}
                onMouseLeave={scheduleGroupFlyoutClose}
                onClick={(e) => e.stopPropagation()}
                onContextMenu={(e) => e.stopPropagation()}
                data-ac-testid={`replica.${automationIdPart(wg.name)}.groups.flyout`}
              >
                {/* #777: built-in Non-stop slot, pinned above the user groups. */}
                <button
                  class="session-context-option session-context-group-option session-context-group-option-nonstop"
                  title={`Watch this workgroup in the ${DEFAULT_NON_STOP_NAME} group`}
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
                      if (!valid()) return "Fix this group's regex before adding a workgroup";
                      if (customMembership()) {
                        return "Membership comes from a custom regex. Use Edit groups to change it.";
                      }
                      return selected() ? "Remove this workgroup from the group" : group.regex;
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
              <span>Add to Group</span>
              <span class="session-context-submenu-arrow">&rsaquo;</span>
            </button>
            {renderAddToGroupFlyout(wg)}
          </Show>
        );

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

        const openMatrixFolder = async (path: string) => {
          setAgentCtxMenu(null);
          setReplicaCtxMenu(null);
          cleanupCtx();
          try {
            await WindowAPI.openInExplorer(path);
          } catch (e) {
            console.error("Failed to open Matrix folder:", e);
          }
        };

        const openReplicaFolder = async (path: string) => {
          setReplicaCtxMenu(null);
          cleanupCtx();
          try {
            await WindowAPI.openInExplorer(path);
          } catch (e) {
            console.error("Failed to open Replica folder:", e);
          }
        };

        const openRepoFolder = async (path: string) => {
          setReplicaCtxMenu(null);
          cleanupCtx();
          try {
            await WindowAPI.openInExplorer(path);
          } catch (e) {
            console.error("Failed to open repo folder:", e);
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
        // #810 - the "project" section key moved to the project-collapse store.
        // Sub-section keys below still use the local collapsedByKey signal.
        // Clear the ref-Map entry for this row when the <For> disposes it
        // (background discovery refresh re-creates rows; registerProjectHeader
        // null-branch is a no-op if a newer row already overwrote the key).
        onCleanup(() => registerProjectHeader(proj.path, null));
        const selectedWorkgroupCollapsedKey = projectPanelCollapseKey(proj.path, "selected-workgroup");
        const workgroupsCollapsedKey = projectPanelCollapseKey(proj.path, "workgroups");
        const loopsCollapsedKey = projectPanelCollapseKey(proj.path, "loops");
        const agentsCollapsedKey = projectPanelCollapseKey(proj.path, "agents");
        const teamsCollapsedKey = projectPanelCollapseKey(proj.path, "teams");
        const hasLoopTargets = () =>
          proj.workgroups.some((wg) => wg.agents.some((agent) => agent.isCoordinator));
        // #748 — pair objects are the <For> keys of the coordinator list, so
        // they must keep identity across memo re-runs (with coordSortByActivity
        // on, every session busy→idle markActivity re-runs this memo; fresh
        // pairs would dispose and re-create every coordinator row — the exact
        // lost-click mechanism this fix removes). Reuse the cached pair while
        // its replica+wg objects are unchanged; <For> then MOVES rows on
        // reorder instead of re-creating them.
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
          setReplicaCtxMenu({
            kind: "active",
            sessionId: session.id,
            sessionName: session.name,
            wg,
            replica,
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
          resetGroupMenuState();
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
          const communication = createMemo(() => session()?.communication ?? null);
          // #747: no liveness gate. A dormant restored coordinator keeps its
          // persisted raised hand; every real-exit path clears communication
          // and emits session_communication_changed, so exited-with-hand can
          // only be the restored state.
          const showRaiseHand = createMemo(() =>
            isCoord() &&
            !!taskTitle &&
            communication()?.kind === "raiseHand" &&
            communication()?.visible === true
          );
          const repoBadges = createMemo(() => {
            const s = session();
            return s && s.gitRepos.length > 0
              ? s.gitRepos
              : configuredReplicaRepoBadgesLive(replica, wg);
          });
          // #552/#580 coordinator idle badge. The value is now the unified
          // team-idle anchor (#580): minutes since the whole team was last truly
          // active — it pins to 0 when you message the coordinator, any member is
          // active, or the coordinator is active. The backend carries that anchor
          // in the EXISTING `lastUserMessageAt` field/event (rename deferred), so
          // the FE derivation is unchanged. A createMemo (NOT an IIFE — the IIFE
          // froze; confirmed blocker) so it subscribes to clockStore.nowMs (the
          // live 30s tick) and settingsStore.current (threshold edits apply
          // instantly). #748: the anchor reads through the volatile live layer
          // (effectiveLastUserMessageAt), which the memo subscribes to — a clock
          // event updates this badge in place instead of re-creating the row.
          const idleBadge = createMemo(() =>
            isCoord()
              ? coordinatorIdleBadge(
                  effectiveLastUserMessageAt(replica),
                  clockStore.nowMs,
                  settingsStore.current
                )
              : null
          );
          // #552 auto-closed pill. Driven by the persisted autoClosedAt marker,
          // read through the volatile live layer (#748). #580: MUTUALLY
          // EXCLUSIVE with the minutes badge — when autoClosed() is true the
          // counter is gated off (XOR below), so a closed team shows ONLY the
          // gray AUTO-CLOSED pill; clearing the marker on reopen makes
          // autoClosed() false and the counter returns.
          // #589: ALSO gate on liveness. On raise the dot turns green from the
          // sessionsStore (live session), but a discovery reload can still
          // surface a stale marker (reloadProject supersedes the event-cleared
          // override with a snapshot that may predate the clear), leaving the
          // pill stuck while the dot is green. An auto-closed team is DESTROYED,
          // so it is never live — there is no legitimate "live + auto-closed"
          // state. Gating on `!live` hides the pill the moment the session goes
          // live, reusing the exact signal the status dot reads, so it
          // self-heals regardless of the stale marker; the XOR'd idle counter
          // returns automatically. Inlined isSessionLive(session()) (===
          // isLive() below) because createMemo runs EAGERLY and `isLive` is
          // declared further down — calling it here would hit its temporal dead
          // zone for a coordinator already auto-closed at mount.
          const autoClosed = createMemo(
            () => isCoord() && !!effectiveAutoClosedAt(replica) && !isSessionLive(session())
          );
          // #588 manually-closed pill. Mirrors autoClosed, but ALSO gated on
          // !isSessionLive(session()): a dormant coordinator has no live session
          // so the pill shows; a reopened/raised coordinator is live so the pill
          // hides immediately, independent of marker-clear event timing (the same
          // stale-on-raise trap #589 fixes for AUTO-CLOSED). Use INLINE
          // isSessionLive(session()), NOT isLive() — isLive is declared later and
          // this memo is eager (TDZ).
          const manuallyClosed = createMemo(
            () => isCoord() && !!effectiveManuallyClosedAt(replica) && !isSessionLive(session())
          );
          // #580 idle-badge tooltip. The auto-close clause is appended ONLY when
          // the setting is enabled: Decision 3 keeps the badge (and its red >=60
          // color) visible even when auto-close is OFF, where the team will NOT
          // close — so an unconditional "auto-closes" claim would be wrong.
          const idleBadgeTitle = () =>
            "Time this team has been idle. Resets when you message the coordinator or any member is active (persists across restarts)." +
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
          // #733: the Coding Agent + Profile badges fall back to the persisted
          // replica config when there is no live session, so a shut-down replica
          // (incl. AUTO-/MANUALLY-CLOSED coordinators — no isCoord() gate, Maria's
          // scope decision) still shows its last agent/profile. Both the render
          // (here) and the sidebar filter (replicaSearchText) route through the
          // shared resolveReplica* helpers so a visible badge is always matchable
          // (#515). The live-session branch stays byte-identical.
          const liveAgentLabel = () => resolveReplicaAgentLabel(session(), replica);
          const profileBadge = () => resolveReplicaProfileBadge(session(), replica);
          // #548: resolver-backed tooltip naming the EFFECTIVE profile (the one
          // actually in effect) for this session's coding agent. Plain function
          // (NOT createMemo) — row-local and recomputes on settings reload.
          // #733: same session-less fallback as the two helpers above — the tooltip
          // resolves from the persisted agent id + profile letter when there is no
          // live session (cfg-missing still short-circuits to undefined, unchanged).
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
            // #588 route through the shared helper: a coordinator close marks +
            // (settings-gated) cascades + confirms when busy; a non-coordinator
            // is a plain destroy inside the helper (unchanged behavior).
            if (s) void requestCoordinatorClose(s);
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
                <span class="replica-item-name">{replica.originProject ? `${replica.name}@${replica.originProject}` : replica.name}</span>
                <div class="ac-discovery-badges" data-ac-testid={badgesTestId()}>
                  {/* #552/#580: the coordinator idle (minutes) badge leads the
                      row; the neutral AUTO-CLOSED pill REPLACES it when the team
                      is auto-closed (mutually exclusive — the #580 XOR gate), so
                      exactly one of the two renders first, before all others. */}
                  <Show when={!autoClosed() && !manuallyClosed() && idleBadge()}>
                    {(b) => (
                      <span
                        class={`ac-discovery-badge coord-idle ${b().colorClass}`}
                        title={idleBadgeTitle()}
                      >
                        {b().label}
                      </span>
                    )}
                  </Show>
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
                      title="This team's coordinator was closed manually. Reopen it to clear."
                    >
                      MANUALLY-CLOSED
                    </span>
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
                    {(badge) => <span class="profile-badge" title={profileBadgeTitle()}>{badge()}</span>}
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
            <button
              class="project-header"
              title={proj.path}
              ref={(el) => {
                // #810 (grinch F1) - register this header element in the
                // stable-scope ref-Map so the focus effect can scroll it
                // into view by normalized path key. CSS attribute selector
                // approach was dropped: backslashes in Windows paths break
                // the CSS string-token parser.
                registerProjectHeader(proj.path, el);
              }}
              onClick={() => toggleProjectPanelCollapsed(proj.path)}
              onContextMenu={handleProjectContextMenu}
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
                    New Workgroup
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
                            <span class="ac-wg-name">Selected Workgroup</span>
                          </div>
                          <span class="ac-team-count">{selectedWorkgroup() ? 1 : 0}</span>
                        </div>
                        <Show when={!isPanelCollapsed(selectedWorkgroupCollapsedKey)}>
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
                        onClick={() => togglePanelCollapsed(workgroupsCollapsedKey)}
                        onContextMenu={handleWorkgroupsHeaderContextMenu}
                      >
                        <span class="ac-discovery-chevron" classList={{ collapsed: isPanelCollapsed(workgroupsCollapsedKey) }}>
                          &#x25BE;
                        </span>
                        <div class="ac-wg-header-text">
                          <span class="ac-wg-name">Workgroups</span>
                        </div>
                        <span class="ac-team-count">{filteredWorkgroups().length}</span>
                      </div>
                      <Show when={!isPanelCollapsed(workgroupsCollapsedKey)}>
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
                              setNewWorkgroupTarget({ projectPath: proj.path });
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
                >
                  <Show when={activeReplicaMenu()}>
                    {(menu) => {
                      // Broom renders in EVERY replica dot state (#545). Disabled
                      // only when the task title has nothing to clear (empty/missing
                      // or the literal "Clean" sentinel); enabled otherwise.
                      const broomDisabled = () => isTaskClean(menu().wg.taskTitle);
                      const broomTitle = () =>
                        broomDisabled() ? "Nothing to clear" : "Clear task title";
                      const matrixFolder = () => replicaMatrixFolder(menu().replica);
                      const repoEntries = () => replicaRepoMenuEntries(menu().wg, menu().replica);
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
                        <button
                          class="session-context-option"
                          title={menu().replica.path}
                          onClick={() => void openReplicaFolder(menu().replica.path)}
                        >
                          &#x1F4C2; Open Replica's Folder
                        </button>
                        <Show when={repoEntries().length > 0}>
                          <For each={repoEntries()}>
                            {(repo, index) => (
                              <button
                                class="session-context-option session-context-repo-option"
                                title={repo.sourcePath}
                                onClick={() => void openRepoFolder(repo.sourcePath)}
                                data-ac-testid={`replica.${menu().sessionId}.menu.repo.${index()}`}
                                data-ac-role="menuitem"
                              >
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
                                <span class="session-context-repo-label">{repo.label}</span>
                              </button>
                            )}
                          </For>
                        </Show>
                        <Show when={matrixFolder()}>
                          {(path) => (
                            <button
                              class="session-context-option"
                              title={path()}
                              onClick={() => void openMatrixFolder(path())}
                            >
                              <MatrixFolderIcon /> Open Matrix folder
                            </button>
                          )}
                        </Show>
                        <div class="context-separator" />
                        <button
                          class="session-context-option"
                          onClick={() => toggleReplicaDetach(menu().sessionId)}
                        >
                          {sessionsStore.isDetached(menu().sessionId) ? "Re-attach to main" : "Open in new window"}
                        </button>
                        {renderAddToGroupItem(menu().wg, menu().replica)}
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
                      const matrixFolder = () => replicaMatrixFolder(menu().replica);
                      const repoEntries = () => replicaRepoMenuEntries(menu().wg, menu().replica);
                      return (
                        <>
                          <button
                            class="session-context-option"
                            onClick={() => {
                              const wg = menu().wg;
                              const replica = menu().replica;
                              setReplicaCtxMenu(null);
                              cleanupCtx();
                              setInactiveCodingAgentTarget({
                                projectPath: proj.path,
                                wgPath: wg.path,
                                replicaPath: replica.path,
                              });
                            }}
                          >
                            Coding Agent
                          </button>
                          <button
                            class="session-context-option"
                            title={menu().replica.path}
                            onClick={() => void openReplicaFolder(menu().replica.path)}
                          >
                            &#x1F4C2; Open Replica's Folder
                          </button>
                          <Show when={repoEntries().length > 0}>
                            <For each={repoEntries()}>
                              {(repo, index) => (
                                <button
                                  class="session-context-option session-context-repo-option"
                                  title={repo.sourcePath}
                                  onClick={() => void openRepoFolder(repo.sourcePath)}
                                  data-ac-testid={`replica.inactive.menu.repo.${index()}`}
                                  data-ac-role="menuitem"
                                >
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
                                  <span class="session-context-repo-label">{repo.label}</span>
                                </button>
                              )}
                            </For>
                          </Show>
                          <Show when={matrixFolder()}>
                            {(path) => (
                              <button
                                class="session-context-option"
                                title={path()}
                                onClick={() => void openMatrixFolder(path())}
                              >
                                <MatrixFolderIcon /> Open Matrix folder
                              </button>
                            )}
                          </Show>
                          {renderAddToGroupItem(menu().wg, menu().replica)}
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
          // #551 FIX 2: a live session's agent IS the explicit current
          // coding agent, so it doubles as the redundancy baseline.
          explicitCurrentAgentId={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.agentId}
          currentRequestedProfile={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.requestedProfile}
          scopeContext={deriveScopeContextFromSession(
            sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId),
            replicaCodingAgentTarget()!.sessionName,
          )}
          // #551: re-assigning the running agent+profile is a no-op and
          // pops a needless restart prompt — disable it with a tooltip.
          disableRedundantReplicaAssign
          // #592: but DRIFT (loaded cell != current config) makes a same-pair
          // re-assign meaningful (re-stamp + relaunch), so it overrides the
          // disable. Same backend profileOutdated the badge reads.
          targetProfileOutdated={sessionsStore.sessions.find((s) => s.id === replicaCodingAgentTarget()!.sessionId)?.profileOutdated}
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
              await restartReplicaSessionCore(
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
            // #551 FIX 2: redundancy keys off the EXPLICIT currentCodingAgentId
            // only — never the preferredAgentId hint above. A never-assigned gray
            // replica (no currentCodingAgentId) keeps "Assign" enabled so its
            // preferred agent can be pinned in one click.
            explicitCurrentAgentId={resolved().replica.currentCodingAgentId ?? null}
            currentRequestedProfile={resolved().replica.currentProfile ?? null}
            scopeContext={replicaScopeContext(resolved().wg, resolved().replica)}
            // #551: pre-launch "Set Coding Agent" opens pre-selected to the
            // replica's current pair; re-assigning it is a no-op, so disable.
            disableRedundantReplicaAssign
            onSelect={async () => {
              // WG replica: the picker already wrote the coding-agent selection
              // via the backend (no restart — the agent isn't running). Capture
              // the project path BEFORE clearing the target (which collapses the
              // resolver), then reload so the chosen agent shows and is
              // pre-selected at first launch.
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
              <span class="agent-modal-title">Close coordinator?</span>
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
              // #599 R1: resume the prior conversation when reopening a closed
              // coordinator; omit (default skip) for a genuinely fresh launch.
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
