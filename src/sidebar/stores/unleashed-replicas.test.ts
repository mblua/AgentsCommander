import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { __setTransportForTests } from "../../shared/ipc";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { unleashedReplicasStore } from "./unleashed-replicas";

// #4752 P05 - path-keyed overrides of the discovery `unleashed` flag.
const path = "C:\\Project\\.ac\\room-1-dev\\__agent_dev-rust";

describe("unleashedReplicasStore (#4752)", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    fake.resolve("set_replica_unleashed", undefined);
    restore = __setTransportForTests(fake);
    unleashedReplicasStore.resetForTests();
  });

  afterEach(() => {
    unleashedReplicasStore.resetForTests();
    restore();
  });

  it("falls back to the discovery flag without an override", () => {
    expect(unleashedReplicasStore.isUnleashed({ path })).toBe(false);
    expect(unleashedReplicasStore.isUnleashed({ path, unleashed: true })).toBe(true);
  });

  it("lets the override win over discovery after the backend accepts", async () => {
    await unleashedReplicasStore.set(path, true);
    expect(fake.callsFor("set_replica_unleashed").map((c) => c.args)).toEqual([
      { replicaPath: path, unleashed: true },
    ]);
    expect(unleashedReplicasStore.isUnleashed({ path, unleashed: false })).toBe(true);
    await unleashedReplicasStore.set(path, false);
    expect(unleashedReplicasStore.isUnleashed({ path, unleashed: true })).toBe(false);
  });

  it("maps Windows case and slash variants to one key", async () => {
    await unleashedReplicasStore.set(path, true);
    const variant = "c:/project/.ac/room-1-dev/__agent_dev-rust";
    expect(unleashedReplicasStore.isUnleashed({ path: variant })).toBe(true);
  });

  it("leaves the state unchanged and rethrows when set fails", async () => {
    fake.reject("set_replica_unleashed", "disk full");
    await expect(unleashedReplicasStore.set(path, true)).rejects.toBeTruthy();
    expect(unleashedReplicasStore.isUnleashed({ path })).toBe(false);
  });

  it("resetForTests clears every override", async () => {
    await unleashedReplicasStore.set(path, true);
    unleashedReplicasStore.resetForTests();
    expect(unleashedReplicasStore.isUnleashed({ path })).toBe(false);
  });
});
