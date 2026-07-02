import { describe, expect, it } from "vitest";
import type { AcAgentReplica, AcDiscoveryResult, AcLoopSummary, AcWorkgroup } from "../../shared/types";
import type { ProjectState } from "./project";
import { deepEqual, mergeDiscoveryResult } from "./project-merge";

// #748 — the identity-preserving discovery merge. Object references drive the
// reference-keyed <For>s in ProjectPanel: every ref preserved here is a DOM row
// NOT disposed/re-created on reload (and a click over it NOT lost).

const PROJECT_PATH = "C:\\Users\\Maria\\Project";

function makeReplica(path: string, overrides: Partial<AcAgentReplica> = {}): AcAgentReplica {
  return {
    name: path.split(/[\\/]/).pop() ?? "agent",
    path,
    repoPaths: [],
    isCoordinator: true,
    ...overrides,
  };
}

function makeWorkgroup(
  name: string,
  agents: AcAgentReplica[],
  overrides: Partial<AcWorkgroup> = {}
): AcWorkgroup {
  return { name, path: `${PROJECT_PATH}\\.ac\\${name}`, task: null, agents, ...overrides };
}

function makeLoop(id: string, overrides: Partial<AcLoopSummary> = {}): AcLoopSummary {
  return {
    id,
    name: id,
    enabled: false,
    expr: "0 9 * * 1-5",
    timezone: "local",
    targetKind: "workgroupCoordinator",
    workgroup: "wg-1-team",
    promptPreview: "preview",
    busyCoordinator: "skip",
    path: `${PROJECT_PATH}\\.ac\\_loop_${id}`,
    configPath: `${PROJECT_PATH}\\.ac\\_loop_${id}\\config.toml`,
    lastCheckedAt: null,
    lastDueAt: null,
    lastDeliveredAt: null,
    lastResult: null,
    pendingDueAt: null,
    lastMissedClosedAt: null,
    nextDueAt: null,
    ...overrides,
  };
}

function makeResult(overrides: Partial<AcDiscoveryResult> = {}): AcDiscoveryResult {
  return {
    workgroups: [],
    agents: [],
    teams: [],
    loops: [],
    contextTemplateUpdates: [],
    ...overrides,
  };
}

function makeProject(result: AcDiscoveryResult): ProjectState {
  return {
    path: PROJECT_PATH,
    folderName: "Project",
    workgroups: result.workgroups,
    agents: result.agents,
    teams: result.teams,
    loops: result.loops,
    contextTemplateUpdates: result.contextTemplateUpdates,
  };
}

const COORD_A = `${PROJECT_PATH}\\.ac\\wg-1-team\\__agent_coord-a`;
const COORD_B = `${PROJECT_PATH}\\.ac\\wg-1-team\\__agent_coord-b`;
const COORD_C = `${PROJECT_PATH}\\.ac\\wg-2-team\\__agent_coord-c`;

/** Two workgroups + a loop, built FRESH each call so every object reference
 *  differs between snapshots even when the data is identical. */
function snapshot(): AcDiscoveryResult {
  return makeResult({
    workgroups: [
      makeWorkgroup("wg-1-team", [makeReplica(COORD_A), makeReplica(COORD_B)]),
      makeWorkgroup("wg-2-team", [makeReplica(COORD_C)]),
    ],
    teams: [{ name: "dev-team", agents: ["coord-a"], coordinator: "coord-a" }],
    loops: [makeLoop("standup")],
  });
}

describe("mergeDiscoveryResult (#748)", () => {
  it("returns the existing project object itself for an identical snapshot", () => {
    const existing = makeProject(snapshot());

    const merged = mergeDiscoveryResult(existing, snapshot());

    expect(merged).toBe(existing);
  });

  it("re-creates only the entities whose data changed, preserving every sibling reference", () => {
    const existing = makeProject(snapshot());
    const next = snapshot();
    next.workgroups[0].agents[0] = makeReplica(COORD_A, { currentProfile: "B" });

    const merged = mergeDiscoveryResult(existing, next);

    expect(merged).not.toBe(existing);
    // Changed chain: wg-1 and coord-a are new objects.
    expect(merged.workgroups[0]).not.toBe(existing.workgroups[0]);
    expect(merged.workgroups[0].agents[0]).not.toBe(existing.workgroups[0].agents[0]);
    expect(merged.workgroups[0].agents[0].currentProfile).toBe("B");
    // Untouched siblings keep identity: coord-b inside the changed wg, the
    // whole wg-2, and the teams/loops arrays.
    expect(merged.workgroups[0].agents[1]).toBe(existing.workgroups[0].agents[1]);
    expect(merged.workgroups[1]).toBe(existing.workgroups[1]);
    expect(merged.teams).toBe(existing.teams);
    expect(merged.loops).toBe(existing.loops);
    expect(merged.contextTemplateUpdates).toBe(existing.contextTemplateUpdates);
  });

  it("a loop-only change leaves the workgroup tree untouched", () => {
    const existing = makeProject(snapshot());
    const next = snapshot();
    next.loops = [makeLoop("standup", { enabled: true })];

    const merged = mergeDiscoveryResult(existing, next);

    expect(merged).not.toBe(existing);
    expect(merged.workgroups).toBe(existing.workgroups);
    expect(merged.loops).not.toBe(existing.loops);
    expect(merged.loops[0].enabled).toBe(true);
  });

  it("matches entities by NORMALIZED path, so a path-shape drift updates in place", () => {
    const existing = makeProject(snapshot());
    const next = snapshot();
    next.workgroups[1] = makeWorkgroup("wg-2-team", [
      makeReplica("c:/users/maria/project/.ac/wg-2-team/__agent_coord-c"),
    ]);
    next.workgroups[1].path = "c:/users/maria/project/.ac/wg-2-team";

    const merged = mergeDiscoveryResult(existing, next);

    // The drifted entity is recognized as the SAME entity (matched by
    // normalized key) and adopts the new path — a textual path change IS a
    // data change, so its object is new — while the untouched sibling
    // workgroup keeps identity.
    expect(merged.workgroups).toHaveLength(2);
    expect(merged.workgroups[1]).not.toBe(existing.workgroups[1]);
    expect(merged.workgroups[1].path).toBe("c:/users/maria/project/.ac/wg-2-team");
    expect(merged.workgroups[0]).toBe(existing.workgroups[0]);
  });

  it("added and removed entities change the array; survivors keep identity", () => {
    const existing = makeProject(snapshot());
    const next = snapshot();
    next.workgroups = [next.workgroups[0]]; // wg-2 removed

    const merged = mergeDiscoveryResult(existing, next);

    expect(merged.workgroups).toHaveLength(1);
    expect(merged.workgroups[0]).toBe(existing.workgroups[0]);
  });

  it("a reorder with identical entities still preserves item identity", () => {
    const existing = makeProject(snapshot());
    const next = snapshot();
    next.workgroups = [next.workgroups[1], next.workgroups[0]];

    const merged = mergeDiscoveryResult(existing, next);

    expect(merged.workgroups[0]).toBe(existing.workgroups[1]);
    expect(merged.workgroups[1]).toBe(existing.workgroups[0]);
  });
});

describe("deepEqual (#748)", () => {
  it("treats an undefined-valued key and a missing key as equal (optional fields)", () => {
    expect(deepEqual({ a: 1, b: undefined }, { a: 1 })).toBe(true);
    expect(deepEqual({ a: 1 }, { a: 1, b: undefined })).toBe(true);
    expect(deepEqual({ a: 1 }, { a: 1, b: null })).toBe(false);
  });

  it("compares nested arrays/objects structurally", () => {
    expect(deepEqual({ a: [{ b: 1 }] }, { a: [{ b: 1 }] })).toBe(true);
    expect(deepEqual({ a: [{ b: 1 }] }, { a: [{ b: 2 }] })).toBe(false);
    expect(deepEqual([1, 2], [1, 2, 3])).toBe(false);
    expect(deepEqual({ a: [1] }, { a: { 0: 1 } })).toBe(false);
    expect(deepEqual(null, {})).toBe(false);
  });
});
