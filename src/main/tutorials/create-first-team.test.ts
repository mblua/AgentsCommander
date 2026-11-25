// @vitest-environment node
import { describe, expect, it } from "vitest";
import type { ProjectState } from "../../sidebar/stores/project-merge";
import type { AcAgentReplica, AcTeam, AcWorkgroup } from "../../shared/types";
import type { NewProjectOutcome } from "../../sidebar/stores/project";
import {
  advance, FINAL, NOTE_NAME, NOTE_NO_FOLDER, NOTE_NOT_EMPTY, NOTE_TEAM, startCtx, view,
  type Probe, type RunCtx,
} from "./create-first-team";

const PATH = "/home/u/proj";
const PID = "home-u-proj";
const ALL = ["personal-assistant", "accountant", "social-media-expert"];

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
  projects: ProjectState[] = [];
  outcome: NewProjectOutcome = { seq: 0, path: null, created: false };
  has(t: string) { return this.ids.has(t); }
  state(t: string) { return this.states.get(t) ?? null; }
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

describe("create-first-team rules", () => {
  it("1. happy path from start to done", () => {
    const probe = new FakeProbe();
    let ctx = startCtx(0);
    expect(view(ctx, probe)).toEqual({ target: tid("actionBar.newOpen"),
      text: "Click New / Open to see the project options.", warn: false, final: false });
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
      expect(view(ctx, probe)).toEqual({ target: HEADER, warn: false, final: false,
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
      expect(view(ctx, probe)).toMatchObject({ target: tid("newAgent.name"),
        text: `Name it ${name} (no spaces), then click Create. Keep No template.` });
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
      text: "Choose my-team, then click Create." });
    probe.hide("newRoom.modal");
    p.workgroups.push(room("/home/u/proj/.ac/room-1", ALL, "personal-assistant"));
    ctx = settle(ctx, probe);
    expect(ctx.step).toBe("done");
    expect(view(ctx, probe)).toEqual({ target: '.ac-wg-header[title="/home/u/proj/.ac/room-1"]',
      text: FINAL, warn: false, final: true });
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
});
