import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcAgentReplica, AcWorkgroup, SessionStatus } from "../../shared/types";
import { resetUiStoresForTests, session } from "../../shared/testing/ui-harness";
import { sessionsStore } from "../stores/sessions";
import {
  isReplicaWorking,
  splitWorkgroupsByWorking,
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
});
