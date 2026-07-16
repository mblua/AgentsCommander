import type { SessionSelection } from "../types";

export const TEST_EPOCH = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
export const TEST_EPOCH_2 = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
export const SESSION_A = "11111111-1111-4111-8111-111111111111";
export const SESSION_B = "22222222-2222-4222-8222-222222222222";
export const SESSION_C = "33333333-3333-4333-8333-333333333333";
export const ROOT_SESSION = "44444444-4444-4444-8444-444444444444";

export function initialSelection(epoch = TEST_EPOCH): SessionSelection {
  return {
    epoch,
    source: "initialHydration",
    userInitiated: false,
    revision: 0,
    mode: "none",
    id: null,
    status: null,
    hasPty: false,
    detached: false,
    displayable: false,
  };
}

export function noneSelection(
  revision = 1,
  epoch = TEST_EPOCH,
): SessionSelection {
  return {
    epoch,
    source: "autoClose",
    userInitiated: false,
    revision,
    mode: "none",
    id: null,
    status: null,
    hasPty: false,
    detached: false,
    displayable: false,
  };
}

export function userNoneSelection(
  revision = 1,
  epoch = TEST_EPOCH,
): SessionSelection {
  return {
    epoch,
    source: "manualClose",
    userInitiated: true,
    revision,
    mode: "none",
    id: null,
    status: null,
    hasPty: false,
    detached: false,
    displayable: false,
  };
}

export function liveSelection(
  id = SESSION_A,
  revision = 1,
  epoch = TEST_EPOCH,
): SessionSelection {
  return {
    epoch,
    source: "restore",
    userInitiated: false,
    revision,
    mode: "live",
    id,
    status: "active",
    hasPty: true,
    detached: false,
    displayable: true,
  };
}

export function userLiveSelection(
  id = SESSION_A,
  revision = 1,
  epoch = TEST_EPOCH,
): SessionSelection {
  return {
    epoch,
    source: "userSwitch",
    userInitiated: true,
    revision,
    mode: "live",
    id,
    status: "active",
    hasPty: true,
    detached: false,
    displayable: true,
  };
}

export function dormantSelection(
  id = SESSION_A,
  revision = 1,
  exitCode = 0,
  epoch = TEST_EPOCH,
): SessionSelection {
  return {
    epoch,
    source: "restore",
    userInitiated: false,
    revision,
    mode: "dormant",
    id,
    status: { exited: exitCode },
    hasPty: false,
    detached: false,
    displayable: false,
  };
}
