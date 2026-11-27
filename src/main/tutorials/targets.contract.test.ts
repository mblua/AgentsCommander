// @vitest-environment node
// #5228 - ties every target the tutorial reads to the source that renders it.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  AGENCY, DESC_FILLED, IDS, INSTALL, NAME_TYPED, NEW_OPEN_DISABLED, NO_TEMPLATE,
  templateRowSelector, ROOM_HEADER_CLASS, TEAM_STEPS, projectHeaderId, projectMenuId,
  orchRowId, roomHeaderSelector,
} from "./create-first-team";
import { automationIdPart } from "../../sidebar/components/replica-repo-badges";
import { BANNER, RM_IDS } from "./root-agent-message";

const moduleUrl = import.meta.url; // variable base keeps the file: URL node:fs reads
const read = (rel: string) =>
  readFileSync(new URL(rel, moduleUrl), "utf8").split("\r\n").join("\n");
const SIDEBAR = "../../sidebar/components/";
const OWNERS: Record<string, string> = {
  actionBar: `${SIDEBAR}ActionBar.tsx`,
  newAgent: `${SIDEBAR}NewEntityAgentModal.tsx`,
  newTeam: `${SIDEBAR}NewTeamModal.tsx`,
  newRoom: `${SIDEBAR}NewWorkgroupModal.tsx`,
};
const hasTestid = (src: string, id: string) => src.includes('data-ac-testid="' + id + '"');
const ownerOf = (id: string) => OWNERS[id.split(".")[0] ?? ""];
const PANEL = read(`${SIDEBAR}ProjectPanel.tsx`);
const KINDS = ["newAgent", "newTeam", "newRoom"] as const;

describe("tutorial targets contract", () => {
  it("every IDS value is rendered by its mapped owner file", () => {
    for (const [key, id] of Object.entries(IDS)) {
      const owner = ownerOf(id);
      expect(owner, `unmapped id ${key}`).toBeDefined();
      expect(hasTestid(read(owner!), id), `${id} in ${owner}`).toBe(true);
    }
  });

  it("state values match the owner sources", () => {
    const bar = read(OWNERS.actionBar!);
    const list = ["disabled", "open", "closed"];
    expect(bar).toContain('["disabled", "open", "closed"]');
    expect(list).toContain(NEW_OPEN_DISABLED);
    expect(read(OWNERS.newTeam!)).toContain("data-ac-state={`step-${step()}`}");
    expect([...TEAM_STEPS]).toEqual(["step-1", "step-2", "step-3"]);
  });

  it("project ids come from ProjectPanel templates", () => {
    expect(PANEL).toContain("const projectAutomationId = () => automationIdPart(proj.path);");
    const header = "project.header.${projectAutomationId()}";
    expect(PANEL).toContain("data-ac-testid={`" + header + "`}");
    expect(projectHeaderId("X")).toBe(header.replace("${projectAutomationId()}", "X"));
    for (const kind of KINDS) {
      const menu = `project.action.${kind}.\${projectAutomationId()}.projectMenu`;
      expect(PANEL).toContain("data-ac-testid={`" + menu + "`}");
      expect(projectMenuId(kind, "X")).toBe(menu.replace("${projectAutomationId()}", "X"));
    }
  });

  it("room header class, title and selector escaping", () => {
    const re = /class="ac-wg-header ac-wg-header--collapsible"\s+title=\{wg\.path\}/;
    expect(PANEL).toMatch(re);
    expect("ac-wg-header ac-wg-header--collapsible".startsWith(ROOM_HEADER_CLASS)).toBe(true);
    expect(roomHeaderSelector('a"b\\c')).toBe('.ac-wg-header[title="a\\"b\\\\c"]');
  });

  it("the tutorial module has no stray testid literal", () => {
    const src = read("./create-first-team.ts");
    const values = new Set<string>(Object.values(IDS));
    const matches = [...src.matchAll(/"(actionBar|newAgent|newTeam|newRoom)\.[A-Za-z.]+"/g)];
    expect(matches.length).toBeGreaterThan(0);
    for (const m of matches) expect(values.has(m[0].slice(1, -1)), m[0]).toBe(true);
  });

  it("agent dialog markers match NewEntityAgentModal (P07)", () => {
    const agm = read(OWNERS.newAgent!);
    const literals = ["data-ac-value={t.id}", 'data-ac-value="none"',
      "data-ac-value={templateQuery()}", "data-ac-value={name().trim()}", '"typed" : "auto"',
      '"filled" : "empty"', 'selectedTemplateId() ?? "none"', '"installing"',
      ...Object.values(AGENCY).map((a) => `"${a}"`)];
    for (const l of literals) expect(agm, l).toContain(l);
    expect([NAME_TYPED, DESC_FILLED, NO_TEMPLATE]).toEqual(["typed", "filled", "none"]);
    expect(INSTALL).toEqual({ installing: "installing" });
    expect(templateRowSelector("x"))
      .toBe('[data-ac-testid="newAgent.template.list"] [data-ac-value="x"]');
  });

  it("tutorial ring stacks above menus", () => {
    const zOf = (css: string, selector: string) => {
      const start = css.indexOf(selector + " {");
      const block = css.slice(start, css.indexOf("}", start));
      return Number(/z-index:\s*(\d+)/.exec(block)?.[1]);
    };
    const main = read("../styles/main.css");
    const sidebar = read("../../sidebar/styles/sidebar.css");
    const menu = zOf(sidebar, ".session-context-menu");
    for (const s of [".tutorial-ring", ".tutorial-bubble"]) {
      expect(zOf(main, s), s).toBeGreaterThan(menu);
      expect(zOf(main, s), s).toBeLessThanOrEqual(10000);
    }
    expect(zOf(".x {\n  z-index: 5;\n}", ".x")).toBe(5);
  });

  it("bubble areas: dialog and menu markers exist in their owner files", () => {
    for (const k of KINDS) expect(read(OWNERS[k]!), k).toContain('data-ac-role="dialog"');
    expect(read(OWNERS.actionBar!)).toContain('data-ac-role="menu"');
    const at = PANEL.indexOf("project.action.newAgent.");
    expect(at).toBeGreaterThan(0);
    expect(PANEL.slice(Math.max(0, at - 300), at)).toContain('class="session-context-menu"');
  });

  it("bubbles are yellow, the ring stays cyan", () => {
    const main = read("../styles/main.css");
    const block = (sel: string) => {
      const start = main.indexOf(sel);
      expect(start, sel).toBeGreaterThanOrEqual(0);
      return main.slice(start, main.indexOf("}", start));
    };
    const bubble = block(".tutorial-bubble {");
    expect(bubble).toContain("border: 1px solid var(--sidebar-warning, #e0a83a);");
    expect(bubble).toContain("0 0 12px var(--sidebar-warning, #e0a83a)");
    expect(bubble).not.toContain("--sidebar-accent");
    expect(block(".tutorial-bubble button:focus-visible"))
      .toContain("outline: 2px solid var(--sidebar-warning, #e0a83a);");
    expect(block(".tutorial-ring {")).toContain("--sidebar-accent");
    expect(block(".tutorial-bubble--warn")).toContain("border-style: dashed");
  });

  it("P12: orchestrator row id shape and the bubble action row wrap", () => {
    expect(PANEL).toContain(
      "`replica.row.${automationIdPart(rowContext)}.${automationIdPart(replica.path)}`");
    expect(PANEL).toContain('item.wg.taskTitle, "quick")');
    expect(orchRowId("/a/b")).toBe(`replica.row.quick.${automationIdPart("/a/b")}`);
    const css = read("../styles/main.css");
    const start = css.indexOf(".tutorial-bubble-actions {");
    expect(start).toBeGreaterThan(-1);
    expect(css.slice(start, css.indexOf("}", start))).toContain("flex-wrap: wrap;");
  });

  it("#5287 root-message targets and banner states", () => {
    const banner = read(`${SIDEBAR}RootAgentBanner.tsx`);
    expect(hasTestid(banner, RM_IDS.banner)).toBe(true);
    expect(hasTestid(read(OWNERS.actionBar!), RM_IDS.settings)).toBe(true);
    expect(banner).toContain("data-ac-state={bannerState()}");
    const start = banner.indexOf("const bannerState = () => {");
    expect(start).toBeGreaterThan(-1);
    const body = banner.slice(start, banner.indexOf("};", start));
    for (const s of Object.values(BANNER)) expect(body, s).toContain(`"${s}"`);
  });

  it("control: a wrong id is not found", () => {
    expect(hasTestid(read(OWNERS.actionBar!), "actionBar.newOpenX")).toBe(false);
    expect(hasTestid(read(OWNERS.newAgent!), "newAgent.template.listX")).toBe(false);
  });
});
