import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { SessionSelection } from "../../shared/types";
import {
  liveSelection,
  SESSION_A,
  TEST_EPOCH,
} from "../../shared/testing/session-selection";
import { terminalStore } from "./terminal";

function attachSelection(revision = 1): SessionSelection {
  return {
    epoch: TEST_EPOCH,
    source: "attach",
    userInitiated: true,
    revision,
    mode: "live",
    id: SESSION_A,
    status: "active",
    hasPty: true,
    detached: false,
    displayable: true,
  };
}

describe("terminal selection source", () => {
  beforeEach(() => terminalStore.resetForTests());
  afterEach(() => terminalStore.resetForTests());

  it("retains an accepted attach source through reservation and matching", () => {
    const selection = attachSelection();
    terminalStore.observeConnection({ state: "connected", generation: 3 });

    expect(terminalStore.reserveSelection(selection, 3, false)).toBe(true);
    expect(terminalStore.selectionSource).toBe("attach");
    expect(terminalStore.matchesSelection(selection, 3)).toBe(true);
  });

  it("updates the source with a newer selection and clears it with test reset", () => {
    terminalStore.observeConnection({ state: "connected", generation: 1 });
    expect(terminalStore.reserveSelection(attachSelection(), 1, false)).toBe(true);
    expect(
      terminalStore.reserveSelection(liveSelection(SESSION_A, 2, TEST_EPOCH), 1, false),
    ).toBe(true);
    expect(terminalStore.selectionSource).toBe("restore");

    terminalStore.resetForTests();
    expect(terminalStore.selectionSource).toBeNull();
  });

  it("marks locked-session binding as an attach source", () => {
    terminalStore.bindLockedSession(
      {
        id: SESSION_A,
        name: "wg-1-dev-team/architect",
        shell: "pwsh",
        shellArgs: [],
        effectiveShellArgs: [],
        createdAt: "2026-08-21T00:00:00.000Z",
        workingDirectory: "C:\\Project\\.ac\\wg-1-dev-team\\__agent_architect",
        status: "running",
        waitingForInput: false,
        communication: null,
        pendingReview: false,
        lastPrompt: null,
        agentId: null,
        agentLabel: null,
        gitRepos: [],
        workgroupTask: null,
        isCoordinator: false,
        isRootAgent: false,
        token: "",
        agentKind: "codex",
        requestedProfile: null,
        effectiveProfile: null,
        profileFallbackChain: [],
        profileFallbackApplied: false,
      },
      terminalStore.taskWriteSeq,
    );
    expect(terminalStore.selectionSource).toBe("attach");
  });
});
