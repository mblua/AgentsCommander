import { createMemo } from "solid-js";
import { createStore } from "solid-js/store";
import { NO_TEAM } from "../../shared/constants";
import type { Session, SessionsState, Team } from "../../shared/types";

const [state, setState] = createStore<SessionsState>({
  sessions: [],
  activeId: null,
  teams: [],
  teamFilter: null,
  showInactive: false,
});

function normalizePath(p: string): string {
  return p.replace(/\\/g, "/").toLowerCase().replace(/\/+$/, "");
}

const allTeamPathsMemo = createMemo(() => {
  const paths = new Set<string>();
  for (const t of state.teams)
    for (const m of t.members) paths.add(normalizePath(m.path));
  return paths;
});

/** Build a placeholder Session for an inactive team member */
function makeInactiveEntry(name: string, path: string): Session {
  return {
    id: `inactive-${normalizePath(path)}`,
    name,
    shell: "",
    shellArgs: [],
    createdAt: "",
    workingDirectory: path,
    status: "idle",
    waitingForInput: false,
    lastPrompt: null,
    gitBranch: null,
    token: "",
  };
}

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

  if (!state.showInactive) return activeSessions;

  // Add inactive team members that don't have active sessions
  const activePathSet = new Set(
    state.sessions.map((s) => normalizePath(s.workingDirectory))
  );

  const inactiveEntries: Session[] = [];

  if (!state.teamFilter || state.teamFilter === NO_TEAM) {
    // "All" or "No team" — show inactive from all teams
    if (state.teamFilter !== NO_TEAM) {
      for (const team of state.teams) {
        for (const m of team.members) {
          const np = normalizePath(m.path);
          if (!activePathSet.has(np) && !inactiveEntries.some((e) => normalizePath(e.workingDirectory) === np)) {
            inactiveEntries.push(makeInactiveEntry(m.name, m.path));
          }
        }
      }
    }
  } else {
    // Specific team
    const team = state.teams.find((t) => t.id === state.teamFilter);
    if (team) {
      for (const m of team.members) {
        const np = normalizePath(m.path);
        if (!activePathSet.has(np) && !inactiveEntries.some((e) => normalizePath(e.workingDirectory) === np)) {
          inactiveEntries.push(makeInactiveEntry(m.name, m.path));
        }
      }
    }
  }

  return [...activeSessions, ...inactiveEntries];
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
  get filteredSessions() {
    return filteredSessionsMemo();
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
    setState("activeId", id);
    // Update statuses
    setState("sessions", (s) => s.id === id, "status", "active");
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
    setState("sessions", (s) => s.id === id, "waitingForInput", waiting);
  },

  setGitBranch(sessionId: string, branch: string | null) {
    setState("sessions", (s) => s.id === sessionId, "gitBranch", branch);
  },

  setTeams(teams: Team[]) {
    setState("teams", teams);
    if (
      state.teamFilter &&
      state.teamFilter !== NO_TEAM &&
      !teams.some((t) => t.id === state.teamFilter)
    ) {
      setState("teamFilter", null);
    }
  },

  setTeamFilter(teamId: string | null) {
    setState("teamFilter", teamId);
  },

  toggleShowInactive() {
    setState("showInactive", !state.showInactive);
  },
};
