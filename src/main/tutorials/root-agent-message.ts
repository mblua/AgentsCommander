import { AGENTS, COORD, TEAM_NAME } from "./create-first-team";

// #5287 - "build the same team with one message to the Root Agent": texts and
// the pure view rule. The app never writes into a session; the person pastes.

export const RM_TITLE = "Build the same team with one message to the Root Agent";
export const RM_LINE = "Copy one message, send it to AgentsCommander's Root Agent, " +
  "and it creates the project, the three agents, the team and the room.";
export const RM_IDS = { banner: "rootAgent.banner", settings: "actionBar.settings" } as const;
export const BANNER = { missing: "missing", dormant: "dormant", live: "live" } as const;

const [PA, ACC, SME] = AGENTS.map((a) => a.name);
/** ONE line: a pasted newline may submit early in some coding-agent terminals. */
export const ROOT_MESSAGE = "Please set up my first team in AgentsCommander. " +
  "1) Create a new project with new-project; ask me which empty folder to use. " +
  "2) In that project, create three agents with create-agent-matrix: " +
  `${PA} (my personal assistant: it organizes my day, answers my messages and asks ` +
  `the other two for help); ${ACC}, from the Agency template "Bookkeeper & Controller"; ` +
  `${SME}, from the Agency template "Social Media Strategist". ` +
  "If the templates are not installed, offer to install them first. " +
  `3) Create the team ${TEAM_NAME} with team create: the three agents, with ${COORD} ` +
  `as coordinator. 4) Add a room for ${TEAM_NAME} in that project with room add. ` +
  "Tell me when it is done.";

export const T_NO_AGENT = "First configure a coding agent: click Settings, open Coding " +
  "Agents and click Add Agent. Then come back to this tutorial.";
export const T_MISSING = "Click this banner to create the Root Agent session. Then paste " +
  "the message in its session and press Enter.";
export const T_DORMANT = "Click this banner to wake the Root Agent session. Then paste the " +
  "message and press Enter.";
export const T_LIVE = "Click this banner to open the Root Agent session, paste the message " +
  "and press Enter. The Root Agent will ask you which folder to use.";
export const T_NO_BANNER = "Show the sidebar to see the Root Agent banner at its top.";
export const COPY = "Copy message";
export const COPIED = "Copied";
export const COPY_FAILED = "Could not copy. Select the message and copy it.";

export type RmView = { target: string | null; text: string; warn: boolean };

/** banner = data-ac-state of rootAgent.banner, or null when it is not in the DOM.
 *  agents = number of configured coding agents, or null while settings load. */
export function rootMessageView(banner: string | null, agents: number | null): RmView {
  const needsAgent = banner === BANNER.missing || banner === BANNER.dormant;
  if (needsAgent && agents === 0) {
    return { target: RM_IDS.settings, text: T_NO_AGENT, warn: true };
  }
  if (banner === null) return { target: null, text: T_NO_BANNER, warn: false };
  const text = banner === BANNER.missing ? T_MISSING
    : banner === BANNER.dormant ? T_DORMANT : T_LIVE;
  return { target: RM_IDS.banner, text, warn: false };
}
