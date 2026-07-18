import { describe, expect, it } from "vitest";
import { decodeSessionSelection } from "./session-selection";
import type { SessionSelection } from "./types";
import { SESSION_A, TEST_EPOCH } from "./testing/session-selection";

interface CauseCase {
  source: SessionSelection["source"];
  userInitiated: boolean;
  mode: SessionSelection["mode"];
}

const allowedCauses: CauseCase[] = [
  { source: "initialHydration", userInitiated: false, mode: "none" },
  { source: "sessionCreated", userInitiated: false, mode: "live" },
  { source: "sessionCreated", userInitiated: true, mode: "live" },
  { source: "userSwitch", userInitiated: true, mode: "live" },
  { source: "userSwitch", userInitiated: true, mode: "dormant" },
  { source: "manualClose", userInitiated: true, mode: "live" },
  { source: "manualClose", userInitiated: true, mode: "none" },
  { source: "autoClose", userInitiated: false, mode: "none" },
  { source: "restart", userInitiated: false, mode: "live" },
  { source: "restart", userInitiated: true, mode: "live" },
  { source: "restart", userInitiated: false, mode: "none" },
  { source: "restart", userInitiated: true, mode: "none" },
  { source: "restore", userInitiated: false, mode: "live" },
  { source: "restore", userInitiated: false, mode: "dormant" },
  { source: "restore", userInitiated: false, mode: "none" },
  { source: "detach", userInitiated: true, mode: "live" },
  { source: "detach", userInitiated: true, mode: "none" },
  { source: "attach", userInitiated: true, mode: "live" },
  { source: "attach", userInitiated: true, mode: "dormant" },
  { source: "spawnRollback", userInitiated: false, mode: "none" },
  { source: "resourceMonitor", userInitiated: false, mode: "none" },
  { source: "resourceMonitor", userInitiated: true, mode: "none" },
  { source: "backgroundCleanup", userInitiated: false, mode: "none" },
  { source: "livenessReconcile", userInitiated: false, mode: "dormant" },
  { source: "livenessReconcile", userInitiated: false, mode: "none" },
];

function rawPayload(cause: CauseCase): Record<string, unknown> {
  const order = {
    epoch: TEST_EPOCH,
    source: cause.source,
    userInitiated: cause.userInitiated,
    revision: cause.source === "initialHydration" ? 0 : 1,
    mode: cause.mode,
  };
  if (cause.mode === "live") {
    return {
      ...order,
      id: SESSION_A,
      status: "active",
      hasPty: true,
      detached: false,
      displayable: true,
    };
  }
  if (cause.mode === "dormant") {
    return {
      ...order,
      id: SESSION_A,
      status: { exited: -7 },
      hasPty: false,
      detached: false,
      displayable: false,
    };
  }
  return {
    ...order,
    id: null,
    status: null,
    hasPty: false,
    detached: false,
    displayable: false,
  };
}

describe("decodeSessionSelection", () => {
  it.each(allowedCauses)(
    "decodes $source/$mode/$userInitiated into a fresh normalized value",
    (cause) => {
      const raw = rawPayload(cause);
      const decoded = decodeSessionSelection(raw);
      expect(decoded).toEqual(raw);
      expect(decoded).not.toBe(raw);
      if (decoded.mode === "dormant") {
        expect(decoded.status).not.toBe(raw.status);
      }
    },
  );

  it("rejects missing, extra, inherited, accessor, and non-plain selection data", () => {
    const valid = rawPayload({ source: "restore", userInitiated: false, mode: "live" });
    const { displayable: _omitted, ...missing } = valid;
    expect(() => decodeSessionSelection(missing)).toThrow(/exact keys/);
    expect(() => decodeSessionSelection({ ...valid, extra: true })).toThrow(/exact keys/);
    const hiddenExtra = { ...valid };
    Object.defineProperty(hiddenExtra, "hidden", { value: true });
    expect(() => decodeSessionSelection(hiddenExtra)).toThrow(/exact keys/);
    expect(() => decodeSessionSelection({ ...valid, [Symbol("extra")]: true })).toThrow(/exact keys/);
    expect(() => decodeSessionSelection(Object.create(valid))).toThrow(/plain object prototype/);
    expect(() => decodeSessionSelection(new Date())).toThrow(/plain object prototype/);

    const accessor = { ...valid };
    Object.defineProperty(accessor, "id", { enumerable: true, get: () => SESSION_A });
    expect(() => decodeSessionSelection(accessor)).toThrow(/accessor-backed/);
  });

  it.each([
    ["negative revision", { revision: -1 }],
    ["unsafe revision", { revision: Number.MAX_SAFE_INTEGER + 1 }],
    ["fractional revision", { revision: 1.5 }],
    ["noncanonical epoch", { epoch: "AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA" }],
    ["non-UUID id", { id: "session-1" }],
    ["detached live", { detached: true }],
    ["hidden live", { displayable: false }],
    ["PTY-less live", { hasPty: false }],
    ["wrong live status", { status: "running" }],
  ])("rejects %s", (_name, mutation) => {
    const valid = rawPayload({ source: "restore", userInitiated: false, mode: "live" });
    expect(() => decodeSessionSelection({ ...valid, ...mutation })).toThrow();
  });

  it.each([
    { status: { exited: 2_147_483_648 } },
    { status: { exited: -2_147_483_649 } },
    { status: { exited: 1.25 } },
    { status: { exited: 1, extra: true } },
    { status: Object.create({ exited: 1 }) },
  ])("rejects invalid dormant status %#", (mutation) => {
    const valid = rawPayload({ source: "restore", userInitiated: false, mode: "dormant" });
    expect(() => decodeSessionSelection({ ...valid, ...mutation })).toThrow();
  });

  it("accepts both actual dormant PTY snapshots but rejects every other dormant invariant", () => {
    const valid = rawPayload({ source: "restore", userInitiated: false, mode: "dormant" });
    expect(decodeSessionSelection({ ...valid, hasPty: true })).toMatchObject({
      mode: "dormant",
      hasPty: true,
    });
    for (const mutation of [
      { hasPty: 0 },
      { detached: true },
      { displayable: true },
      { id: null },
      { status: "active" },
    ]) {
      expect(() => decodeSessionSelection({ ...valid, ...mutation })).toThrow();
    }
    const accessorStatus = { exited: 1 };
    Object.defineProperty(accessorStatus, "exited", {
      enumerable: true,
      get: () => 1,
    });
    expect(() => decodeSessionSelection({ ...valid, status: accessorStatus })).toThrow(/accessor-backed/);
  });

  it("rejects every mutation of the none-mode invariant literals", () => {
    const valid = rawPayload({ source: "autoClose", userInitiated: false, mode: "none" });
    for (const mutation of [
      { id: SESSION_A },
      { status: "active" },
      { hasPty: true },
      { detached: true },
      { displayable: true },
    ]) {
      expect(() => decodeSessionSelection({ ...valid, ...mutation })).toThrow();
    }
  });

  it.each([
    { source: "initialHydration", userInitiated: true, mode: "none", revision: 0 },
    { source: "initialHydration", userInitiated: false, mode: "none", revision: 1 },
    { source: "sessionCreated", userInitiated: false, mode: "none", revision: 1 },
    { source: "userSwitch", userInitiated: false, mode: "live", revision: 1 },
    { source: "manualClose", userInitiated: false, mode: "none", revision: 1 },
    { source: "autoClose", userInitiated: true, mode: "none", revision: 1 },
    { source: "restart", userInitiated: false, mode: "dormant", revision: 1 },
    { source: "restore", userInitiated: true, mode: "live", revision: 1 },
    { source: "detach", userInitiated: false, mode: "none", revision: 1 },
    { source: "attach", userInitiated: false, mode: "dormant", revision: 1 },
    { source: "spawnRollback", userInitiated: true, mode: "none", revision: 1 },
    { source: "resourceMonitor", userInitiated: false, mode: "live", revision: 1 },
    { source: "backgroundCleanup", userInitiated: true, mode: "none", revision: 1 },
    { source: "livenessReconcile", userInitiated: true, mode: "dormant", revision: 1 },
  ] as const)("rejects one-field source-policy mutation %#", (cause) => {
    const raw = rawPayload({
      source: cause.source,
      userInitiated: cause.userInitiated,
      mode: cause.mode,
    });
    raw.revision = cause.revision;
    expect(() => decodeSessionSelection(raw)).toThrow();
  });
});

const validCompileFixture = {
  epoch: TEST_EPOCH,
  source: "userSwitch",
  userInitiated: true,
  revision: 1,
  mode: "live",
  id: SESSION_A,
  status: "active",
  hasPty: true,
  detached: false,
  displayable: true,
} satisfies SessionSelection;
void validCompileFixture;

// @ts-expect-error userSwitch cannot publish none.
const invalidUserSwitchNone: SessionSelection = {
  epoch: TEST_EPOCH, source: "userSwitch", userInitiated: true, revision: 1,
  mode: "none", id: null, status: null, hasPty: false, detached: false, displayable: false,
};
// @ts-expect-error autoClose cannot be user initiated.
const invalidAutoCloseUser: SessionSelection = {
  epoch: TEST_EPOCH, source: "autoClose", userInitiated: true, revision: 1,
  mode: "none", id: null, status: null, hasPty: false, detached: false, displayable: false,
};
// @ts-expect-error dormant cannot be displayable or active.
const invalidDormantDisplay: SessionSelection = {
  epoch: TEST_EPOCH, source: "restore", userInitiated: false, revision: 1,
  mode: "dormant", id: SESSION_A, status: "active", hasPty: true, detached: false, displayable: true,
};
void invalidUserSwitchNone;
void invalidAutoCloseUser;
void invalidDormantDisplay;
