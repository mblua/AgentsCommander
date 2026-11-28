// @vitest-environment node
// #5287 - pure rules and the exact message.
import { describe, expect, it } from "vitest";
import { AGENTS, TEAM_NAME } from "./create-first-team";
import {
  RM_IDS, ROOT_MESSAGE, T_DORMANT, T_LIVE, T_MISSING, T_NO_AGENT, T_NO_BANNER, rootMessageView,
} from "./root-agent-message";

const EXACT = "Please set up my first team in AgentsCommander. 1) Create a new project with " +
  "new-project; ask me which empty folder to use. 2) In that project, create three agents " +
  "with create-agent-matrix: personal-assistant (my personal assistant: it organizes my " +
  "day, answers my messages and asks the other two for help); accountant, from the Agency " +
  "template \"Bookkeeper & Controller\"; social-media-expert, from the Agency template " +
  "\"Social Media Strategist\". If the templates are not installed, offer to install them " +
  "first. 3) Create the team my-team with team create: the three agents, with " +
  "personal-assistant as coordinator. 4) Add a room for my-team in that project with " +
  "room add. Tell me when it is done.";

describe("rootMessageView", () => {
  const cases: Array<[string | null, number | null, string | null, string, boolean]> = [
    ["missing", 0, RM_IDS.settings, T_NO_AGENT, true],
    ["dormant", 0, RM_IDS.settings, T_NO_AGENT, true],
    ["live", 0, RM_IDS.banner, T_LIVE, false],
    [null, 0, null, T_NO_BANNER, false],
    [null, 2, null, T_NO_BANNER, false],
    ["missing", 1, RM_IDS.banner, T_MISSING, false],
    ["missing", null, RM_IDS.banner, T_MISSING, false],
    ["dormant", 1, RM_IDS.banner, T_DORMANT, false],
    ["dormant", null, RM_IDS.banner, T_DORMANT, false],
    ["live", 1, RM_IDS.banner, T_LIVE, false],
    ["other", 1, RM_IDS.banner, T_LIVE, false],
  ];
  for (const [banner, agents, target, text, warn] of cases) {
    it(`${banner} / ${agents}`, () => {
      expect(rootMessageView(banner, agents)).toEqual({ target, text, warn });
    });
  }
});

describe("ROOT_MESSAGE", () => {
  it("is the exact one-line text", () => {
    expect(ROOT_MESSAGE).toBe(EXACT);
    expect(ROOT_MESSAGE).not.toContain("\n");
    expect(ROOT_MESSAGE).not.toContain("\r");
  });
  it("names every agent, both templates and the team", () => {
    for (const a of AGENTS) expect(ROOT_MESSAGE).toContain(a.name);
    expect(ROOT_MESSAGE).toContain("\"Bookkeeper & Controller\"");
    expect(ROOT_MESSAGE).toContain("\"Social Media Strategist\"");
    expect(ROOT_MESSAGE).toContain(TEAM_NAME);
  });
});
