// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import ProjectPanel from "./ProjectPanel";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  baseSettings,
  click,
  discovery,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  session,
  waitFor,
} from "../../shared/testing/ui-harness";
import { projectStore } from "../stores/project";
import { settingsStore } from "../../shared/stores/settings";
import type {
  AcAgentReplica,
  AgentConfig,
  ApplyCodingAgentProfileSelectionResult,
  CodingAgentProfileResolution,
  PreviewCodingAgentProfileSelectionResult,
} from "../../shared/types";

// #599 R1 — reopening a coordinator that AC tore down (auto-close #552/#580 or
// manual close #588) DESTROYS the session, so the double-click reopen lands on
// the "create in-place" path (no live/exited record) instead of the
// restart-with-resume branch. handleReplicaClick must carry a resume intent —
// derived purely from the autoClosedAt / manuallyClosedAt close markers — so the
// create injects `--continue`; a genuinely fresh first launch (no marker) must
// NOT resume. This drives the real ProjectPanel → AgentPickerModal → SessionAPI
// .create flow through the FakeTransport and asserts the recorded create_session
// `skipAutoResume` payload, per the frontend-visual-verification discipline.

const projectPath = "C:\\Project";
const workgroupPath = `${projectPath}\\.ac\\wg-2-dev-team`;
const coordPath = `${workgroupPath}\\__agent_dev-webpage-ui`;
const coordName = "dev-webpage-ui";
const coordSessionName = `wg-2-dev-team/${coordName}`;

/** A single configured coding agent so the picker can enable "Apply". */
function claudeAgent(): AgentConfig {
  return {
    id: "claude",
    label: "Claude Code",
    command: "claude",
    color: "#d97706",
    gitPullBefore: false,
    excludeGlobalClaudeMd: false,
    envs: [],
    isolatedHome: false,
  };
}

/** Discovery with one coordinator replica carrying the given close markers. */
function coordDiscovery(coord: Partial<AcAgentReplica>) {
  return discovery({
    workgroups: [
      {
        name: "wg-2-dev-team",
        path: workgroupPath,
        task: null,
        taskTitle: "Reopen resume",
        agents: [
          {
            name: coordName,
            path: coordPath,
            repoPaths: [],
            isCoordinator: true,
            ...coord,
          },
        ],
      },
    ],
  });
}

function resolution(): CodingAgentProfileResolution {
  return {
    requestedProfile: "A",
    effectiveProfile: "A",
    fallbackChain: ["A"],
    fallbackApplied: false,
    requestedProfileInput: null,
    instanceProfileOverride: null,
    originDefaultProfile: null,
    agentDefaultProfile: null,
    warnings: [],
  };
}

function previewResult(): PreviewCodingAgentProfileSelectionResult {
  return {
    scope: "replica",
    targetCount: 1,
    liveSessionCount: 0,
    targetFingerprint: "fp-replica",
    requiresExplicitConfirmation: false,
    targets: [],
    warnings: [],
  };
}

function applyResult(): ApplyCodingAgentProfileSelectionResult {
  return {
    scope: "replica",
    updatedCount: 1,
    restartedCount: 0,
    updatedReplicaPaths: [coordPath],
    restartedSessionIds: [],
    destroyedButNotRecreatedSessionIds: [],
    targetFingerprint: "fp-replica",
    warnings: [],
    errors: [],
  };
}

/** Mount ProjectPanel with a single (no-session) coordinator replica and every
 *  backend command the reopen → picker-apply → create flow touches. */
function mountWith(coord: Partial<AcAgentReplica>) {
  const fake = new FakeTransport();
  fake.resolve("new_project", { path: projectPath, registered: true, created: false });
  fake.resolve("get_settings", baseSettings({ agents: [claudeAgent()] }));
  fake.resolve("discover_project", coordDiscovery(coord));
  fake.resolve("resolve_coding_agent_profile", resolution());
  fake.resolve("preview_coding_agent_profile_selection", previewResult());
  fake.resolve("apply_coding_agent_profile_selection", applyResult());
  fake.resolve(
    "create_session",
    session({ id: "reopened", name: coordSessionName, workingDirectory: coordPath, isCoordinator: true }),
  );
  fake.resolve("switch_session", null);
  const rendered = renderWithFakeTransport(() => <ProjectPanel />, fake);
  return { fake, rendered };
}

/** Drive the real flow: load the project, click the coordinator row, then click
 *  the picker's "Assign to this replica" once it enables. Returns the recorded
 *  create_session args. */
async function reopenAndCapture(fake: FakeTransport, rendered: { root: HTMLElement }) {
  await settingsStore.load();
  await projectStore.createAndLoad(projectPath);
  await waitFor(() => expect(rendered.root.textContent).toContain(coordName));

  const row = rendered.root.querySelector(".replica-item");
  expect(row).not.toBeNull();
  click(row!);

  // The picker renders through a Portal at the document root.
  await waitFor(() => {
    const apply = document.body.querySelector<HTMLButtonElement>(
      '[data-ac-testid="agentPicker.apply"]',
    );
    expect(apply).not.toBeNull();
    expect(apply!.disabled).toBe(false);
  });
  click(document.body.querySelector('[data-ac-testid="agentPicker.apply"]')!);

  await waitFor(() => expect(fake.callsFor("create_session")).toHaveLength(1));
  return fake.lastCall("create_session")!.args;
}

describe("ProjectPanel coordinator reopen resume (#599 R1)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  it("passes skipAutoResume=false when reopening an auto-closed coordinator", async () => {
    const { fake, rendered } = mountWith({ autoClosedAt: "2026-06-24T00:00:00.000Z" });
    try {
      const args = await reopenAndCapture(fake, rendered);
      // The auto-closed marker is the "reopen of an already-run coordinator"
      // signal → resume the prior conversation.
      expect(args.skipAutoResume).toBe(false);
      expect(args.cwd).toBe(coordPath);
      expect(args.agentId).toBe("claude");
    } finally {
      rendered.cleanup();
    }
  });

  it("passes skipAutoResume=false when reopening a manually-closed coordinator", async () => {
    const { fake, rendered } = mountWith({ manuallyClosedAt: "2026-06-24T00:00:00.000Z" });
    try {
      const args = await reopenAndCapture(fake, rendered);
      expect(args.skipAutoResume).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("does NOT resume (skipAutoResume null) for a genuinely fresh first launch", async () => {
    // No autoClosedAt / manuallyClosedAt marker → this is a first launch, never a
    // reopen, so the create keeps its intentional fresh-conversation default.
    const { fake, rendered } = mountWith({});
    try {
      const args = await reopenAndCapture(fake, rendered);
      // SessionAPI.create maps an omitted skipAutoResume to null (the backend
      // treats absent/None as "skip resume = current behavior").
      expect(args.skipAutoResume).toBeNull();
    } finally {
      rendered.cleanup();
    }
  });
});
