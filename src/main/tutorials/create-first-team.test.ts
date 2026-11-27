// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { ProjectState } from "../../sidebar/stores/project-merge";
import type { AcAgentReplica, AcTeam, AcWorkgroup } from "../../shared/types";
import type { NewProjectOutcome } from "../../sidebar/stores/project";
import {
  advance, BOOKKEEPER_ID, NOTE_NAME, NOTE_NO_FOLDER, NOTE_NOT_EMPTY, NOTE_TEAM,
  startCtx, T_CREATE, T_DOWNLOAD, T_FAILED, T_INSTALL, T_LOADING,
  T_NAME_SKIP, T_PICK_BK, T_UNKNOWN,
  T_WRONG_BK, templateRowSelector, view, type Probe, type RunCtx, type StepView,
} from "./create-first-team";

const PATH = "/home/u/proj";
const PID = "home-u-proj";
const ALL = ["personal-assistant", "accountant", "social-media-expert"];

// #5228 item 17: literal texts (never the exported consts) so a text change fails here.
const L_NAME_PA = "Name it personal-assistant (no spaces), then write its role in " +
  "Description and click Create.";
const L_ROLE = "Now write its role in Description, then click Create. Example: \"Organizes " +
  "my day, answers my messages and asks the accountant and the social media expert for help.\"";
const L_SEARCH_BK = 'Type bookkeeper in the search box, then click "Bookkeeper & ' +
  'Controller" (Finance), change the name to accountant and click Create.';
const L_FILLED_AC = "The template filled the name. Change it to accountant, then click Create.";
const L_SUGGEST = 'We suggest "Social Media Strategist": type social media and click it, ' +
  "then name it social-media-expert and click Create. You can pick another template or " +
  "No template.";
const L_NAME_AC = "Name it accountant (no spaces), then click Create.";
const L_NAME_SM = "Name it social-media-expert (no spaces), then click Create.";
const L_CHANGE_SM = "Change the name to social-media-expert, then click Create.";
const L_FINAL = "Done! You can talk to your personal assistant (orchestrator) in the room " +
  "or in the Orchestrators section of the project.";
const L_ORCH = "We recommend talking to it here, in Orchestrators: click personal-assistant " +
  "and ask it to talk to the accountant.";
const L_ORCH_OPEN = "Open the Orchestrators section of this project. " + L_ORCH;
const L_WARN_AC =
  "The name must be exactly accountant. Name it accountant (no spaces), then click Create.";

function project(path = PATH): ProjectState {
  return {
    path, folderName: "proj", workgroups: [], agents: [], teams: [], loops: [],
    contextTemplateUpdates: [],
  };
}
const addAgent = (p: ProjectState, ref: string) =>
  p.agents.push({ name: ref, path: `${p.path}/.ac/_agent_${ref}`, roleExists: true });
const team = (name: string, names: string[], coord: string | null): AcTeam =>
  ({ name, agents: names.map((n) => `proj/${n}`), coordinator: coord && `proj/${coord}` });
function replica(name: string, isCoordinator: boolean): AcAgentReplica {
  return { name, path: `/r/__agent_${name}`, repoPaths: [], isCoordinator } as AcAgentReplica;
}
function room(path: string, names: string[], coord: string | null): AcWorkgroup {
  const agents = names.map((n) => replica(n, n === coord));
  return { name: "room-1", path, task: null, agents, teamName: "my-team" };
}

class FakeProbe implements Probe {
  ids = new Set<string>();
  states = new Map<string, string>();
  values = new Map<string, string>();
  selectors = new Set<string>();
  projects: ProjectState[] = [];
  outcome: NewProjectOutcome = { seq: 0, path: null, created: false };
  has(t: string) { return this.ids.has(t); }
  state(t: string) { return this.states.get(t) ?? null; }
  value(t: string) { return this.values.get(t) ?? null; }
  matches(s: string) { return this.selectors.has(s); }
  show(...t: string[]) { t.forEach((x) => this.ids.add(x)); return this; }
  hide(...t: string[]) { t.forEach((x) => this.ids.delete(x)); return this; }
  pick(path: string | null, created: boolean) {
    this.outcome = { seq: this.outcome.seq + 1, path, created };
  }
}

function settle(ctx: RunCtx, probe: Probe): RunCtx {
  for (let i = 0; i < 5; i++) {
    const n = advance(ctx, probe);
    if (n === ctx) return ctx;
    ctx = n;
  }
  return ctx;
}

const tid = (id: string) => `[data-ac-testid="${id}"]`;
const HEADER = tid(`project.header.${PID}`);
const menuId = (k: string) => `project.action.${k}.${PID}.projectMenu`;

/** Runs to the header step of a fresh project P. */
function toHeader(): { probe: FakeProbe; ctx: RunCtx; p: ProjectState } {
  const probe = new FakeProbe();
  let ctx = startCtx(0);
  probe.show("actionBar.menu");
  ctx = settle(ctx, probe);
  probe.hide("actionBar.menu");
  probe.states.set("actionBar.newOpen", "disabled");
  ctx = settle(ctx, probe);
  const p = project();
  probe.projects.push(p);
  probe.states.set("actionBar.newOpen", "closed");
  probe.pick(PATH, true);
  ctx = settle(ctx, probe);
  expect(ctx.step).toBe("header");
  return { probe, ctx, p };
}

const BK_ROW = templateRowSelector(BOOKKEEPER_ID);
const NAME = tid("newAgent.name");
const CREATE = tid("newAgent.create");
const SEARCH = tid("newAgent.templateSearch");
const INSTALL_BTN = tid("newAgent.installTemplates");

/** Fills the agent dialog markers (P07): name value/state, template, query, agency. */
function dlg(probe: FakeProbe, m: { name?: string; typed?: boolean; tpl?: string; q?: string;
  agency?: string; install?: string; desc?: string; row?: boolean }) {
  const set = (map: Map<string, string>, k: string, v: string | undefined) =>
    v === undefined ? map.delete(k) : map.set(k, v);
  set(probe.values, "newAgent.name", m.name);
  set(probe.states, "newAgent.name", m.typed ? "typed" : "auto");
  set(probe.states, "newAgent.template.list", m.tpl);
  set(probe.values, "newAgent.templateSearch", m.q);
  set(probe.states, "newAgent.modal", m.agency);
  const failed = m.install === "failed";
  set(probe.states, "newAgent.installTemplates", failed ? "idle" : m.install);
  if (failed) probe.show("newAgent.installTemplates.error");
  else probe.hide("newAgent.installTemplates.error");
  set(probe.states, "newAgent.description", m.desc);
  if (m.row === false) probe.selectors.delete(BK_ROW);
  else probe.selectors.add(BK_ROW);
}
const v = (target: string, text: string, warn = false, skip = false): StepView =>
  ({ target, text, warn, final: false, skip });
const LIST = '[data-ac-testid="newAgent.template.list"]';
const vg = (t: string, x: string, w = false): StepView => ({ ...v(t, x, w), group: LIST });

/** Creates the agents before `index` and opens the dialog of agent `index`. */
function toAgent(index: number): { probe: FakeProbe; ctx: RunCtx; p: ProjectState } {
  const h = toHeader();
  ALL.slice(0, index).forEach((n) => addAgent(h.p, `proj/${n}`));
  const ctx = openDialog(h.probe, settle(h.ctx, h.probe), "newAgent", "newAgent.modal");
  expect(ctx.goalKey).toBe(`agent:${index}`);
  return { ...h, ctx };
}

/** Opens the menu and dialog of the current goal. */
function openDialog(probe: FakeProbe, ctx: RunCtx, kind: string, modal: string): RunCtx {
  probe.show(menuId(kind));
  ctx = settle(ctx, probe);
  expect(ctx.step).toBe("menuItem");
  probe.hide(menuId(kind)).show(modal);
  ctx = settle(ctx, probe);
  expect(ctx.step).toBe("dialog");
  return ctx;
}

/** Walks one agent dialog through its rows until Create is the target. */
function walkAgent(probe: FakeProbe, ctx: RunCtx, i: number, name: string) {
  if (i === 0) {
    dlg(probe, {});
    expect(view(ctx, probe)).toEqual(v(NAME, L_NAME_PA));
    dlg(probe, { name, typed: true });
    expect(view(ctx, probe)).toEqual(v(tid("newAgent.description"), L_ROLE));
    dlg(probe, { name, typed: true, desc: "filled" });
  } else if (i === 1) {
    dlg(probe, { agency: "ready" });
    expect(view(ctx, probe)).toEqual(vg(SEARCH, L_SEARCH_BK));
    dlg(probe, { agency: "ready", q: "bookkeeper" });
    expect(view(ctx, probe)).toEqual(vg(BK_ROW, T_PICK_BK));
    dlg(probe, { agency: "ready", q: "bookkeeper", tpl: BOOKKEEPER_ID, name: "bk" });
    expect(view(ctx, probe)).toEqual(v(NAME, L_FILLED_AC));
    dlg(probe, { agency: "ready", q: "bookkeeper", tpl: BOOKKEEPER_ID, name, typed: true });
  } else {
    dlg(probe, { agency: "ready" });
    expect(view(ctx, probe)).toEqual(vg(SEARCH, L_SUGGEST));
    dlg(probe, { agency: "ready", name, typed: true });
  }
  expect(view(ctx, probe)).toEqual(v(CREATE, T_CREATE));
}

describe("create-first-team rules", () => {
  it("1. happy path from start to done", () => {
    const probe = new FakeProbe();
    let ctx = startCtx(0);
    expect(view(ctx, probe)).toEqual({ target: tid("actionBar.newOpen"),
      text: "Click New / Open to see the project options.", warn: false, final: false,
      skip: false });
    probe.show("actionBar.menu");
    ctx = settle(ctx, probe);
    expect(ctx.step).toBe("newProject");
    expect(view(ctx, probe)).toMatchObject(
      { target: tid("actionBar.menu.newProject"), text: "Click New Project." });
    probe.hide("actionBar.menu");
    probe.states.set("actionBar.newOpen", "disabled");
    ctx = settle(ctx, probe);
    expect(ctx.step).toBe("folder");
    expect(view(ctx, probe)).toMatchObject({ target: null,
      text: "Choose an empty folder for your project in the system window." });
    const p = project();
    probe.projects.push(p);
    probe.states.set("actionBar.newOpen", "closed");
    probe.pick(PATH, true);
    ctx = settle(ctx, probe);
    expect(ctx).toMatchObject({ step: "header", projectPath: PATH, seq: 1 });

    const labels = ["personal assistant", "accountant", "social media expert"];
    ALL.forEach((name, i) => {
      expect(view(ctx, probe)).toEqual({ target: HEADER, warn: false, final: false, skip: false,
        text: `Agent ${i + 1} of 3: ${labels[i]}. ` +
          "Right-click the project name to see what you can create." });
      probe.show(menuId("newAgent"));
      ctx = settle(ctx, probe);
      expect(ctx.step).toBe("menuItem");
      expect(view(ctx, probe)).toMatchObject(
        { target: tid(menuId("newAgent")), text: "Click New Agent." });
      probe.hide(menuId("newAgent")).show("newAgent.modal");
      ctx = settle(ctx, probe);
      expect(ctx).toMatchObject({ step: "dialog", goalKey: `agent:${i}`, count: i });
      walkAgent(probe, ctx, i, name);
      probe.hide("newAgent.modal");
      addAgent(p, `proj/${name}`);
      ctx = settle(ctx, probe);
      expect(ctx).toMatchObject({ step: "header", note: null });
    });

    expect(view(ctx, probe).text).toBe(
      "Now the team. Right-click the project name to see what you can create.");
    ctx = openDialog(probe, ctx, "newTeam", "newTeam.modal");
    expect(view(ctx, probe)).toMatchObject(
      { target: tid("newTeam.name"), text: "Name the team my-team, then click Next." });
    probe.states.set("newTeam.modal", "step-2");
    expect(view(ctx, probe)).toMatchObject({ target: tid("newTeam.agents"),
      text: "Check personal-assistant, accountant and social-media-expert. " +
        "Then choose Coord next to personal-assistant." });
    probe.states.set("newTeam.modal", "step-3");
    expect(view(ctx, probe)).toMatchObject(
      { target: tid("newTeam.create"), text: "Click Create." });
    probe.hide("newTeam.modal");
    p.teams.push(team("my-team", ALL, "personal-assistant"));
    ctx = settle(ctx, probe);
    expect(ctx).toMatchObject({ step: "header", note: null });

    expect(view(ctx, probe).text).toBe(
      "Now the room. Right-click the project name to see what you can create.");
    probe.show(menuId("newRoom"));
    ctx = settle(ctx, probe);
    expect(view(ctx, probe)).toMatchObject(
      { target: tid(menuId("newRoom")), text: "Click New Room." });
    probe.hide(menuId("newRoom")).show("newRoom.modal");
    ctx = settle(ctx, probe);
    expect(view(ctx, probe)).toMatchObject({ target: tid("newRoom.teamSearch"),
      text: "Choose my-team, then click Create.",
      group: '[data-ac-testid="newRoom.team.list"]' });
    probe.hide("newRoom.modal");
    p.workgroups.push(room("/home/u/proj/.ac/room-1", ALL, "personal-assistant"));
    ctx = settle(ctx, probe);
    expect(ctx.step).toBe("done");
    expect(view(ctx, probe)).toEqual({ target: '.ac-wg-header[title="/home/u/proj/.ac/room-1"]',
      text: L_FINAL, warn: false, final: false, skip: false, next: true });
  });

  function atFolder(): { probe: FakeProbe; ctx: RunCtx } {
    const probe = new FakeProbe().show("actionBar.menu");
    let ctx = settle(startCtx(0), probe);
    probe.hide("actionBar.menu");
    probe.states.set("actionBar.newOpen", "disabled");
    ctx = settle(ctx, probe);
    expect(ctx.step).toBe("folder");
    return { probe, ctx };
  }

  it("2. folder window cancelled", () => {
    const { probe, ctx } = atFolder();
    probe.pick(null, false);
    const out = advance(ctx, probe);
    expect(out).toMatchObject({ step: "openMenu", note: NOTE_NO_FOLDER, seq: 1 });
    expect(view(out, probe)).toMatchObject({ text: NOTE_NO_FOLDER, warn: true });
  });

  it("3. folder of an existing project", () => {
    for (const loaded of [false, true]) {
      const { probe, ctx } = atFolder();
      if (loaded) probe.projects.push(project());
      probe.pick(PATH, false);
      expect(advance(ctx, probe))
        .toMatchObject({ step: "openMenu", note: NOTE_NOT_EMPTY, projectPath: null });
    }
  });

  it("4. agent made with another name", () => {
    let { probe, ctx, p } = toHeader();
    ctx = openDialog(probe, ctx, "newAgent", "newAgent.modal");
    probe.hide("newAgent.modal");
    addAgent(p, "proj/assistant");
    ctx = settle(ctx, probe);
    expect(ctx).toMatchObject({ step: "header", note: NOTE_NAME("personal-assistant") });
    expect(view(ctx, probe)).toMatchObject({ warn: true,
      text: "Use the name personal-assistant so the next steps can find it. Agent 1 of 3: " +
        "personal assistant. Right-click the project name to see what you can create." });
    ctx = openDialog(probe, ctx, "newAgent", "newAgent.modal");
    expect(ctx.goalKey).toBe("agent:0");
  });

  it("5. dialog or menu closed with nothing new; team page Back", () => {
    let { probe, ctx, p } = toHeader();
    ctx = openDialog(probe, ctx, "newAgent", "newAgent.modal");
    probe.hide("newAgent.modal");
    expect(settle(ctx, probe)).toMatchObject({ step: "header", note: null });
    probe.show(menuId("newAgent"));
    ctx = settle(ctx, probe);
    probe.hide(menuId("newAgent"));
    expect(settle(ctx, probe).step).toBe("header");
    ALL.forEach((n) => addAgent(p, `proj/${n}`));
    ctx = openDialog(probe, settle(ctx, probe), "newTeam", "newTeam.modal");
    probe.states.set("newTeam.modal", "step-2");
    probe.states.set("newTeam.modal", "step-1");
    expect(view(ctx, probe).text).toBe("Name the team my-team, then click Next.");
  });

  it("6. invalid teams warn; a valid team with any name works", () => {
    for (const bad of [team("my-team", ALL.slice(0, 2), "personal-assistant"),
      team("my-team", ALL, "accountant")]) {
      let { probe, ctx, p } = toHeader();
      ALL.forEach((n) => addAgent(p, `proj/${n}`));
      ctx = openDialog(probe, settle(ctx, probe), "newTeam", "newTeam.modal");
      probe.hide("newTeam.modal");
      p.teams.push(bad);
      ctx = settle(ctx, probe);
      expect(ctx).toMatchObject({ step: "header", note: NOTE_TEAM });
      p.teams.push(team("team-b", ALL, "personal-assistant"));
      ctx = openDialog(probe, settle(ctx, probe), "newRoom", "newRoom.modal");
      expect(view(ctx, probe).text).toBe("Choose team-b, then click Create.");
    }
  });

  it("7. agents created out of order", () => {
    const { probe, ctx, p } = toHeader();
    addAgent(p, "proj/social-media-expert");
    addAgent(p, "proj/personal-assistant");
    expect(view(ctx, probe).text).toContain("Agent 2 of 3: accountant.");
  });

  it("8. P removed; done target escapes the path", () => {
    let { probe, ctx, p } = toHeader();
    probe.projects = [];
    expect(advance(ctx, probe)).toMatchObject({ step: "openMenu", projectPath: null });
    probe.projects = [p];
    ALL.forEach((n) => addAgent(p, `proj/${n}`));
    p.teams.push(team("my-team", ALL, "personal-assistant"));
    p.workgroups.push(room('/a"b\\c', ALL, "personal-assistant"));
    ctx = settle(ctx, probe);
    expect(ctx.step).toBe("done");
    expect(view(ctx, probe).target).toBe('.ac-wg-header[title="/a\\"b\\\\c"]');
  });

  it("9. raw _agent_ refs count the same", () => {
    const { probe, ctx, p } = toHeader();
    addAgent(p, "_agent_personal-assistant");
    addAgent(p, "_agent_accountant");
    addAgent(p, "proj/social-media-expert");
    p.teams.push({ name: "t", agents: ALL.map((n) => `_agent_${n}`),
      coordinator: "_agent_personal-assistant" });
    expect(view(ctx, probe).text).toContain("Now the room.");
  });

  it("10. an existing empty project never becomes P", () => {
    let { probe, ctx } = atFolder();
    probe.projects.push(project());
    probe.pick(PATH, false);
    expect(advance(ctx, probe))
      .toMatchObject({ step: "openMenu", note: NOTE_NOT_EMPTY, projectPath: null });
    ({ probe, ctx } = atFolder());
    probe.states.set("actionBar.newOpen", "closed");
    probe.projects.push(project());
    expect(advance(ctx, probe))
      .toMatchObject({ step: "openMenu", note: NOTE_NO_FOLDER, projectPath: null });
  });

  it("11. old outcome ignored; created project waits for the store", () => {
    const probe = new FakeProbe();
    probe.outcome = { seq: 5, path: PATH, created: true };
    probe.projects.push(project());
    let ctx = startCtx(5);
    probe.states.set("actionBar.newOpen", "disabled");
    probe.show("actionBar.menu");
    ctx = settle(ctx, probe);
    expect(ctx).toMatchObject({ step: "folder", projectPath: null });
    probe.projects = [];
    probe.pick(PATH, true);
    expect(advance(ctx, probe)).toBe(ctx);
    probe.projects.push(project());
    expect(advance(ctx, probe)).toMatchObject({ step: "header", projectPath: PATH, seq: 6 });
  });

  it("12. first valid team named; incomplete rooms are not done", () => {
    let { probe, ctx, p } = toHeader();
    ALL.forEach((n) => addAgent(p, `proj/${n}`));
    p.teams.push(team("alpha", ALL, "personal-assistant"));
    p.teams.push(team("beta", ALL, "personal-assistant"));
    ctx = openDialog(probe, settle(ctx, probe), "newRoom", "newRoom.modal");
    expect(view(ctx, probe).text).toBe("Choose alpha, then click Create.");
    p.workgroups.push(room("/r1", ALL, null));
    p.workgroups.push(room("/r2", ALL.slice(0, 2), "personal-assistant"));
    expect(settle(ctx, probe).step).not.toBe("done");
  });

  it("14. wrong my-team sorts first; the room still finishes", () => {
    let { probe, ctx, p } = toHeader();
    ALL.forEach((n) => addAgent(p, `proj/${n}`));
    p.teams.push(team("my-team", ALL.slice(0, 2), "personal-assistant"));
    p.teams.push(team("my-team-2", ALL, "personal-assistant"));
    p.workgroups.push(room("/proj/.ac/room-2", ALL, "personal-assistant"));
    ctx = settle(ctx, probe);
    expect(ctx.step).toBe("done");
    expect(view(ctx, probe).target).toBe('.ac-wg-header[title="/proj/.ac/room-2"]');
  });

  it("A1. agent 1 rows 1-3; empty Description keeps row 2; missing is ignored", () => {
    const { probe, ctx } = toAgent(0);
    dlg(probe, { agency: "missing" });
    expect(view(ctx, probe)).toEqual(v(NAME, L_NAME_PA));
    expect(view(ctx, probe).group).toBeUndefined();
    dlg(probe, { agency: "missing", name: "personal-assistant", typed: true, desc: "empty" });
    expect(view(ctx, probe)).toEqual(v(tid("newAgent.description"), L_ROLE));
    expect(view(ctx, probe).target).not.toBe(CREATE);
    expect(view(ctx, probe).group).toBeUndefined();
    dlg(probe, { agency: "missing", name: "personal-assistant", typed: true, desc: "filled" });
    expect(view(ctx, probe)).toEqual(v(CREATE, T_CREATE));
    expect(view(ctx, probe).group).toBeUndefined();
  });

  it("A2. agent 2 ready: search, pick, rename, create; typed name; wrong template", () => {
    const { probe, ctx } = toAgent(1);
    dlg(probe, { agency: "ready", q: "Book" });
    expect(view(ctx, probe)).toEqual(vg(SEARCH, L_SEARCH_BK));
    dlg(probe, { agency: "ready", q: "BOOKKEEPER" });
    expect(view(ctx, probe)).toEqual(vg(BK_ROW, T_PICK_BK));
    dlg(probe, { agency: "ready", q: "bookkeeper", tpl: "local:other", name: "other" });
    expect(view(ctx, probe)).toEqual(vg(BK_ROW, T_WRONG_BK, true));
    dlg(probe, { agency: "ready", q: "bookkeeper", tpl: BOOKKEEPER_ID, name: "x" });
    expect(view(ctx, probe)).toEqual(v(NAME, L_FILLED_AC));
    dlg(probe, { agency: "ready", q: "bookkeeper", tpl: BOOKKEEPER_ID, name: "x", typed: true });
    expect(view(ctx, probe)).toEqual(v(NAME, L_WARN_AC, true));
    dlg(probe, { agency: "ready", tpl: BOOKKEEPER_ID, name: "accountant", typed: true });
    expect(view(ctx, probe)).toEqual(v(CREATE, T_CREATE));
  });

  it("A3. agent 2 missing: install, installing, failed, then ready in the same step", () => {
    let { probe, ctx } = toAgent(1);
    dlg(probe, { agency: "missing", install: "idle" });
    expect(view(ctx, probe)).toEqual({ ...v(INSTALL_BTN, T_INSTALL, false, true), link: true });
    expect(view(ctx, probe).group).toBeUndefined();
    dlg(probe, { agency: "missing", install: "installing" });
    expect(view(ctx, probe)).toEqual({ ...v(INSTALL_BTN, T_DOWNLOAD, false, true), link: true });
    dlg(probe, { agency: "missing", install: "failed" });
    expect(view(ctx, probe)).toEqual({ ...v(INSTALL_BTN, T_FAILED, true, true), link: true });
    dlg(probe, { agency: "ready" });
    expect(settle(ctx, probe)).toBe(ctx);
    expect(view(ctx, probe)).toEqual(vg(SEARCH, L_SEARCH_BK));
  });

  it("A4. skip survives steps for agents 2 and 3; a new run resets it", () => {
    let { probe, ctx, p } = toAgent(1);
    dlg(probe, { agency: "missing" });
    expect(view(ctx, probe).skip).toBe(true);
    ctx = { ...ctx, skip: true };
    expect(view(ctx, probe)).toEqual(v(NAME, T_NAME_SKIP("accountant")));
    dlg(probe, { agency: "missing", name: "accountant", typed: true });
    expect(view(ctx, probe)).toEqual(v(CREATE, T_CREATE));
    probe.hide("newAgent.modal");
    addAgent(p, "proj/accountant");
    ctx = settle(ctx, probe);
    expect(ctx).toMatchObject({ step: "header", skip: true });
    ctx = openDialog(probe, ctx, "newAgent", "newAgent.modal");
    expect(ctx).toMatchObject({ goalKey: "agent:2", skip: true });
    dlg(probe, { agency: "missing" });
    expect(view(ctx, probe)).toEqual(v(NAME, T_NAME_SKIP("social-media-expert")));
    dlg(probe, { agency: "missing", name: "social-media-expert", typed: true });
    expect(view(ctx, probe)).toEqual(v(CREATE, T_CREATE));
    expect(startCtx(9).skip).toBe(false);
  });

  it("A5. unknown acts like skip with a prefix; with skip set, no prefix", () => {
    const { probe, ctx } = toAgent(1);
    dlg(probe, { agency: "unknown" });
    expect(view(ctx, probe)).toEqual(v(NAME, `${T_UNKNOWN} ${T_NAME_SKIP("accountant")}`));
    expect(view(ctx, probe).text).not.toContain("Install");
    dlg(probe, { agency: "unknown", name: "accountant" });
    expect(view(ctx, probe)).toEqual(v(CREATE, `${T_UNKNOWN} ${T_CREATE}`));
    expect(view({ ...ctx, skip: true }, probe)).toEqual(v(CREATE, T_CREATE));
  });

  it("A6. loading (or no state) targets the modal", () => {
    const { probe, ctx } = toAgent(2);
    dlg(probe, { agency: "loading" });
    expect(view(ctx, probe)).toEqual(v(tid("newAgent.modal"), T_LOADING));
    expect(view(ctx, probe).group).toBeUndefined();
    dlg(probe, {});
    expect(view(ctx, probe)).toEqual(v(tid("newAgent.modal"), T_LOADING));
  });

  it("A7. agent 3: suggestion, change after a pick, typed name; any template ok", () => {
    let { probe, ctx } = toAgent(2);
    dlg(probe, { agency: "ready", tpl: "none" });
    expect(view(ctx, probe)).toEqual(vg(SEARCH, L_SUGGEST));
    dlg(probe, { agency: "ready", tpl: "agency:marketing-x", name: "x" });
    expect(view(ctx, probe)).toEqual(v(NAME, L_CHANGE_SM));
    ctx = settle(ctx, probe);
    expect(ctx.filled).toBe("x");
    dlg(probe, { agency: "ready", tpl: "agency:marketing-x", name: "x", typed: true });
    expect(view(ctx, probe)).toEqual(v(NAME, L_NAME_SM));
    dlg(probe, { agency: "ready", tpl: "agency:marketing-x", name: "social-media-expert",
      typed: true });
    expect(view(ctx, probe)).toEqual(v(CREATE, T_CREATE));
    dlg(probe, { agency: "ready", name: "s", typed: true });
    expect(view(ctx, probe)).toEqual(v(NAME, L_NAME_SM));
  });

  it("A8. accountant with No template and the right name advances, no note", () => {
    let { probe, ctx, p } = toAgent(1);
    dlg(probe, { agency: "ready", name: "accountant", typed: true });
    probe.hide("newAgent.modal");
    addAgent(p, "proj/accountant");
    ctx = settle(ctx, probe);
    expect(ctx).toMatchObject({ step: "header", note: null });
    expect(view(ctx, probe).text).toContain("Agent 3 of 3");
  });

  it("A9. Bookkeeper row gone (9b-9c); a filtered-out row is not gone", () => {
    const { probe, ctx } = toAgent(1);
    for (const q of ["", "bookkeeper"]) {
      dlg(probe, { agency: "ready", q, row: false });
      expect(view(ctx, probe)).toEqual(v(NAME, T_NAME_SKIP("accountant")));
      dlg(probe, { agency: "ready", q, row: false, name: "accountant" });
      expect(view(ctx, probe)).toEqual(v(CREATE, T_CREATE));
    }
    dlg(probe, { agency: "ready", q: "tax", row: false });
    expect(view(ctx, probe)).toEqual(vg(SEARCH, L_SEARCH_BK));
  });

  it("A10. no text shows the slug; the old agent line is gone", () => {
    const texts: string[] = [];
    for (const i of [0, 1, 2]) {
      const { probe, ctx } = toAgent(i);
      for (const agency of ["ready", "missing", "unknown", "loading"]) {
        for (const tpl of [undefined, BOOKKEEPER_ID, "local:x"]) {
          dlg(probe, { agency, tpl, q: "bookkeeper", install: "failed" });
          texts.push(view(ctx, probe).text, view({ ...ctx, skip: true }, probe).text);
        }
      }
    }
    for (const t of texts) {
      expect(t).not.toContain("finance-bookkeeper-controller");
      expect(t).not.toContain("Keep No template");
    }
  });

  const W_PA = "The name must be exactly personal-assistant. Name it personal-assistant (no " +
    "spaces), then write its role in Description and click Create.";
  const W_SM = "The name must be exactly social-media-expert. Name it social-media-expert (no " +
    "spaces), then click Create.";
  const W_AC_SKIP = "The name must be exactly accountant. Name it accountant (no spaces), then " +
    "click Create. No template is fine.";
  const W_AC_UNKNOWN = "The name must be exactly accountant. Could not check the templates. " +
    "Name it accountant (no spaces), then click Create. No template is fine.";
  const shown: string[] = [];
  const see = (ctx: RunCtx, probe: Probe, want: StepView) => {
    const got = view(ctx, probe);
    shown.push(got.text);
    expect(got).toEqual(want);
  };

  it("W1.1 wrong typed first name warns", () => {
    const { probe, ctx } = toAgent(0);
    dlg(probe, { name: "personal-assitant", typed: true });
    see(ctx, probe, v(NAME, W_PA, true));
  });

  it("W1.2 wrong typed name with Bookkeeper warns", () => {
    const { probe, ctx } = toAgent(1);
    dlg(probe, { agency: "ready", tpl: BOOKKEEPER_ID, name: "acountant", typed: true });
    see(ctx, probe, v(NAME, L_WARN_AC, true));
  });

  it("W1.3 wrong typed third name with No template warns", () => {
    const { probe, ctx } = toAgent(2);
    dlg(probe, { agency: "ready", tpl: "none", name: "social-medai", typed: true });
    see(ctx, probe, v(NAME, W_SM, true));
  });

  it("W1.4 wrong typed name after skip warns", () => {
    const { probe, ctx } = toAgent(1);
    dlg(probe, { agency: "missing", name: "acountant", typed: true });
    see({ ...ctx, skip: true }, probe, v(NAME, W_AC_SKIP, true));
  });

  it("W1.5 wrong typed name with unknown templates warns, T_EXACT first", () => {
    const { probe, ctx } = toAgent(1);
    dlg(probe, { agency: "unknown", name: "acountant", typed: true });
    see(ctx, probe, v(NAME, W_AC_UNKNOWN, true));
  });

  it("W1.6 wrong typed name with the Bookkeeper row missing warns", () => {
    const { probe, ctx } = toAgent(1);
    dlg(probe, { agency: "ready", q: "", row: false, name: "acountant", typed: true });
    see(ctx, probe, v(NAME, W_AC_SKIP, true));
  });

  it("W2 a start of the name, empty, auto or right name: no warn", () => {
    const a = toAgent(0);
    dlg(a.probe, { name: "personal-", typed: true });
    see(a.ctx, a.probe, v(NAME, L_NAME_PA));
    dlg(a.probe, { name: "", typed: true });
    see(a.ctx, a.probe, v(NAME, L_NAME_PA));
    dlg(a.probe, { name: "personal-assistant", typed: true });
    see(a.ctx, a.probe, v(tid("newAgent.description"), L_ROLE));
    const b = toAgent(1);
    dlg(b.probe, { agency: "ready", tpl: BOOKKEEPER_ID, name: "bookkeeper-controller" });
    see(b.ctx, b.probe, v(NAME, L_FILLED_AC));
    const c = toAgent(2);
    dlg(c.probe, { agency: "ready", tpl: "none", name: "social-media", typed: true });
    see(c.ctx, c.probe, v(NAME, L_NAME_SM));
  });

  it("W3-W5 erase a filled name is quiet; a typo warns; closing resets filled", () => {
    let { probe, ctx } = toAgent(1);
    const at = (name: string, typed: boolean) =>
      dlg(probe, { agency: "ready", tpl: BOOKKEEPER_ID, name, typed });
    at("bookkeeper-controller", false);
    ctx = settle(ctx, probe);
    expect(ctx.filled).toBe("bookkeeper-controller");
    for (const name of ["bookkeeper-controlle", "b"]) {
      at(name, true);
      ctx = settle(ctx, probe);
      see(ctx, probe, v(NAME, L_NAME_AC));
    }
    for (const name of ["acountant", "bookkeeper-x"]) {
      at(name, true);
      ctx = settle(ctx, probe);
      see(ctx, probe, v(NAME, L_WARN_AC, true));
    }
    probe.hide("newAgent.modal");
    ctx = settle(ctx, probe);
    expect(ctx.filled).toBe(null);
  });

  it("W5b opening the dialog resets filled", () => {
    const h = toHeader();
    addAgent(h.p, "proj/personal-assistant");
    let ctx = settle(h.ctx, h.probe);
    h.probe.show(menuId("newAgent"));
    ctx = settle(ctx, h.probe);
    expect(ctx.step).toBe("menuItem");
    ctx = { ...ctx, filled: "bookkeeper-controller" };
    h.probe.hide(menuId("newAgent")).show("newAgent.modal");
    dlg(h.probe, { name: "", agency: "ready" });
    ctx = settle(ctx, h.probe);
    expect(ctx.step).toBe("dialog");
    expect(ctx.filled).toBe(null);
  });

  it("W6 (extra) every W1-W3, search and suggest text names the button", () => {
    expect(shown.length).toBeGreaterThan(10);
    for (const t of [...shown, L_SEARCH_BK, L_SUGGEST]) {
      expect(t).toMatch(/click (Create|Next)\.( |$)/);
    }
  });
});

describe("#5228 P12 final bubbles and agency link", () => {
  const ROOM = "/home/u/proj/.ac/room-1";
  const ROW = "replica.row.quick.r-__agent_personal-assistant";
  function finished(): { probe: FakeProbe; ctx: RunCtx; p: ProjectState } {
    const h = toHeader();
    ALL.forEach((n) => addAgent(h.p, `proj/${n}`));
    h.p.teams.push(team("my-team", ALL, "personal-assistant"));
    h.p.workgroups.push(room(ROOM, ALL, "personal-assistant"));
    const ctx = settle(h.ctx, h.probe);
    expect(ctx.step).toBe("done");
    return { ...h, ctx };
  }

  it("done view: FINAL text, Next, not final, room header target", () => {
    const { probe, ctx } = finished();
    expect(view(ctx, probe)).toEqual({ target: `.ac-wg-header[title="${ROOM}"]`,
      text: L_FINAL, warn: false, final: false, skip: false, next: true });
  });

  it("orch view: row present rings it; row missing shows the corner text", () => {
    const { probe, ctx: done } = finished();
    const ctx: RunCtx = { ...done, step: "orch" };
    expect(view(ctx, probe)).toEqual({ target: null, text: L_ORCH_OPEN, warn: false,
      final: true, skip: false });
    probe.show(ROW);
    expect(view(ctx, probe)).toEqual({
      target: '[data-ac-testid="replica.row.quick.r-__agent_personal-assistant"]',
      text: L_ORCH, warn: false, final: true, skip: false });
  });

  it("advance at orch stays; project gone resets; goal lost shows the corner text", () => {
    const { probe, ctx: done, p } = finished();
    const ctx: RunCtx = { ...done, step: "orch" };
    probe.show(ROW);
    expect(advance(ctx, probe)).toBe(ctx);
    p.workgroups = [];
    expect(advance(ctx, probe)).toBe(ctx);
    expect(view(ctx, probe)).toMatchObject({ target: null, text: L_ORCH_OPEN, final: true });
    probe.projects = [];
    expect(advance(ctx, probe)).toMatchObject({ step: "openMenu", projectPath: null });
  });

  it("only install rows have link; name and Create rows do not", () => {
    const { probe, ctx } = toAgent(1);
    for (const install of ["idle", "installing", "failed"]) {
      dlg(probe, { agency: "missing", install });
      expect(view(ctx, probe).link).toBe(true);
    }
    const a = toAgent(0);
    dlg(a.probe, { name: "x", typed: true });
    expect(view(a.ctx, a.probe).link).toBeUndefined();
    dlg(a.probe, { name: "personal-assistant", typed: true, desc: "filled" });
    expect(view(a.ctx, a.probe).link).toBeUndefined();
    const b = toAgent(1);
    dlg(b.probe, { agency: "ready", tpl: BOOKKEEPER_ID, name: "accountant", typed: true });
    expect(view(b.ctx, b.probe).link).toBeUndefined();
  });
});
