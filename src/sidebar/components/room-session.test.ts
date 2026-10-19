import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcAgentReplica, AcWorkgroup, SessionStatus } from "../../shared/types";
import { resetUiStoresForTests, session } from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import { remoteActivityStore } from "../stores/remote-activity";
import type { CiState } from "../../shared/types";
import {
  isReplicaWorking,
  replicaRepoBadgesLive,
  splitRoomsByActive,
  splitRoomsByWorking,
  roomCiRunning,
  roomIsActive,
  roomIsWorking,
} from "./room-session";

const projectPath = "C:\\Project";

function replicaPath(roomName: string): string {
  return `${projectPath}\\.ac\\${roomName}\\__agent_dev-webpage-ui`;
}

function replica(roomName: string): AcAgentReplica {
  return {
    name: "dev-webpage-ui",
    path: replicaPath(roomName),
    repoPaths: [],
    isCoordinator: true,
  };
}

// #2202 - the CI half. `repoPaths` carries the EXACT string the remote-activity
// payload is keyed by, because the map applies no normalization.
const ciRepoPath = (roomName: string): string => `${projectPath}\\.ac\\${roomName}\\repo-AgentsCommander`;

function ciReplica(roomName: string, overrides: Partial<AcAgentReplica> = {}): AcAgentReplica {
  return { ...replica(roomName), repoPaths: [ciRepoPath(roomName)], ...overrides };
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

function room(name: string, agents: AcAgentReplica[] = [replica(name)]): AcWorkgroup {
  return {
    name,
    path: `${projectPath}\\.ac\\${name}`,
    task: null,
    taskTitle: null,
    agents,
  };
}

function replicaSession(
  roomName: string,
  status: SessionStatus = "running",
  flags: { pendingReview?: boolean; waitingForInput?: boolean } = {}
) {
  return session({
    id: `s-${roomName}`,
    name: `${roomName}/dev-webpage-ui`,
    workingDirectory: replicaPath(roomName),
    status,
    pendingReview: flags.pendingReview ?? false,
    waitingForInput: flags.waitingForInput ?? false,
  });
}

describe("room-session working predicates", () => {
  beforeEach(() => resetUiStoresForTests());
  afterEach(() => resetUiStoresForTests());

  it("classifies replicas from session state, not dot classes", () => {
    const group = room("wg-1-dev-team");
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
    const group = room("wg-1-dev-team");
    // At-rest writers pair pendingReview with waitingForInput, but #882
    // deliberately preserves the existing constructed-state behavior too.
    sessionsStore.setSessions([replicaSession(group.name, "running", { pendingReview: true })]);

    expect(isReplicaWorking(group, group.agents[0])).toBe(false);
    expect(roomIsWorking(group)).toBe(false);
  });

  it("classifies workgroups by whether any replica is working", () => {
    const working = replica("wg-1-dev-team");
    const waiting = { ...replica("wg-1-dev-team"), name: "dev-rust" };
    const group = room("wg-1-dev-team", [working, waiting]);

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
    expect(roomIsWorking(group)).toBe(true);

    sessionsStore.setSessions([replicaSession(group.name, "idle")]);
    expect(roomIsWorking(group)).toBe(false);
    expect(roomIsWorking(room("wg-empty", []))).toBe(false);
  });

  it("partitions workgroups completely and preserves order in both buckets", () => {
    const groups = [
      room("wg-1-dev-team"),
      room("wg-2-rust-team"),
      room("wg-3-docs-team"),
      room("wg-4-qa-team"),
    ];
    sessionsStore.setSessions([
      replicaSession("wg-1-dev-team"),
      replicaSession("wg-3-docs-team", "active"),
    ]);

    const split = splitRoomsByWorking(groups);

    expect(split.working.length + split.notWorking.length).toBe(groups.length);
    expect(new Set([...split.working, ...split.notWorking]).size).toBe(groups.length);
    expect(split.working.map((group) => group.name)).toEqual(["wg-1-dev-team", "wg-3-docs-team"]);
    expect(split.notWorking.map((group) => group.name)).toEqual(["wg-2-rust-team", "wg-4-qa-team"]);
  });

  it("#2202 counts a coordinator's running CI as the room's CI, and a non-coordinator's not", () => {
    const coordRoom = room("wg-1-dev-team", [ciReplica("wg-1-dev-team")]);
    const memberRoom = room("wg-2-rust-team", [
      ciReplica("wg-2-rust-team", { isCoordinator: false }),
    ]);
    publishCi([
      [ciRepoPath("wg-1-dev-team"), "running"],
      [ciRepoPath("wg-2-rust-team"), "running"],
    ]);

    expect(roomCiRunning(coordRoom)).toBe(true);
    expect(roomCiRunning(memberRoom)).toBe(false);

    publishCi([[ciRepoPath("wg-1-dev-team"), "idle"]]);
    expect(roomCiRunning(coordRoom)).toBe(false);
  });

  it("#2202 reads session gitRepos first and the configured badges only as a fallback", () => {
    const group = room("wg-1-dev-team", [ciReplica("wg-1-dev-team")]);
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
    expect(roomCiRunning(group)).toBe(false);
    publishCi([[sessionRepoPath, "running"]]);
    expect(roomCiRunning(group)).toBe(true);
  });

  it("#2202 treats working-only, CI-only and both as active, and keeps working session-only", () => {
    const workingRoom = room("wg-1-dev-team", [ciReplica("wg-1-dev-team")]);
    const ciRoom = room("wg-2-rust-team", [ciReplica("wg-2-rust-team")]);
    const bothRoom = room("wg-3-docs-team", [ciReplica("wg-3-docs-team")]);
    const idleRoom = room("wg-4-qa-team", [ciReplica("wg-4-qa-team")]);

    sessionsStore.setSessions([
      replicaSession("wg-1-dev-team"),
      replicaSession("wg-3-docs-team"),
      replicaSession("wg-4-qa-team", "idle"),
    ]);
    publishCi([
      [ciRepoPath("wg-2-rust-team"), "running"],
      [ciRepoPath("wg-3-docs-team"), "running"],
    ]);

    expect(roomIsActive(projectPath, workingRoom)).toBe(true);
    expect(roomIsActive(projectPath, ciRoom)).toBe(true);
    expect(roomIsActive(projectPath, bothRoom)).toBe(true);
    expect(roomIsActive(projectPath, idleRoom)).toBe(false);

    const rooms = [workingRoom, ciRoom, bothRoom, idleRoom];
    const split = splitRoomsByActive(projectPath, rooms);
    expect(split.active.length + split.notActive.length).toBe(rooms.length);
    expect(split.active.map((group) => group.name)).toEqual([
      "wg-1-dev-team",
      "wg-2-rust-team",
      "wg-3-docs-team",
    ]);
    expect(split.notActive.map((group) => group.name)).toEqual(["wg-4-qa-team"]);

    // The internal primitive stays session-only: the CI-only room is NOT working.
    expect(roomIsWorking(ciRoom)).toBe(false);
    const workingSplit = splitRoomsByWorking(rooms);
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
