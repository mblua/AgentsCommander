// @vitest-environment node
// #5228 - ties every target the tutorial reads to the source that renders it.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  IDS, NEW_OPEN_DISABLED, ROOM_HEADER_CLASS, TEAM_STEPS, projectHeaderId, projectMenuId,
  roomHeaderSelector,
} from "./create-first-team";

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

  it("control: a wrong id is not found", () => {
    expect(hasTestid(read(OWNERS.actionBar!), "actionBar.newOpenX")).toBe(false);
  });
});
