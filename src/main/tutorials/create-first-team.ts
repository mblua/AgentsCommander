import type { ProjectState } from "../../sidebar/stores/project-merge";
import { normalizeProjectPathForCompare } from "../../sidebar/stores/project-refresh";
import { automationIdPart } from "../../sidebar/components/replica-repo-badges";
import type { NewProjectOutcome } from "../../sidebar/stores/project";

// #5228 - "create your first team" tutorial: definition and pure step rules.
// The tutorial only watches real controls (by data-ac-testid) and the project
// store; it never clicks, never creates anything, never changes a dialog.

export const TUTORIAL_TITLE =
  "Build a team: personal assistant, accountant and social media expert";
export const TUTORIAL_LINE = "Create a project, three agents, a team and a room, step by step.";
export const AGENTS = [
  { name: "personal-assistant", label: "personal assistant" },
  { name: "accountant", label: "accountant" },
  { name: "social-media-expert", label: "social media expert" },
] as const;
export const COORD = "personal-assistant";
export const TEAM_NAME = "my-team";
/** Every fixed testid the tutorial reads. The module has no other testid literal. */
export const IDS = {
  newOpen: "actionBar.newOpen", menu: "actionBar.menu",
  menuNewProject: "actionBar.menu.newProject", home: "actionBar.home",
  agentModal: "newAgent.modal", agentName: "newAgent.name", agentCreate: "newAgent.create",
  teamModal: "newTeam.modal", teamName: "newTeam.name", teamAgents: "newTeam.agents",
  teamCreate: "newTeam.create", roomModal: "newRoom.modal", roomTeam: "newRoom.teamSearch",
} as const;
export const NEW_OPEN_DISABLED = "disabled"; // ActionBar triState value
export const TEAM_STEPS = ["step-1", "step-2", "step-3"] as const;
export const ROOM_HEADER_CLASS = "ac-wg-header";
export const projectHeaderId = (pid: string) => `project.header.${pid}`;
export const projectMenuId = (kind: "newAgent" | "newTeam" | "newRoom", pid: string) =>
  `project.action.${kind}.${pid}.projectMenu`;

export function roomHeaderSelector(roomPath: string): string {
  const escaped = roomPath.replace(/["\\]/g, (c) => `\\${c}`);
  return `.${ROOM_HEADER_CLASS}[title="${escaped}"]`;
}

export const NOTE_NO_FOLDER = "No folder chosen. Click New / Open to try again.";
export const NOTE_NOT_EMPTY =
  "That folder already has a project. Click New / Open and choose an empty folder.";
export const NOTE_NAME = (n: string) => `Use the name ${n} so the next steps can find it.`;
export const NOTE_TEAM = "This team needs all three agents and personal-assistant as Coord. " +
  "Create another team, for example my-team-2.";
export const HIDDEN =
  "This step's control is hidden. Expand it in the sidebar or exit the tutorial.";
export const FINAL = "Done! Now you can ask your personal assistant (orchestrator) " +
  "to talk to the accountant.";

export type StepId =
  "openMenu" | "newProject" | "folder" | "header" | "menuItem" | "dialog" | "done";
export type Probe = {
  has(testid: string): boolean;
  state(testid: string): string | null;
  projects: readonly ProjectState[];
  outcome: NewProjectOutcome;
};
export type RunCtx = {
  step: StepId;
  seq: number;
  projectPath: string | null;
  goalKey: string | null;
  count: number;
  note: string | null;
};
export type StepView = { target: string | null; text: string; warn: boolean; final: boolean };

type Workgroup = ProjectState["workgroups"][number];
type Team = ProjectState["teams"][number];
type Goal =
  | { key: string; kind: "agent"; index: number; agent: (typeof AGENTS)[number] }
  | { key: "team"; kind: "team" }
  | { key: "room"; kind: "room"; team: Team }
  | { key: "finished"; kind: "finished"; room: Workgroup };
type Kind = "newAgent" | "newTeam" | "newRoom";

const NAMES = AGENTS.map((a) => a.name);

export function startCtx(seq: number): RunCtx {
  return { step: "openMenu", seq, projectPath: null, goalKey: null, count: 0, note: null };
}

export function tail(ref: string): string {
  const last = ref.split("/").pop() ?? "";
  return last.replace(/^__agent_/, "").replace(/^_agent_/, "");
}

const pid = (p: ProjectState) => automationIdPart(p.path);
const sel = (testid: string) => `[data-ac-testid="${testid}"]`;
const hasAll = (refs: readonly string[]) => {
  const tails = refs.map(tail);
  return NAMES.every((n) => tails.includes(n));
};

function isTutorialTeam(t: Team): boolean {
  return hasAll(t.agents) && t.coordinator !== null && tail(t.coordinator) === COORD;
}

function isTutorialRoom(wg: Workgroup): boolean {
  const coord = wg.agents.find((r) => tail(r.name) === COORD);
  return hasAll(wg.agents.map((r) => r.name)) && coord?.isCoordinator === true;
}

function goal(p: ProjectState): Goal {
  const tails = p.agents.map((a) => tail(a.name));
  const index = AGENTS.findIndex((a) => !tails.includes(a.name));
  const agent = AGENTS[index];
  if (agent) return { key: `agent:${index}`, kind: "agent", index, agent };
  const team = p.teams.find(isTutorialTeam);
  if (!team) return { key: "team", kind: "team" };
  const room = p.workgroups.find(isTutorialRoom);
  if (room) return { key: "finished", kind: "finished", room };
  return { key: "room", kind: "room", team };
}

function kindOf(g: Goal): { menu: Kind; dialog: string } {
  if (g.kind === "agent") return { menu: "newAgent", dialog: IDS.agentModal };
  if (g.kind === "team") return { menu: "newTeam", dialog: IDS.teamModal };
  return { menu: "newRoom", dialog: IDS.roomModal };
}

const samePath = (a: string, b: string) =>
  normalizeProjectPathForCompare(a) === normalizeProjectPathForCompare(b);

function findProject(probe: Probe, path: string | null): ProjectState | undefined {
  if (path === null) return undefined;
  return probe.projects.find((p) => samePath(p.path, path));
}

function next(ctx: RunCtx, patch: Partial<RunCtx>): RunCtx {
  return { ...ctx, ...patch };
}

const isFresh = (ctx: RunCtx, probe: Probe) => probe.outcome.seq > ctx.seq;
const isDisabled = (probe: Probe) => probe.state(IDS.newOpen) === NEW_OPEN_DISABLED;

function advanceFolder(ctx: RunCtx, probe: Probe): RunCtx {
  const o = probe.outcome;
  if (!isFresh(ctx, probe)) {
    if (isDisabled(probe)) return ctx;
    return next(ctx, { step: "openMenu", note: NOTE_NO_FOLDER });
  }
  if (!o.created) {
    const note = o.path === null ? NOTE_NO_FOLDER : NOTE_NOT_EMPTY;
    return next(ctx, { step: "openMenu", seq: o.seq, note });
  }
  const p = findProject(probe, o.path);
  if (!p) return ctx;
  return next(ctx, { step: "header", seq: o.seq, projectPath: p.path, note: null });
}

function advanceMenuItem(ctx: RunCtx, p: ProjectState, probe: Probe): RunCtx {
  const g = goal(p);
  const k = kindOf(g);
  if (probe.has(k.dialog)) {
    const count = g.kind === "agent" ? p.agents.length : g.kind === "team" ? p.teams.length : 0;
    return next(ctx, { step: "dialog", goalKey: g.key, count });
  }
  if (!probe.has(projectMenuId(k.menu, pid(p)))) return next(ctx, { step: "header" });
  return ctx;
}

function closedNote(ctx: RunCtx, p: ProjectState, g: Goal): string | null {
  if (g.kind === "agent" && p.agents.length > ctx.count) return NOTE_NAME(g.agent.name);
  if (g.kind === "team" && p.teams.length > ctx.count) return NOTE_TEAM;
  return null;
}

function advanceDialog(ctx: RunCtx, p: ProjectState, probe: Probe): RunCtx {
  const g = goal(p);
  if (g.key !== ctx.goalKey) return next(ctx, { step: "header", note: null });
  if (probe.has(kindOf(g).dialog)) return ctx;
  return next(ctx, { step: "header", note: closedNote(ctx, p, g) });
}

function advanceWithProject(ctx: RunCtx, p: ProjectState, probe: Probe): RunCtx {
  if (ctx.step === "header") {
    const menu = projectMenuId(kindOf(goal(p)).menu, pid(p));
    return probe.has(menu) ? next(ctx, { step: "menuItem" }) : ctx;
  }
  if (ctx.step === "menuItem") return advanceMenuItem(ctx, p, probe);
  if (ctx.step === "dialog") return advanceDialog(ctx, p, probe);
  return ctx;
}

function advanceNoProject(ctx: RunCtx, probe: Probe): RunCtx {
  if (ctx.step === "openMenu") {
    return probe.has(IDS.menu) ? next(ctx, { step: "newProject" }) : ctx;
  }
  if (ctx.step === "newProject") {
    if (isDisabled(probe) || isFresh(ctx, probe)) return next(ctx, { step: "folder", note: null });
    return probe.has(IDS.menu) ? ctx : next(ctx, { step: "openMenu" });
  }
  if (ctx.step === "folder") return advanceFolder(ctx, probe);
  return ctx;
}

/** One rule step. Returns the same object when nothing changes. */
export function advance(ctx: RunCtx, probe: Probe): RunCtx {
  if (ctx.projectPath === null) return advanceNoProject(ctx, probe);
  const p = findProject(probe, ctx.projectPath);
  if (!p) return next(ctx, { step: "openMenu", projectPath: null });
  if (ctx.step === "done") return ctx;
  if (goal(p).kind === "finished") return next(ctx, { step: "done" });
  return advanceWithProject(ctx, p, probe);
}

const plain = (target: string | null, text: string): StepView =>
  ({ target, text, warn: false, final: false });

function headerText(g: Goal): string {
  const tip = "Right-click the project name to see what you can create.";
  if (g.kind === "agent") {
    return `Agent ${g.index + 1} of 3: ${g.agent.label}. ${tip}`;
  }
  return g.kind === "team" ? `Now the team. ${tip}` : `Now the room. ${tip}`;
}

function teamDialogView(probe: Probe): StepView {
  const s = probe.state(IDS.teamModal);
  if (s === TEAM_STEPS[1]) {
    return plain(sel(IDS.teamAgents), "Check personal-assistant, accountant and " +
      "social-media-expert. Then choose Coord next to personal-assistant.");
  }
  if (s === TEAM_STEPS[2]) return plain(sel(IDS.teamCreate), "Click Create.");
  return plain(sel(IDS.teamName), `Name the team ${TEAM_NAME}, then click Next.`);
}

function dialogView(g: Goal, probe: Probe): StepView {
  if (g.kind === "agent") {
    const n = g.agent.name;
    return plain(sel(IDS.agentName),
      `Name it ${n} (no spaces), then click Create. Keep No template.`);
  }
  if (g.kind === "team") return teamDialogView(probe);
  const team = g.kind === "room" ? g.team.name : "";
  return plain(sel(IDS.roomTeam), `Choose ${team}, then click Create.`);
}

const MENU_TEXT: Record<Kind, string> =
  { newAgent: "Click New Agent.", newTeam: "Click New Team.", newRoom: "Click New Room." };

function projectView(ctx: RunCtx, p: ProjectState, probe: Probe): StepView {
  const g = goal(p);
  if (ctx.step === "done") {
    const target = g.kind === "finished" ? roomHeaderSelector(g.room.path) : null;
    return { target, text: FINAL, warn: false, final: true };
  }
  if (ctx.step === "menuItem") {
    const k = kindOf(g).menu;
    return plain(sel(projectMenuId(k, pid(p))), MENU_TEXT[k]);
  }
  if (ctx.step === "dialog") return dialogView(g, probe);
  const prefix = ctx.note ? `${ctx.note} ` : "";
  const target = sel(projectHeaderId(pid(p)));
  return { target, text: prefix + headerText(g), warn: ctx.note !== null, final: false };
}

export function view(ctx: RunCtx, probe: Probe): StepView {
  if (ctx.step === "openMenu") {
    const text = ctx.note ?? "Click New / Open to see the project options.";
    return { target: sel(IDS.newOpen), text, warn: ctx.note !== null, final: false };
  }
  if (ctx.step === "newProject") return plain(sel(IDS.menuNewProject), "Click New Project.");
  const p = findProject(probe, ctx.projectPath);
  if (ctx.step === "folder" || !p) {
    return plain(null, "Choose an empty folder for your project in the system window.");
  }
  return projectView(ctx, p, probe);
}
