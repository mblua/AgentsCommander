import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcAgentReplica, AcWorkgroup, SessionStatus } from "../../shared/types";
import { resetUiStoresForTests, session } from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import { remoteActivityStore } from "../stores/remote-activity";
import type { CiState } from "../../shared/types";
import {
  isReplicaWorking,
  replicaRepoBadgesLive,
  splitWorkgroupsByActive,
  splitWorkgroupsByWorking,
  workgroupCiRunning,
  workgroupIsActive,
  workgroupIsWorking,
} from "./workgroup-session";

const projectPath = "C:\\Project";

function replicaPath(wgName: string): string {
  return `${projectPath}\\.ac\\${wgName}\\__agent_dev-webpage-ui`;
}

function replica(wgName: string): AcAgentReplica {
  return {
    name: "dev-webpage-ui",
    path: replicaPath(wgName),
    repoPaths: [],
    isCoordinator: true,
  };
}

// #2202 - the CI half. `repoPaths` carries the EXACT string the remote-activity
// payload is keyed by, because the map applies no normalization.
const ciRepoPath = (wgName: string): string => `${projectPath}\\.ac\\${wgName}\\repo-AgentsCommander`;

function ciReplica(wgName: string, overrides: Partial<AcAgentReplica> = {}): AcAgentReplica {
  return { ...replica(wgName), repoPaths: [ciRepoPath(wgName)], ...overrides };
}

/** The store replaces the WHOLE map per update, so every repo of interest is
 *  published in ONE call. */
function publishCi(entries: [string, CiState][]): void {
  remoteActivityStore.applyRemoteActivityUpdate({
    repoPaths: entries.map(([repoPath]) => repoPath),
    ciStates: entries.map(([, ci]) => ci),
    stalenessStates: entries.map(() => "current" as const),
    behindBy: entries.map(() => null),
  });
}

function wg(name: string, agents: AcAgentReplica[] = [replica(name)]): AcWorkgroup {
  return {
    name,
    path: `${projectPath}\\.ac\\${name}`,
    task: null,
    taskTitle: null,
    agents,
  };
}

function replicaSession(
  wgName: string,
  status: SessionStatus = "running",
  flags: { pendingReview?: boolean; waitingForInput?: boolean } = {}
) {
  return session({
    id: `s-${wgName}`,
    name: `${wgName}/dev-webpage-ui`,
    workingDirectory: replicaPath(wgName),
    status,
    pendingReview: flags.pendingReview ?? false,
    waitingForInput: flags.waitingForInput ?? false,
  });
}

describe("workgroup-session working predicates", () => {
  beforeEach(() => resetUiStoresForTests());
  afterEach(() => resetUiStoresForTests());

  it("classifies replicas from session state, not dot classes", () => {
    const group = wg("wg-1-dev-team");
    const rep = group.agents[0];

    sessionsStore.setSessions([replicaSession(group.name)]);
    expect(isReplicaWorking(group, rep)).toBe(true);

    sessionsStore.setSessions([replicaSession(group.name, "running", { waitingForInput: true })]);
    expect(isReplicaWorking(group, rep)).toBe(false);

    sessionsStore.setSessions([replicaSession(group.name, "running", { pendingReview: true })]);
    expect(isReplicaWorking(group, rep)).toBe(false);

    sessionsStore.setSessions([replicaSession(group.name, { exited: 0 })]);
    expect(isReplicaWorking(group, rep)).toBe(false);

    sessionsStore.setSessions([]);
    expect(isReplicaWorking(group, rep)).toBe(false);
  });

  it("pins running plus pendingReview as not working", () => {
    const group = wg("wg-1-dev-team");
    // At-rest writers pair pendingReview with waitingForInput, but #882
    // deliberately preserves the existing constructed-state behavior too.
    sessionsStore.setSessions([replicaSession(group.name, "running", { pendingReview: true })]);

    expect(isReplicaWorking(group, group.agents[0])).toBe(false);
    expect(workgroupIsWorking(group)).toBe(false);
  });

  it("classifies workgroups by whether any replica is working", () => {
    const working = replica("wg-1-dev-team");
    const waiting = { ...replica("wg-1-dev-team"), name: "dev-rust" };
    const group = wg("wg-1-dev-team", [working, waiting]);

    sessionsStore.setSessions([
      replicaSession(group.name),
      session({
        id: "s-waiting",
        name: `${group.name}/dev-rust`,
        workingDirectory: waiting.path,
        status: "running",
        waitingForInput: true,
      }),
    ]);
    expect(workgroupIsWorking(group)).toBe(true);

    sessionsStore.setSessions([replicaSession(group.name, "idle")]);
    expect(workgroupIsWorking(group)).toBe(false);
    expect(workgroupIsWorking(wg("wg-empty", []))).toBe(false);
  });

  it("partitions workgroups completely and preserves order in both buckets", () => {
    const groups = [
      wg("wg-1-dev-team"),
      wg("wg-2-rust-team"),
      wg("wg-3-docs-team"),
      wg("wg-4-qa-team"),
    ];
    sessionsStore.setSessions([
      replicaSession("wg-1-dev-team"),
      replicaSession("wg-3-docs-team", "active"),
    ]);

    const split = splitWorkgroupsByWorking(groups);

    expect(split.working.length + split.notWorking.length).toBe(groups.length);
    expect(new Set([...split.working, ...split.notWorking]).size).toBe(groups.length);
    expect(split.working.map((group) => group.name)).toEqual(["wg-1-dev-team", "wg-3-docs-team"]);
    expect(split.notWorking.map((group) => group.name)).toEqual(["wg-2-rust-team", "wg-4-qa-team"]);
  });

  it("#2202 counts a coordinator's running CI as the room's CI, and a non-coordinator's not", () => {
    const coordRoom = wg("wg-1-dev-team", [ciReplica("wg-1-dev-team")]);
    const memberRoom = wg("wg-2-rust-team", [
      ciReplica("wg-2-rust-team", { isCoordinator: false }),
    ]);
    publishCi([
      [ciRepoPath("wg-1-dev-team"), "running"],
      [ciRepoPath("wg-2-rust-team"), "running"],
    ]);

    expect(workgroupCiRunning(coordRoom)).toBe(true);
    expect(workgroupCiRunning(memberRoom)).toBe(false);

    publishCi([[ciRepoPath("wg-1-dev-team"), "idle"]]);
    expect(workgroupCiRunning(coordRoom)).toBe(false);
  });

  it("#2202 reads session gitRepos first and the configured badges only as a fallback", () => {
    const group = wg("wg-1-dev-team", [ciReplica("wg-1-dev-team")]);
    const rep = group.agents[0];
    const sessionRepoPath = `${projectPath}\\.ac\\wg-1-dev-team\\repo-Live`;

    expect(replicaRepoBadgesLive(group, rep).map((repo) => repo.sourcePath)).toEqual([
      ciRepoPath("wg-1-dev-team"),
    ]);

    sessionsStore.setSessions([
      session({
        id: "s-wg-1-dev-team",
        name: "wg-1-dev-team/dev-webpage-ui",
        workingDirectory: replicaPath("wg-1-dev-team"),
        status: "idle",
        gitRepos: [{ label: "Live", sourcePath: sessionRepoPath, branch: null, dirty: null }],
      }),
    ]);
    expect(replicaRepoBadgesLive(group, rep).map((repo) => repo.sourcePath)).toEqual([
      sessionRepoPath,
    ]);

    // The CI answer follows the live list, not the configured one.
    publishCi([[ciRepoPath("wg-1-dev-team"), "running"]]);
    expect(workgroupCiRunning(group)).toBe(false);
    publishCi([[sessionRepoPath, "running"]]);
    expect(workgroupCiRunning(group)).toBe(true);
  });

  it("#2202 treats working-only, CI-only and both as active, and keeps working session-only", () => {
    const workingRoom = wg("wg-1-dev-team", [ciReplica("wg-1-dev-team")]);
    const ciRoom = wg("wg-2-rust-team", [ciReplica("wg-2-rust-team")]);
    const bothRoom = wg("wg-3-docs-team", [ciReplica("wg-3-docs-team")]);
    const idleRoom = wg("wg-4-qa-team", [ciReplica("wg-4-qa-team")]);

    sessionsStore.setSessions([
      replicaSession("wg-1-dev-team"),
      replicaSession("wg-3-docs-team"),
      replicaSession("wg-4-qa-team", "idle"),
    ]);
    publishCi([
      [ciRepoPath("wg-2-rust-team"), "running"],
      [ciRepoPath("wg-3-docs-team"), "running"],
    ]);

    expect(workgroupIsActive(workingRoom)).toBe(true);
    expect(workgroupIsActive(ciRoom)).toBe(true);
    expect(workgroupIsActive(bothRoom)).toBe(true);
    expect(workgroupIsActive(idleRoom)).toBe(false);

    const rooms = [workingRoom, ciRoom, bothRoom, idleRoom];
    const split = splitWorkgroupsByActive(rooms);
    expect(split.active.length + split.notActive.length).toBe(rooms.length);
    expect(split.active.map((group) => group.name)).toEqual([
      "wg-1-dev-team",
      "wg-2-rust-team",
      "wg-3-docs-team",
    ]);
    expect(split.notActive.map((group) => group.name)).toEqual(["wg-4-qa-team"]);

    // The internal primitive stays session-only: the CI-only room is NOT working.
    expect(workgroupIsWorking(ciRoom)).toBe(false);
    const workingSplit = splitWorkgroupsByWorking(rooms);
    expect(workingSplit.working.map((group) => group.name)).toEqual([
      "wg-1-dev-team",
      "wg-3-docs-team",
    ]);
    expect(workingSplit.notWorking.map((group) => group.name)).toEqual([
      "wg-2-rust-team",
      "wg-4-qa-team",
    ]);
  });
});
