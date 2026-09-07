import { createMemo, createSignal } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { NO_TEAM } from "../../shared/constants";
import type { RepoMatch, Session, SessionCommunication, SessionRepo, SessionSelection, SessionsState, Team, TeamSessionGroup } from "../../shared/types";
import type { TransportConnectionState } from "../../shared/transport";
import { projectStore } from "./project";
import { normalizeProjectPathForCompare } from "./project-refresh";
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { applySelectionToSessionList, reconcileVisibleOrderKeys, upsertSessionList } from "./sessions-helpers";

const [toggleInFlight, setToggleInFlight] = createSignal(false);
const [sidebarPointerInside, setSidebarPointerInside] = createSignal(false);
const [sidebarMenuOpen, setSidebarMenuOpen] = createSignal(false);
const [lastCoordinatorVisibleOrderByProject, setLastCoordinatorVisibleOrderByProject] = createSignal<Record<string, string[]>>({});
const [frozenCoordinatorVisibleOrderByProject, setFrozenCoordinatorVisibleOrderByProject] = createSignal<Record<string, string[]>>({});
// Independent of selection ordering: every full-row upsert/removal invalidates
// an older wholesale list snapshot, even when the local membership is unchanged.
let rowMembershipGeneration = 0;

// #1779 - the counter advances on every setSessionWaiting call. A reconciliation
// that read the backend session list BEFORE an edge landed must not apply its
// stale snapshot on top of the fresher edge, so it captures this value before its
// await and compares it exactly once after.
let waitingEdgeGeneration = 0;

// Combined sidebar interaction lock: the coordinator tile order stays frozen
// while the pointer is inside the sidebar OR while any sidebar context menu
// (or flyout) node is present in the DOM. Menus are rendered through Solid
// <Portal> under document.body, so a DOM-derived presence lock releases on
// every close path structurally (close = node removed = observer recomputes).
let sidebarOrderLockActive = false;
let sidebarMenuLockObserverInstalled = false;

function refreshSidebarOrderLock(): void {
  const active = sidebarPointerInside() || sidebarMenuOpen();
  if (active === sidebarOrderLockActive) return;
  sidebarOrderLockActive = active;
  if (active) {
    setFrozenCoordinatorVisibleOrderByProject(lastCoordinatorVisibleOrderByProject());
  } else {
    setFrozenCoordinatorVisibleOrderByProject({});
  }
}

function updateSidebarMenuOpen(value: boolean): void {
  if (value === sidebarMenuOpen()) return;
  setSidebarMenuOpen(value);
  refreshSidebarOrderLock();
}

function installSidebarMenuLockObserver(): void {
  if (sidebarMenuLockObserverInstalled) return;
  if (typeof document === "undefined" || !document.body) return; // node-env unit tests
  sidebarMenuLockObserverInstalled = true;
  new MutationObserver(() => {
    updateSidebarMenuOpen(
      document.querySelector(".session-context-menu, .session-context-flyout") !== null
    );
  }).observe(document.body, { childList: true, subtree: true });
}
installSidebarMenuLockObserver();

const [state, setState] = createStore<SessionsState>({
  sessions: [],
  activeId: null,
  selection: null,
  selectionEpoch: null,
  selectionRevision: -1,
  selectionConnectionGeneration: null,
  retiredSelectionEpochs: [],
  connectionGeneration: -1,
  transportConnected: false,
  awaitingHydrationGeneration: null,
  teams: [],
  teamFilter: null,
  showInactive: false,
  showCategories: true,
  alwaysShowSelectedWorkgroup: true,
  repos: [],
  coordSortByActivity: false,
  lastActivityBySessionId: {},
  contextPercentBySessionId: {},
  hydrated: false,
});

function projectStoredSelection(sessions: Session[]): void {
  if (
    !state.transportConnected ||
    !state.selection ||
    state.selectionConnectionGeneration !== state.connectionGeneration
  ) {
    setState("sessions", sessions.map((session) =>
      session.status === "active"
        ? { ...session, status: "running" as const }
        : session,
    ));
    setState("activeId", null);
    return;
  }
  const applied = applySelectionToSessionList(sessions, state.selection);
  setState("sessions", applied.sessions);
  setState("activeId", applied.activeId);
}

function advanceRowMembershipGeneration(): void {
  rowMembershipGeneration += 1;
}

function normalizePath(p: string): string {
  return p.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
}

function isUnderAnyPath(path: string, roots: string[]): boolean {
  if (roots.length === 0) return false;
  const normalizedPath = normalizeProjectPathForCompare(path);
  return roots.some((root) => {
    const normalizedRoot = normalizeProjectPathForCompare(root);
    return normalizedPath === normalizedRoot || normalizedPath.startsWith(`${normalizedRoot}/`);
  });
}

function stringArraysEqual(a: string[] | undefined, b: string[]): boolean {
  if (!a || a.length !== b.length) return false;
  return a.every((value, index) => value === b[index]);
}

const allTeamPathsMemo = createMemo(() => {
  const paths = new Set<string>();
  for (const t of state.teams) {
    if (t.visible === false) continue;
    for (const m of t.members) paths.add(normalizePath(m.path));
  }
  return paths;
});

function makeInactiveEntry(name: string, path: string): Session {
  return {
    id: `inactive-${normalizePath(path)}`,
    name,
    shell: "",
    shellArgs: [],
    effectiveShellArgs: null,
    createdAt: "",
    workingDirectory: path,
    status: "idle",
    waitingForInput: false,
    pendingReview: false,
    lastPrompt: null,
    agentId: null,
    agentLabel: null,
    gitRepos: [],
    workgroupTask: null,
    isCoordinator: false,
    isRootAgent: false,
    token: "",
    agentKind: null,
    requestedProfile: null,
    effectiveProfile: null,
    profileFallbackChain: [],
    profileFallbackApplied: false,
  };
}

const wgReplicaMemo = createMemo(() => {
  const names = new Set<string>();
  const paths = new Set<string>();
  for (const proj of projectStore.projects) {
    for (const wg of proj.workgroups) {
      for (const replica of wg.agents) {
        names.add(`${wg.name}/${replica.name}`);
        paths.add(normalizePath(replica.path));
      }
    }
    for (const agent of proj.agents) {
      names.add(agent.name);
      paths.add(normalizePath(agent.path));
    }
  }
  return { names, paths };
});

const filteredSessionsMemo = createMemo(() => {
  const nonRootSessions = state.sessions.filter((s) => !s.isRootAgent);

  const activeSessions = (() => {
    if (!state.teamFilter) return nonRootSessions;

    let matches: (normalizedPath: string) => boolean;

    if (state.teamFilter === NO_TEAM) {
      const allPaths = allTeamPathsMemo();
      matches = (p) => !allPaths.has(p);
    } else {
      const team = state.teams.find((t) => t.id === state.teamFilter);
      if (!team) return nonRootSessions;
      const paths = new Set(team.members.map((m) => normalizePath(m.path)));
      matches = (p) => paths.has(p);
    }

    return nonRootSessions.filter((s) => {
      if (!s.workingDirectory) return state.teamFilter === NO_TEAM;
      return matches(normalizePath(s.workingDirectory));
    });
  })();

  const wg = wgReplicaMemo();
  const visibleSessions = wg.names.size > 0
    ? activeSessions.filter((s) => !wg.names.has(s.name))
    : activeSessions;
  const archived = projectStore.archivedPaths;
  const notArchived = (s: Session) =>
    !s.workingDirectory || !isUnderAnyPath(s.workingDirectory, archived);
  const projectVisibleSessions =
    archived.length > 0 ? visibleSessions.filter(notArchived) : visibleSessions;

  const sortKey = (s: Session) => {
    const i = s.name.lastIndexOf("/");
    return i >= 0 ? s.name.slice(i + 1) : s.name;
  };
  if (!state.showInactive) return [...projectVisibleSessions].sort((a, b) => sortKey(a).localeCompare(sortKey(b), "en", { sensitivity: "base", numeric: true }));

  const activePathSet = new Set(
    state.sessions
      .filter((s) => s.workingDirectory)
      .map((s) => normalizePath(s.workingDirectory))
  );
  const addedPaths = new Set<string>();
  const inactiveEntries: Session[] = [];

  const addInactive = (name: string, path: string) => {
    const np = normalizePath(path);
    if (!activePathSet.has(np) && !addedPaths.has(np)) {
      addedPaths.add(np);
      inactiveEntries.push(makeInactiveEntry(name, path));
    }
  };

  if (!state.teamFilter) {
    for (const repo of state.repos) {
      addInactive(repo.name, repo.path);
    }
  } else if (state.teamFilter === NO_TEAM) {
    const teamPaths = allTeamPathsMemo();
    for (const repo of state.repos) {
      if (!teamPaths.has(normalizePath(repo.path))) {
        addInactive(repo.name, repo.path);
      }
    }
  } else {
    const team = state.teams.find((t) => t.id === state.teamFilter);
    if (team) {
      for (const m of team.members) {
        addInactive(m.name, m.path);
      }
    }
  }

  const filteredInactive = wg.paths.size > 0
    ? inactiveEntries.filter((e) => !wg.paths.has(normalizePath(e.workingDirectory)))
    : inactiveEntries;
  const visibleInactive =
    archived.length > 0 ? filteredInactive.filter(notArchived) : filteredInactive;

  return [...projectVisibleSessions, ...visibleInactive].sort((a, b) =>
    sortKey(a).localeCompare(sortKey(b), "en", { sensitivity: "base", numeric: true })
  );
});

const [collapsedTeams, setCollapsedTeams] = createSignal<Record<string, boolean>>({});

const [detachedIds, setDetachedIds] = createSignal<Set<string>>(new Set());

const groupedSessionsMemo = createMemo((): { groups: TeamSessionGroup[]; ungrouped: Session[] } => {
  const sessions = filteredSessionsMemo();
  const teams = state.teams;

  if (teams.length === 0) return { groups: [], ungrouped: sessions };

  const groups: TeamSessionGroup[] = [];
  const assignedPaths = new Set<string>();

  for (const team of teams) {
    if (team.visible === false) continue;
    const memberPaths = new Set(team.members.map((m) => normalizePath(m.path)));

    const teamSessions = sessions.filter((s) =>
      s.workingDirectory && memberPaths.has(normalizePath(s.workingDirectory))
    );

    if (teamSessions.length === 0 && !state.showInactive) continue;

    let coordinator: Session | null = null;
    const members: Session[] = [];

    for (const s of teamSessions) {
      const np = normalizePath(s.workingDirectory);
      const member = team.members.find((m) => normalizePath(m.path) === np);
      if (member && team.coordinatorName && member.name === team.coordinatorName) {
        coordinator = s;
      } else {
        members.push(s);
      }
      assignedPaths.add(np);
    }

    if (state.showInactive) {
      const activePathSet = new Set(teamSessions.map((s) => normalizePath(s.workingDirectory)));
      for (const m of team.members) {
        const np = normalizePath(m.path);
        if (!activePathSet.has(np)) {
          const inactive = makeInactiveEntry(m.name, m.path);
          if (team.coordinatorName && m.name === team.coordinatorName) {
            coordinator = inactive;
          } else {
            members.push(inactive);
          }
          assignedPaths.add(np);
        }
      }
    }

    groups.push({ team, coordinator, members });
  }

  const ungrouped = sessions.filter((s) => {
    if (!s.workingDirectory) return true;
    return !assignedPaths.has(normalizePath(s.workingDirectory));
  });

  return { groups, ungrouped };
});

export const sessionsStore = {
  get sessions() {
    return state.sessions;
  },
  get activeId() {
    return state.activeId;
  },
  get selection() {
    return state.selection;
  },
  get selectionEpoch() {
    return state.selectionEpoch;
  },
  get selectionRevision() {
    return state.selectionRevision;
  },
  get selectionConnectionGeneration() {
    return state.selectionConnectionGeneration;
  },
  get connectionGeneration() {
    return state.connectionGeneration;
  },
  get transportConnected() {
    return state.transportConnected;
  },
  get awaitingHydrationGeneration() {
    return state.awaitingHydrationGeneration;
  },
  get teams() {
    return state.teams;
  },
  get teamFilter() {
    return state.teamFilter;
  },
  get showInactive() {
    return state.showInactive;
  },
  get showCategories() {
    return state.showCategories;
  },
  get alwaysShowSelectedWorkgroup() {
    return state.alwaysShowSelectedWorkgroup;
  },
  get repos() {
    return state.repos;
  },
  get filteredSessions() {
    return filteredSessionsMemo();
  },
  get groupedSessions() {
    return groupedSessionsMemo();
  },
  get collapsedTeams() {
    return collapsedTeams();
  },
  get rowMembershipGeneration() {
    return rowMembershipGeneration;
  },
  get waitingEdgeGeneration() {
    return waitingEdgeGeneration;
  },

  setSessions(sessions: Session[]) {
    advanceRowMembershipGeneration();
    projectStoredSelection(sessions);
  },

  setSessionsIfRowMembershipUnchanged(
    sessions: Session[],
    expectedGeneration: number,
  ): boolean {
    if (rowMembershipGeneration !== expectedGeneration) return false;
    advanceRowMembershipGeneration();
    projectStoredSelection(sessions);
    return true;
  },

  addSession(session: Session) {
    advanceRowMembershipGeneration();
    projectStoredSelection(upsertSessionList(state.sessions, session));
  },

  removeSession(id: string) {
    // A destroy event is newer membership evidence even when the row is not
    // currently present; an older pending list may still contain that ID.
    advanceRowMembershipGeneration();
    projectStoredSelection(state.sessions.filter((session) => session.id !== id));
    if (state.activeId === id) setState("activeId", null);
  },

  observeConnection(connection: TransportConnectionState): boolean {
    if (connection.generation < state.connectionGeneration) return false;
    const connected = connection.state === "connected";
    const generationChanged = connection.generation !== state.connectionGeneration;
    const changed =
      generationChanged ||
      connected !== state.transportConnected;
    setState("connectionGeneration", connection.generation);
    setState("transportConnected", connected);
    if (!connected || generationChanged) {
      setState("awaitingHydrationGeneration", null);
      setState("activeId", null);
      setState("sessions", (sessions) =>
        sessions.map((session) =>
          session.status === "active"
            ? { ...session, status: "running" as const }
            : session,
        ),
      );
    }
    return changed;
  },

  beginHydration(generation: number): boolean {
    if (
      generation !== state.connectionGeneration ||
      !state.transportConnected
    ) {
      return false;
    }
    setState("awaitingHydrationGeneration", generation);
    return true;
  },

  cancelHydration(generation?: number): void {
    if (
      generation === undefined ||
      state.awaitingHydrationGeneration === generation
    ) {
      setState("awaitingHydrationGeneration", null);
    }
  },

  applySelection(
    selection: SessionSelection,
    generation: number,
    allowEqualReconnect = false,
  ): boolean {
    if (
      generation !== state.connectionGeneration ||
      !state.transportConnected
    ) {
      return false;
    }
    if (state.selectionEpoch === selection.epoch) {
      if (selection.revision < state.selectionRevision) return false;
      if (selection.revision === state.selectionRevision) {
        if (
          !allowEqualReconnect ||
          state.awaitingHydrationGeneration !== generation
        ) {
          return false;
        }
      }
    } else {
      if (state.retiredSelectionEpochs.includes(selection.epoch)) return false;
      const previousEpoch = state.selectionEpoch;
      if (previousEpoch) {
        setState("retiredSelectionEpochs", (epochs) => [
          ...epochs,
          previousEpoch,
        ]);
      }
    }
    setState("selection", selection);
    setState("selectionEpoch", selection.epoch);
    setState("selectionRevision", selection.revision);
    setState("selectionConnectionGeneration", generation);
    setState("awaitingHydrationGeneration", null);
    const applied = applySelectionToSessionList(state.sessions, selection);
    setState("sessions", applied.sessions);
    setState("activeId", applied.activeId);
    return true;
  },

  setVisibleActiveIdForTests(id: string | null): void {
    if (import.meta.env.MODE !== "test") {
      throw new Error("setVisibleActiveIdForTests is test-only");
    }
    setState("activeId", id);
  },

  resetSelectionForTests(): void {
    if (import.meta.env.MODE !== "test") {
      throw new Error("resetSelectionForTests is test-only");
    }
    setState("selection", null);
    setState("selectionEpoch", null);
    setState("selectionRevision", -1);
    setState("selectionConnectionGeneration", null);
    setState("retiredSelectionEpochs", []);
    setState("connectionGeneration", -1);
    setState("transportConnected", false);
    setState("awaitingHydrationGeneration", null);
    setState("activeId", null);
  },

  renameSession(id: string, name: string) {
    setState("sessions", (s) => s.id === id, "name", name);
  },

  setSessionWaiting(id: string, waiting: boolean) {
    const session = state.sessions.find((s) => s.id === id);
    const wasAlreadyWaiting = session?.waitingForInput ?? false;
    const wasPendingReview = session?.pendingReview ?? false;
    const isActive = id === state.activeId;
    waitingEdgeGeneration += 1;
    console.debug(
      `[idle-fe] setSessionWaiting ${id.slice(0, 8)} waiting=${waiting} wasAlreadyWaiting=${wasAlreadyWaiting} wasPendingReview=${wasPendingReview} isActive=${isActive} gen=${waitingEdgeGeneration}`,
    );
    setState("sessions", (s) => s.id === id, "waitingForInput", waiting);
    if (waiting && !wasAlreadyWaiting && !isActive) {
      console.debug(`[idle-fe] raise pendingReview ${id.slice(0, 8)}`);
      setState("sessions", (s) => s.id === id, "pendingReview", true);
    }
    if (!waiting) {
      console.debug(
        `[idle-fe] clear pendingReview ${id.slice(0, 8)} wasPendingReview=${wasPendingReview}`,
      );
      setState("sessions", (s) => s.id === id, "pendingReview", false);
    }
  },

  setCommunication(sessionId: string, communication: SessionCommunication | null) {
    setState("sessions", (s) => s.id === sessionId, "communication", communication);
  },

  setGitRepos(sessionId: string, repos: SessionRepo[]) {
    setState("sessions", (s) => s.id === sessionId, "gitRepos", repos);
  },

  setIsCoordinator(sessionId: string, value: boolean) {
    setState("sessions", (s) => s.id === sessionId, "isCoordinator", value);
  },

  setProfileOutdated(id: string, outdated: boolean) {
    setState("sessions", (s) => s.id === id, "profileOutdated", outdated);
  },

  setTeams(teams: Team[]) {
    setState("teams", teams);
    if (
      state.teamFilter &&
      state.teamFilter !== NO_TEAM &&
      !teams.some((t) => t.id === state.teamFilter && t.visible !== false)
    ) {
      setState("teamFilter", null);
    }
  },

  setRepos(repos: RepoMatch[]) {
    setState("repos", repos);
  },

  setTeamFilter(teamId: string | null) {
    setState("teamFilter", teamId);
  },

  toggleShowInactive() {
    setState("showInactive", !state.showInactive);
  },

  toggleShowCategories() {
    setState("showCategories", !state.showCategories);
  },

  async toggleAlwaysShowSelectedWorkgroup() {
    if (toggleInFlight()) return;
    setToggleInFlight(true);
    const next = !state.alwaysShowSelectedWorkgroup;
    setState("alwaysShowSelectedWorkgroup", next);
    try {
      const current = await SettingsAPI.get();
      await SettingsAPI.update({ ...current, alwaysShowSelectedWorkgroup: next });
      void settingsStore.refresh();
    } catch (e) {
      console.error("[always-show-wg] Failed to persist alwaysShowSelectedWorkgroup:", e);
      setState("alwaysShowSelectedWorkgroup", !next);
    } finally {
      setToggleInFlight(false);
    }
  },

  get coordSortByActivity() {
    return state.coordSortByActivity;
  },
  get lastActivityBySessionId() {
    return state.lastActivityBySessionId;
  },
  get contextPercentBySessionId() {
    return state.contextPercentBySessionId;
  },
  get hydrated() {
    return state.hydrated;
  },
  get toggleInFlight() {
    return toggleInFlight();
  },
  get sidebarPointerInside() {
    return sidebarPointerInside();
  },
  get sidebarMenuOpen() {
    return sidebarMenuOpen();
  },

  setAlwaysShowSelectedWorkgroup(value: boolean) {
    setState("alwaysShowSelectedWorkgroup", value);
  },

  setSidebarPointerInside(value: boolean) {
    if (value === sidebarPointerInside()) return;
    setSidebarPointerInside(value);
    refreshSidebarOrderLock();
  },

  setSidebarMenuOpen(value: boolean) {
    updateSidebarMenuOpen(value);
  },

  coordinatorVisibleOrder(projectPath: string, nextKeys: string[]): string[] {
    if (!sidebarPointerInside() && !sidebarMenuOpen()) return nextKeys;

    const frozenByProject = frozenCoordinatorVisibleOrderByProject();
    const frozenKeys = frozenByProject[projectPath] ?? lastCoordinatorVisibleOrderByProject()[projectPath] ?? nextKeys;
    const visibleKeys = reconcileVisibleOrderKeys(nextKeys, frozenKeys);
    if (!stringArraysEqual(frozenByProject[projectPath], visibleKeys)) {
      setFrozenCoordinatorVisibleOrderByProject((prev) => ({ ...prev, [projectPath]: visibleKeys }));
    }
    return visibleKeys;
  },

  recordCoordinatorVisibleOrder(projectPath: string, visibleKeys: string[]) {
    if (stringArraysEqual(lastCoordinatorVisibleOrderByProject()[projectPath], visibleKeys)) return;
    setLastCoordinatorVisibleOrderByProject((prev) => ({ ...prev, [projectPath]: visibleKeys }));
  },

  setCoordSortByActivity(value: boolean) {
    setState("coordSortByActivity", value);
    setState("hydrated", true);
  },

  async toggleCoordSortByActivity() {
    if (!state.hydrated) return;
    if (toggleInFlight()) return;
    setToggleInFlight(true);
    const next = !state.coordSortByActivity;
    setState("coordSortByActivity", next);
    try {
      const current = await SettingsAPI.get();
      await SettingsAPI.update({ ...current, coordSortByActivity: next });
      void settingsStore.refresh();
    } catch (e) {
      console.error("[coord-sort] Failed to persist coordSortByActivity:", e);
      setState("coordSortByActivity", !next);
    } finally {
      setToggleInFlight(false);
    }
  },

  markActivity(sessionId: string) {
    setState("lastActivityBySessionId", (prev) => ({ ...prev, [sessionId]: performance.now() }));
  },

  setSessionContext(sessionId: string, percent: number | null) {
    setState("contextPercentBySessionId", (prev) => ({ ...prev, [sessionId]: percent }));
  },

  hydrateSessionContext(sessionId: string, percent: number | null) {
    setState("contextPercentBySessionId", (prev) =>
      sessionId in prev ? prev : { ...prev, [sessionId]: percent },
    );
  },

  resetContextReadingsForTests() {
    const emptyReadings: Record<string, number | null> = {};
    setState(
      "contextPercentBySessionId",
      reconcile(emptyReadings),
    );
  },

  resetActivityForTests() {
    setState("lastActivityBySessionId", reconcile({}));
  },

  toggleTeamCollapsed(teamId: string) {
    setCollapsedTeams((prev) => ({ ...prev, [teamId]: !prev[teamId] }));
  },

  findSessionByName(name: string): Session | undefined {
    return state.sessions.find((s) => s.name === name);
  },

  isDetached(id: string): boolean {
    return detachedIds().has(id);
  },

  setDetached(id: string, detached: boolean) {
    const current = detachedIds();
    if (detached === current.has(id)) return; // no-op
    const next = new Set(current);
    if (detached) next.add(id); else next.delete(id);
    setDetachedIds(next);
  },

  clearDetached() {
    setDetachedIds(new Set<string>());
  },

};
