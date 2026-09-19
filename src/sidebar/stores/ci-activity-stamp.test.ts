// @vitest-environment jsdom
//
// #2202 - the CI-stop activity stamp. The watcher is a falling-edge detector over
// `workgroupCiRunning`, the same predicate the rail counter, dot, tooltip, row
// tint and watchdog read, so the stamp cannot disagree with what the user sees.
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { createRoot } from "solid-js";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { __setTransportForTests } from "../../shared/ipc";
import { baseSettings, discovery, resetUiStoresForTests, session, waitFor } from "../../shared/testing/ui-harness";
import type { CiState } from "../../shared/types";
import { projectStore } from "./project";
import { sessionsStore } from "./sessions";
import { settingsStore } from "../../shared/stores/settings";
import { remoteActivityStore } from "./remote-activity";
import { startCiActivityStamp } from "./ci-activity-stamp";

const projectPath = "C:\\Project";
const ROOM = "wg-1-dev-team";
const wgPath = `${projectPath}\\.ac\\${ROOM}`;
const coordPath = `${wgPath}\\__agent_orchestrator`;
const memberPath = `${wgPath}\\__agent_member`;
const coordRepo = `${wgPath}\\repo-Coord`;
const memberRepo = `${wgPath}\\repo-Member`;
const COORD_SESSION = "session-orchestrator";

function roomDiscovery() {
  return discovery({
    workgroups: [
      {
        name: ROOM,
        path: wgPath,
        task: null,
        taskTitle: null,
        agents: [
          { name: "orchestrator", path: coordPath, repoPaths: [coordRepo], isCoordinator: true },
          { name: "member", path: memberPath, repoPaths: [memberRepo], isCoordinator: false },
        ],
      },
    ],
  });
}

/** The store replaces the WHOLE map per update, so all repos go in ONE call. */
function publishCi(entries: [string, CiState][]): void {
  remoteActivityStore.applyRemoteActivityUpdate({
    repoPaths: entries.map(([repoPath]) => repoPath),
    ciStates: entries.map(([, ci]) => ci),
    stalenessStates: entries.map(() => "current" as const),
    behindBy: entries.map(() => null),
  });
}

async function seed() {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings());
  fake.resolve("discover_project", roomDiscovery());
  const restore = __setTransportForTests(fake);
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  sessionsStore.setSessions([
    session({ id: COORD_SESSION, name: `${ROOM}/orchestrator`, workingDirectory: coordPath, status: "idle" }),
    session({ id: "session-member", name: `${ROOM}/member`, workingDirectory: memberPath, status: "idle" }),
  ]);
  return restore;
}

const stampOf = (sessionId: string): number | undefined =>
  sessionsStore.lastActivityBySessionId[sessionId];

describe("#2202 CI-stop activity stamp", () => {
  let restore: (() => void) | null = null;
  let dispose: (() => void) | null = null;

  beforeEach(() => resetUiStoresForTests());

  afterEach(() => {
    dispose?.();
    dispose = null;
    restore?.();
    restore = null;
    resetUiStoresForTests();
  });

  async function start(): Promise<void> {
    createRoot((d) => {
      dispose = d;
      startCiActivityStamp();
    });
    await Promise.resolve();
  }

  it("seeds the first observation without stamping, even with CI already stopped", async () => {
    restore = await seed();
    publishCi([[coordRepo, "idle"]]);
    await start();
    expect(stampOf(COORD_SESSION)).toBeUndefined();

    // Positive control in the same run: the effect IS live, so the negative above
    // cannot pass because nothing is watching.
    publishCi([[coordRepo, "running"]]);
    await waitFor(() => expect(stampOf(COORD_SESSION)).toBeUndefined());
    publishCi([[coordRepo, "idle"]]);
    await waitFor(() => expect(stampOf(COORD_SESSION)).toBeTypeOf("number"));
  });

  it("stamps the coordinator session on running -> idle (CI finished), once per edge", async () => {
    restore = await seed();
    publishCi([[coordRepo, "running"]]);
    await start();
    expect(stampOf(COORD_SESSION)).toBeUndefined();

    publishCi([[coordRepo, "idle"]]);
    await waitFor(() => expect(stampOf(COORD_SESSION)).toBeTypeOf("number"));
    const first = stampOf(COORD_SESSION);

    // A second identical update is not a new edge, so the stamp does not move.
    publishCi([[coordRepo, "idle"]]);
    await Promise.resolve();
    expect(stampOf(COORD_SESSION)).toBe(first);
    // The non-coordinator session is never stamped.
    expect(stampOf("session-member")).toBeUndefined();
  });

  it("stamps when the repo key is garbage-collected out of the map", async () => {
    restore = await seed();
    publishCi([[coordRepo, "running"]]);
    await start();

    publishCi([]); // the path departed; it reads as not running, exactly as the tint does
    await waitFor(() => expect(stampOf(COORD_SESSION)).toBeTypeOf("number"));
  });

  it("never stamps for a non-coordinator's CI transition", async () => {
    restore = await seed();
    publishCi([[memberRepo, "running"]]);
    await start();

    publishCi([[memberRepo, "idle"]]);
    await Promise.resolve();
    expect(stampOf(COORD_SESSION)).toBeUndefined();
    expect(stampOf("session-member")).toBeUndefined();
  });
});
