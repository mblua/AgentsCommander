import { createMemo, createSignal } from "solid-js";
import { createStore } from "solid-js/store";
import { NO_TEAM } from "../../shared/constants";
import type { RepoMatch, Session, SessionRepo, SessionsState, Team, TeamSessionGroup } from "../../shared/types";
import { projectStore } from "./project";

const [state, setState] = createStore<SessionsState>({
  sessions: [],
  activeId: null,
  teams: [],
  teamFilter: null,
  showInactive: false,
  showCategories: true,
  repos: [],
});

function normalizePath(p: string): string {
  return p.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
}

const allTeamPathsMemo = createMemo(() => {
  const paths = new Set<string>();
  for (const t of state.teams) {
    if (t.visible === false) continue;
    for (const m of t.members) paths.add(normalizePath(m.path));
  }
  return paths;
});

/** Build a placeholder Session for an inactive repo */
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
    isCoordinator: false,
    token: "",
  };
}

/** Names and paths owned by WG replicas and matrix agents — used to hide them from Agent Sessions */
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
  const activeSessions = (() => {
    if (!state.teamFilter) return state.sessions;

    let matches: (normalizedPath: string) => boolean;

    if (state.teamFilter === NO_TEAM) {
      const allPaths = allTeamPathsMemo();
      matches = (p) => !allPaths.has(p);
    } else {
      const team = state.teams.find((t) => t.id === state.teamFilter);
      if (!team) return state.sessions;
      const paths = new Set(team.members.map((m) => normalizePath(m.path)));
      matches = (p) => paths.has(p);
    }

    return state.sessions.filter((s) => {
      if (!s.workingDirectory) return state.teamFilter === NO_TEAM;
      return matches(normalizePath(s.workingDirectory));
    });
  })();

  // Hide sessions owned by WG replicas — they display in ProjectPanel instead
  const wg = wgReplicaMemo();
  const visibleSessions = wg.names.size > 0
    ? activeSessions.filter((s) => !wg.names.has(s.name))
    : activeSessions;

  const sortKey = (s: Session) => {
    const i = s.name.lastIndexOf("/");
    return i >= 0 ? s.name.slice(i + 1) : s.name;
  };
  if (!state.showInactive) return [...visibleSessions].sort((a, b) => sortKey(a).localeCompare(sortKey(b), "en", { sensitivity: "base", numeric: true }));

  // Add inactive repos/members that don't have active sessions
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
    // "All" — show inactive from all discovered repos
    for (const repo of state.repos) {
      addInactive(repo.name, repo.path);
    }
  } else if (state.teamFilter === NO_TEAM) {
    // "No team" — show inactive repos NOT in any team
    const teamPaths = allTeamPathsMemo();
    for (const repo of state.repos) {
      if (!teamPaths.has(normalizePath(repo.path))) {
        addInactive(repo.name, repo.path);
      }
    }
  } else {
    // Specific team — show inactive team members only
    const team = state.teams.find((t) => t.id === state.teamFilter);
    if (team) {
      for (const m of team.members) {
        addInactive(m.name, m.path);
      }
    }
  }

  // Also filter inactive entries whose paths belong to WG replicas
  const filteredInactive = wg.paths.size > 0
    ? inactiveEntries.filter((e) => !wg.paths.has(normalizePath(e.workingDirectory)))
    : inactiveEntries;

  return [...visibleSessions, ...filteredInactive].sort((a, b) =>
    sortKey(a).localeCompare(sortKey(b), "en", { sensitivity: "base", numeric: true })
  );
});

const [collapsedTeams, setCollapsedTeams] = createSignal<Record<string, boolean>>({});

const groupedSessionsMemo = createMemo((): { groups: TeamSessionGroup[]; ungrouped: Session[] } => {
  const sessions = filteredSessionsMemo();
  const teams = state.teams;

  if (teams.length === 0) return { groups: [], ungrouped: sessions };

  const groups: TeamSessionGroup[] = [];
  const assignedPaths = new Set<string>();

  for (const team of teams) {
    // Skip hidden teams — their sessions will appear as ungrouped
    if (team.visible === false) continue;
    const memberPaths = new Set(team.members.map((m) => normalizePath(m.path)));

    // Find sessions belonging to this team
    const teamSessions = sessions.filter((s) =>
      s.workingDirectory && memberPaths.has(normalizePath(s.workingDirectory))
    );

    if (teamSessions.length === 0 && !state.showInactive) continue;

    // Identify coordinator session
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

    // When showInactive, add inactive placeholders for missing team members
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

  // Sessions not in any team
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

  setSessions(sessions: Session[]) {
    setState("sessions", sessions);
  },

  addSession(session: Session) {
    setState("sessions", (prev) =>
      prev.some((s) => s.id === session.id) ? prev : [...prev, session]
    );
  },

  removeSession(id: string) {
    setState("sessions", (prev) => prev.filter((s) => s.id !== id));
  },

  setActiveId(id: string | null) {
    const prev = state.activeId;
    console.log(`[idle-fe] setActiveId: ${id?.slice(0,8)} (prev: ${prev?.slice(0,8)})`);
    setState("activeId", id);
    setState("sessions", (s) => s.id === id, "status", "active");
    setState("sessions", (s) => s.id === id, "pendingReview", false);
    setState(
      "sessions",
      (s) => s.id !== id && s.status === "active",
      "status",
      "running"
    );
  },

  renameSession(id: string, name: string) {
    setState("sessions", (s) => s.id === id, "name", name);
  },

  setSessionWaiting(id: string, waiting: boolean) {
    const session = state.sessions.find((s) => s.id === id);
    const wasAlreadyWaiting = session?.waitingForInput ?? false;
    const isActive = id === state.activeId;
    console.log(`[idle-fe] setSessionWaiting: ${id.slice(0,8)} waiting=${waiting} wasAlreadyWaiting=${wasAlreadyWaiting} isActive=${isActive} pendingReview=${session?.pendingReview}`);
    setState("sessions", (s) => s.id === id, "waitingForInput", waiting);
    // Only set pendingReview on a real busy→idle transition, not re-detection
    if (waiting && !wasAlreadyWaiting && !isActive) {
      console.log(`[idle-fe] >>> SETTING pendingReview=true for ${id.slice(0,8)}`);
      setState("sessions", (s) => s.id === id, "pendingReview", true);
    }
    if (!waiting) {
      setState("sessions", (s) => s.id === id, "pendingReview", false);
    }
  },

  setGitRepos(sessionId: string, repos: SessionRepo[]) {
    setState("sessions", (s) => s.id === sessionId, "gitRepos", repos);
  },

  setIsCoordinator(sessionId: string, value: boolean) {
    setState("sessions", (s) => s.id === sessionId, "isCoordinator", value);
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

  toggleTeamCollapsed(teamId: string) {
    setCollapsedTeams((prev) => ({ ...prev, [teamId]: !prev[teamId] }));
  },

  /** Find a session whose name exactly matches the given string */
  findSessionByName(name: string): Session | undefined {
    return state.sessions.find((s) => s.name === name);
  },

};
