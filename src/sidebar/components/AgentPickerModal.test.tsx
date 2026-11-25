// @vitest-environment jsdom
// The testkit import comes first: it registers the shared ipc mock before anything loads ipc.
import {
  REPO_PATH,
  WG_REPLICA_PATH,
  WG_SCOPE_CONTEXT,
  applyResult,
  clickRadio,
  installPickerHooks,
  maybe,
  mockSettingsApi,
  renderPicker,
  resolution,
  settings,
  settle,
  state,
  target,
  text,
} from "./AgentPickerModal.testkit";
import { describe, expect, it } from "vitest";
import type { AppSettings, CodingAgentProfileResolution } from "../../shared/types";
import { composeEffectiveCommand, profileCellCommandText } from "../../shared/profile-utils";

describe("AgentPickerModal", () => {
  installPickerHooks();

  it("round9 picker lifetime stops deferred settings requests after disposal", async () => {
    let resolveSettings!: (value: AppSettings) => void;
    const pendingSettings = new Promise<AppSettings>((resolve) => {
      resolveSettings = resolve;
    });
    mockSettingsApi.get.mockReturnValueOnce(pendingSettings);
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    try {
      expect(mockSettingsApi.get).toHaveBeenCalledTimes(1);
    } finally {
      dispose();
    }
    const requests = [
      mockSettingsApi.previewSelectionLockRemoval,
      mockSettingsApi.previewCodingAgentProfileSelection,
      mockSettingsApi.getReplicaSelectionDefault,
      mockSettingsApi.resolveCodingAgentProfile,
    ];
    const counts = requests.map((request) => request.mock.calls.length);
    resolveSettings(state.currentSettings);
    // The onMount await was registered first; drain it and queued continuations.
    await pendingSettings;
    for (let turn = 0; turn < 12; turn += 1) await Promise.resolve();
    expect(requests.map((request) => request.mock.calls.length)).toEqual(counts);
    expect(maybe("agentPicker.providers")).toBeNull();
  });

  it("round9 picker lifetime installs deferred settings while live", async () => {
    let resolveSettings!: (value: AppSettings) => void;
    const pendingSettings = new Promise<AppSettings>((resolve) => {
      resolveSettings = resolve;
    });
    mockSettingsApi.get.mockReturnValueOnce(pendingSettings);
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    try {
      expect(mockSettingsApi.get).toHaveBeenCalledTimes(1);
      resolveSettings(state.currentSettings);
      await pendingSettings;
      for (let turn = 0; turn < 12; turn += 1) await Promise.resolve();
      expect(mockSettingsApi.previewSelectionLockRemoval).toHaveBeenCalledWith(
        expect.objectContaining({ targetReplicaPath: WG_REPLICA_PATH, scope: "replica" }),
      );
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({ targetReplicaPath: WG_REPLICA_PATH, codingAgentId: "codex", profile: "A" }),
      );
      expect(mockSettingsApi.getReplicaSelectionDefault).toHaveBeenCalledWith({
        targetReplicaPath: WG_REPLICA_PATH,
      });
      expect(mockSettingsApi.resolveCodingAgentProfile).toHaveBeenCalledWith(
        WG_REPLICA_PATH, "codex", "A",
      );
      expect(text("agentPicker.provider.codex")).toContain("Codex");
      expect(target("agentPicker.profile.A").getAttribute("data-ac-profile-letter")).toBe("A");
      expect(text("agentPicker.profile.A.pill")).toBe("MATCH");
    } finally {
      dispose();
    }
  });

  // The picker shows only the composed launch preview, still labelled Command.
  it("keeps the composed Command preview from base and profile text", async () => {
    const { dispose } = renderPicker();
    try {
      await settle();
      const cell = state.currentSettings.codingAgentProfiles.profilesByAgent.codex.A;
      const param = target("agentPicker.profile.A").querySelector(".agent-profile-param")!;
      const [label, value] = Array.from(param.children).map((child) => child.textContent);
      expect(label).toBe("Command ");
      expect(value).toBe(composeEffectiveCommand("codex", profileCellCommandText(cell)));
      expect(value).toContain("--model gpt-5");
      expect(target("agentPicker.profiles").textContent).not.toMatch(/Base command|Additional arguments/);
    } finally {
      dispose();
    }
  });

  it("renders the selector regions and the scope-picker apply button", async () => {
    const { dispose } = renderPicker();
    await settle();

    expect(target("agentPicker.providers")).toBeTruthy();
    expect(target("agentPicker.profiles")).toBeTruthy();
    expect(target("agentPicker.comparison")).toBeTruthy();
    expect(target("agentPicker.cancel")).toBeTruthy();
    expect(target("agentPicker.apply")).toBeTruthy();
    expect(text("agentPicker.apply")).toContain("Assign to this replica");
    expect(target("agentPicker.provider.codex").getAttribute("data-ac-agent-id")).toBe("codex");
    expect(target("agentPicker.profile.A").getAttribute("data-ac-profile-letter")).toBe("A");

    dispose();
  });

  it("hides broad scope when no scope context is supplied", async () => {
    const { dispose } = renderPicker({ scopeContext: undefined });
    await settle();

    expect(maybe("agentPicker.scope.replica")).toBeNull();
    expect(maybe("agentPicker.scope.kind")).toBeNull();
    expect(maybe("agentPicker.scope.workgroup")).toBeNull();
    // Replica scope is implied; apply is enabled.
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("preserves the no-agent empty state with apply disabled", async () => {
    state.currentSettings = settings({ agents: [] });
    mockSettingsApi.get.mockResolvedValue(state.currentSettings);
    const { dispose } = renderPicker({ currentAgentId: null });
    await settle();

    expect(document.body.textContent).toContain("No agents configured. Add agents in Settings.");
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

    dispose();
  });

  it("updates active provider state and the active comparison row when a coding agent is clicked", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    target<HTMLButtonElement>("agentPicker.provider.claude").click();
    await settle();

    expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("active");
    expect(text("agentPicker.comparison.row.claude")).toContain("Claude Code");
    expect(text("agentPicker.comparison.row.claude")).toContain("A direct");
    expect(text("agentPicker.comparison.row.claude.launchLine")).toBe(
      "claude claude --dangerously-skip-permissions",
    );

    dispose();
  });

  it("shows declared env vars for the selected profile and keeps comparison compact", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    target<HTMLButtonElement>("agentPicker.profile.B").click();
    await settle();

    expect(text("agentPicker.profile.B.env")).toContain("Declared env");
    expect(text("agentPicker.profile.B.env")).toContain("CODEX_PROFILE");
    expect(text("agentPicker.profile.B.env")).toContain("fast");
    expect(text("agentPicker.comparison.row.codex")).toContain("B-FAST direct");
    expect(target("agentPicker.comparison").querySelector("[data-ac-profile-letter]")).toBeNull();
    const comparisonText = text("agentPicker.comparison");
    expect(comparisonText).toContain("Same Profile In Other Agents");
    expect(comparisonText).not.toContain("Effective Projection");
    expect(comparisonText).not.toContain("Chosen pair");
    expect(comparisonText).not.toMatch(/Command Delta/i);
    expect(comparisonText).not.toMatch(/Env Summary/i);
    expect(comparisonText).not.toContain("2 env vars");
    expect(comparisonText).not.toContain("CODEX_PROFILE=fast");
    expect(maybe("agentPicker.fallback")).toBeNull();

    dispose();
  });

  it("each provider chip resolves its OWN per-agent label, not the highlighted agent's", async () => {
    // codex (agents[0] = primigenio) and claude each get a distinct A label.
    const base = settings();
    state.currentSettings = settings({
      codingAgentProfiles: {
        ...base.codingAgentProfiles,
        profileLabelsByAgent: {
          codex: { A: "alpha-codex" },
          claude: { A: "alpha-claude" },
        },
      },
    });
    mockSettingsApi.get.mockResolvedValue(state.currentSettings);
    // Repo path → no backend resolution; each provider's default resolves to A.
    const { dispose } = renderPicker({ agentPath: REPO_PATH, currentAgentId: "codex" });
    await settle();

    const chip = (id: string) =>
      target(`agentPicker.provider.${id}`)
        .querySelector(".agent-profile-provider-chip")
        ?.textContent?.trim();

    // codex is the highlighted row. With the :568 bug, claude's chip would resolve
    // against codex and read "A-ALPHA-CODEX". Each must show its OWN A label.
    expect(chip("codex")).toBe("A-ALPHA-CODEX");
    expect(chip("claude")).toBe("A-ALPHA-CLAUDE");

    dispose();
  });

  it("does not pre-select a coding agent on hover; selection stays click-only", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    // Baseline: codex is the initial selection and drives the active comparison row.
    expect(target("agentPicker.provider.codex").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-state")).toBe("active");

    // Hovering claude must NOT activate it nor update the active comparison row.
    const claudeCard = target<HTMLButtonElement>("agentPicker.provider.claude");
    claudeCard.dispatchEvent(new MouseEvent("mouseenter", { bubbles: false }));
    claudeCard.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    await settle();
    expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("inactive");
    expect(target("agentPicker.provider.codex").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("inactive");

    // Clicking still selects and updates the active comparison row.
    claudeCard.click();
    await settle();
    expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("active");
    expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("active");
    expect(text("agentPicker.comparison.row.claude")).toContain("A direct");
    expect(text("agentPicker.comparison.row.claude.launchLine")).toBe(
      "claude claude --dangerously-skip-permissions",
    );

    dispose();
  });

  it("shows fallback on missing profile cards and in the comparison row", async () => {
    const { dispose } = renderPicker({ agentPath: REPO_PATH });
    await settle();

    target<HTMLButtonElement>("agentPicker.profile.C").click();
    await settle();

    expect(target("agentPicker.profile.C").getAttribute("data-ac-state")).toBe("active");
    expect(text("agentPicker.profile.C")).toContain("Fallback C->B");
    expect(text("agentPicker.fallback")).toContain("C-REVIEW is not configured");
    expect(text("agentPicker.fallback")).toContain("A remains the final fallback");
    expect(text("agentPicker.comparison.row.codex")).toContain("C-REVIEW → B-FAST (fallback)");
    expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-profile-status")).toBe("fallback");

    dispose();
  });

  it("renders safe, workgroup-danger, and cross-workgroup kind scopes for a WG replica", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    expect(target("agentPicker.scope.replica")).toBeTruthy();
    expect(target("agentPicker.scope.kind")).toBeTruthy();
    expect(target("agentPicker.scope.workgroup")).toBeTruthy();
    // All three broad-scope previews are fetched up front, so each
    // radio button shows its true targetCount (not 0 for the unselected ones).
    expect(text("agentPicker.scope.replica")).toContain("1 replica");
    expect(text("agentPicker.scope.kind")).toContain("3 replicas");
    expect(text("agentPicker.scope.workgroup")).toContain("4 replicas");
    // Sonar S6772: the radio label text stays glued to the input and the count chip.
    expect(target("agentPicker.scope.replica").textContent).toBe("This replica 1 replica");
    // Replica scope is safe → apply enabled immediately.
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("keeps workgroup apply disabled until the arm checkbox is checked", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.workgroup");
    await settle();

    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("keeps kind apply disabled until the arm checkbox is checked", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();

    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
    expect(maybe("agentPicker.kindConfirm")).toBeNull();
    expect(target<HTMLInputElement>("agentPicker.armToggle").closest("label")?.textContent).toContain(
      "I understand this overwrites 3 replicas of this kind",
    );

    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("resets the kind arm checkbox when the profile changes", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    // Changing the profile must reset the checkbox confirmation and re-disable apply.
    target<HTMLButtonElement>("agentPicker.profile.B").click();
    await settle();
    expect(target<HTMLInputElement>("agentPicker.armToggle").checked).toBe(false);
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

    dispose();
  });

  it("sends confirmedTargetFingerprint and null typedConfirmation for a kind apply", async () => {
    const { dispose, onSelect } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "kind",
        codingAgentId: "codex",
        profile: "A",
        restartSessions: false,
        confirmedTargetFingerprint: "fp-kind",
        typedConfirmation: null,
      }),
    );
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "kind", restartSessions: false }),
    );

    dispose();
  });

  it("keeps the restart toggle for kind scope and carries it into preview and apply", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();

    target<HTMLInputElement>("agentPicker.restartToggle").click();
    await settle();

    expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "kind", restartSessions: true }),
    );

    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "kind",
        restartSessions: true,
        confirmedTargetFingerprint: "fp-kind",
        typedConfirmation: null,
      }),
    );

    dispose();
  });

  it("requires a backend preview fingerprint plus the arm checkbox for a workgroup apply", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.workgroup");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "workgroup",
        confirmedTargetFingerprint: "fp-wg",
        typedConfirmation: null,
      }),
    );

    dispose();
  });

  it("hides the restart toggle for replica scope and applies without a backend restart", async () => {
    // Replica scope is restarted via the post-assign "Restart now?" modal, so
    // the in-modal toggle is gone and apply never asks the backend to restart.
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    expect(maybe("agentPicker.restartToggle")).toBeNull();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();
    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "replica", restartSessions: false }),
    );

    dispose();
  });

  it("keeps the restart toggle for workgroup scope and carries it into preview and apply", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.workgroup");
    await settle();

    // Toggle is available again for the multi-target scope.
    target<HTMLInputElement>("agentPicker.restartToggle").click();
    await settle();

    // Restart change re-previews with restartSessions: true.
    expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenLastCalledWith(
      expect.objectContaining({ restartSessions: true }),
    );

    // Arm the danger gate, then apply.
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();
    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "workgroup", restartSessions: true }),
    );

    dispose();
  });

  it("renders live counts and the cross-workgroup target list from the backend preview", async () => {
    const { dispose } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();

    const targets = target("agentPicker.targets");
    expect(targets).toBeTruthy();
    expect(text("agentPicker.targets")).toContain("3 replica(s) across 3 room(s)");
    expect(text("agentPicker.targets")).toContain("3 live session(s)");
    // A replica with two live sessions is surfaced per-row.
    const rows = targets.querySelectorAll('[data-ac-role="row"]');
    expect(rows).toHaveLength(3);
    expect(targets.querySelector('[data-ac-live-sessions="2"]')).toBeTruthy();

    dispose();
  });

  it("renders apply errors and keeps the modal open without selecting", async () => {
    mockSettingsApi.applyCodingAgentProfileSelection.mockResolvedValue(
      applyResult({
        scope: "kind",
        updatedCount: 0,
        errors: [
          { code: "staleFingerprint", message: "Targets changed; rerun preview.", sessionIds: [], replicaPaths: [] },
        ],
      }),
    );
    const { dispose, onSelect } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    // The failure banner leads with the human-readable backend message
    // (not the internal code) and is surfaced loudly via a toast as well.
    expect(target("agentPicker.errors")).toBeTruthy();
    expect(text("agentPicker.errors")).toContain("Assignment failed");
    expect(text("agentPicker.errors")).toContain("Targets changed; rerun preview.");
    expect(maybe("agentPicker.toast")).toBeTruthy();
    expect(text("agentPicker.toast")).toContain("Targets changed; rerun preview.");
    // Modal stays open; selection is not committed; checkbox confirmation is reset.
    expect(onSelect).not.toHaveBeenCalled();
    expect(maybe("agentPicker.modal")).toBeTruthy();
    expect(target<HTMLInputElement>("agentPicker.armToggle").checked).toBe(false);

    dispose();
  });

  it("resets broad-scope confirmation and re-previews when stale fingerprint apply rejects", async () => {
    const staleFingerprintMessage =
      "Target selection changed. Rerun preview before applying profile selection.";
    mockSettingsApi.applyCodingAgentProfileSelection.mockRejectedValue(
      new Error(staleFingerprintMessage),
    );
    const { dispose, onSelect } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    clickRadio("agentPicker.scope.kind");
    await settle();
    target<HTMLInputElement>("agentPicker.armToggle").click();
    await settle();
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    mockSettingsApi.previewCodingAgentProfileSelection.mockClear();
    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(onSelect).not.toHaveBeenCalled();
    expect(text("agentPicker.toast")).toContain(staleFingerprintMessage);
    expect(target<HTMLInputElement>("agentPicker.armToggle").checked).toBe(false);
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
    expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(1);
    expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "kind",
        codingAgentId: "codex",
        profile: "A",
      }),
    );

    dispose();
  });

  it("applies a replica-scope selection through the backend and then commits", async () => {
    const { dispose, onSelect } = renderPicker({
      agentPath: WG_REPLICA_PATH,
      scopeContext: WG_SCOPE_CONTEXT,
      currentRequestedProfile: "A",
    });
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
      expect.objectContaining({
        scope: "replica",
        codingAgentId: "codex",
        profile: "A",
        confirmedTargetFingerprint: null,
        typedConfirmation: null,
      }),
    );
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ scope: "replica", effectiveProfile: "A" }),
    );

    dispose();
  });

  it("does not call the backend apply for a normal repo path", async () => {
    const { dispose, onSelect } = renderPicker({ agentPath: REPO_PATH, scopeContext: undefined });
    await settle();

    target<HTMLButtonElement>("agentPicker.apply").click();
    await settle();

    expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();
    expect(mockSettingsApi.previewCodingAgentProfileSelection).not.toHaveBeenCalled();
    expect(onSelect).toHaveBeenCalledWith(expect.objectContaining({ scope: "replica" }));

    dispose();
  });

  it("surfaces backend profile-resolution warnings without disabling apply", async () => {
    mockSettingsApi.resolveCodingAgentProfile.mockResolvedValue(
      resolution({
        requestedProfile: "A",
        effectiveProfile: "A",
        fallbackChain: ["A"],
        warnings: ["invalid persisted override ignored"],
      }),
    );
    const { dispose } = renderPicker();
    await settle();

    expect(text("agentPicker.fallback")).toContain("Profile warning: invalid persisted override ignored");
    expect(text("agentPicker.fallback")).not.toContain("launches with configured");
    expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

    dispose();
  });

  it("ignores stale backend resolution results after the selected coding agent changes", async () => {
    const pending: Array<{
      agentId: string;
      resolve: (value: CodingAgentProfileResolution) => void;
    }> = [];
    mockSettingsApi.resolveCodingAgentProfile.mockImplementation(
      (_agentPath: string | null, agentId: string) =>
        new Promise<CodingAgentProfileResolution>((resolveBackend) => {
          pending.push({ agentId, resolve: resolveBackend });
        }),
    );
    const { dispose } = renderPicker();
    await settle();

    target<HTMLButtonElement>("agentPicker.provider.claude").click();
    await settle();
    expect(pending.map((item) => item.agentId)).toEqual(["codex", "claude"]);

    pending[0].resolve(
      resolution({ requestedProfile: "C", effectiveProfile: "C", fallbackChain: ["C"], warnings: ["old warning"] }),
    );
    await settle();
    expect(text("agentPicker.fallback")).not.toContain("old warning");
    expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("active");

    dispose();
  });

  it("renders a per-card status pill (match / configured / fallback / MISSING)", async () => {
    // Default fixture: codex configures A + B; claude configures A only.
    const { dispose } = renderPicker({ agentPath: REPO_PATH, currentAgentId: "codex" });
    await settle();

    // Selected agent = codex. A baseline → MATCH; B has its own cell → CONFIGURED;
    // C has no cell anywhere → resolves through B → FALLBACK.
    expect(target("agentPicker.profile.A.pill").getAttribute("data-ac-state")).toBe("match");
    expect(text("agentPicker.profile.A.pill")).toContain("MATCH");
    expect(target("agentPicker.profile.B.pill").getAttribute("data-ac-state")).toBe("configured");
    expect(text("agentPicker.profile.B.pill")).toContain("CONFIGURED");
    expect(target("agentPicker.profile.C.pill").getAttribute("data-ac-state")).toBe("fallback");
    expect(text("agentPicker.profile.C.pill")).toContain("FALLBACK");

    // Switch to claude: B has no cell here but is configured on codex → MISSING.
    target<HTMLButtonElement>("agentPicker.provider.claude").click();
    await settle();
    expect(target("agentPicker.profile.A.pill").getAttribute("data-ac-state")).toBe("match");
    expect(target("agentPicker.profile.B.pill").getAttribute("data-ac-state")).toBe("missing");
    expect(text("agentPicker.profile.B.pill")).toContain("MISSING");

    dispose();
  });

  // Disable "Assign to this replica" when the pending selection still equals
  // the replica's current Coding Agent + Profile (assign flows opt in).
  describe("redundant replica assignment", () => {
    const REDUNDANT_TOOLTIP = "This replica already uses this Coding Agent + Profile.";

    it("disables apply with a tooltip while the selection matches the current pair", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
      });
      await settle();

      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(text("agentPicker.apply")).toContain("Assign to this replica");
      expect(apply.disabled).toBe(true);
      expect(apply.getAttribute("title")).toBe(REDUNDANT_TOOLTIP);

      dispose();
    });

    it("re-enables apply when a different coding agent is selected", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
      });
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      target<HTMLButtonElement>("agentPicker.provider.claude").click();
      await settle();

      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(apply.disabled).toBe(false);
      expect(apply.getAttribute("title")).toBeNull();

      dispose();
    });

    it("re-enables apply on a different profile and re-disables when the current pair returns", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
      });
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      // codex configures A and B; A differs from the current B.
      target<HTMLButtonElement>("agentPicker.profile.A").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      // Returning to the current letter re-disables (value comparison, not a touched flag).
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      dispose();
    });

    it("leaves apply enabled for a redundant pair when the opt-in is off (launch flow)", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        // disableRedundantReplicaAssign omitted → unchanged launch/legacy behavior
      });
      await settle();

      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(apply.disabled).toBe(false);
      expect(apply.getAttribute("title")).toBeNull();

      dispose();
    });

    // Drift overrides the redundancy disable. When the target session's
    // loaded profile no longer matches its configuration (profileOutdated), the
    // same-pair re-assign is meaningful (re-stamp the cell content + relaunch), so
    // "Assign to this replica" must stay ENABLED even on the otherwise-redundant pair.
    it("re-enables apply for a redundant pair when the target session has drifted", async () => {
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
        targetProfileOutdated: true,
      });
      await settle();

      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(text("agentPicker.apply")).toContain("Assign to this replica");
      // Without drift this exact pair would be disabled with the redundant tooltip
      // (see the first test in this block); drift flips it back on.
      expect(apply.disabled).toBe(false);
      expect(apply.getAttribute("title")).toBeNull();

      dispose();
    });

    it("scopes the disable to replica scope for a WG replica", async () => {
      const { dispose } = renderPicker({
        agentPath: WG_REPLICA_PATH,
        scopeContext: WG_SCOPE_CONTEXT,
        currentAgentId: "codex",
        currentRequestedProfile: "A",
        disableRedundantReplicaAssign: true,
      });
      await settle();

      // Replica scope + current pair → disabled with tooltip.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(target("agentPicker.apply").getAttribute("title")).toBe(REDUNDANT_TOOLTIP);

      // A different profile re-enables replica scope (redundancy is replica-only).
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);
      expect(target("agentPicker.apply").getAttribute("title")).toBeNull();

      dispose();
    });

    it("uses the backend instance override as the baseline so a stale session profile can't leak a no-op", async () => {
      // Live-session flow where the replica's persisted instance override ("C")
      // differs from the session's launch-time requested profile ("B"). The
      // backend ranks the override first (resolve_profile), so the modal resolves
      // to "C"; the redundancy baseline must be "C", not the stale "B".
      mockSettingsApi.resolveCodingAgentProfile.mockResolvedValue(
        resolution({
          requestedProfile: "C",
          effectiveProfile: "C",
          fallbackChain: ["C"],
          instanceProfileOverride: "C",
        }),
      );
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: "B",
        disableRedundantReplicaAssign: true,
      });
      await settle();

      // Opens resolved to the override "C" → redundant → disabled.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      // Picking a different letter ("A") is a real change → enabled.
      target<HTMLButtonElement>("agentPicker.profile.A").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      // Picking the override letter "C" is the replica's current pair → disabled
      // again. Without the override-aware baseline this would compare against the
      // stale "B" and wrongly stay enabled (a no-op assign + needless restart
      // prompt — exactly what the no-op guard blocks).
      target<HTMLButtonElement>("agentPicker.profile.C").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(target("agentPicker.apply").getAttribute("title")).toBe(REDUNDANT_TOOLTIP);

      dispose();
    });

    it("uses the origin default profile tier as the baseline (FIX 1)", async () => {
      // Replica with an origin-matrix default profile of "C" (set via "set default
      // profile") and NO instance override / explicit requested profile. The backend
      // fallback chain ranks origin_default above agent_default, so the redundancy
      // baseline must be the origin "C" — not the local defaultProfileByAgent ("B"
      // for "architect" in the fixture), which the buggy memo read instead.
      mockSettingsApi.resolveCodingAgentProfile.mockResolvedValue(
        resolution({
          requestedProfile: "B",
          effectiveProfile: "B",
          fallbackChain: ["B"],
          instanceProfileOverride: null,
          originDefaultProfile: "C",
          agentDefaultProfile: "B",
        }),
      );
      const { dispose } = renderPicker({
        currentAgentId: "codex",
        currentRequestedProfile: null,
        disableRedundantReplicaAssign: true,
      });
      await settle();

      // Selecting the origin-default letter "C" is the replica's current pair → no-op
      // → disabled. (The buggy memo resolved the baseline to "B" and wrongly enabled.)
      target<HTMLButtonElement>("agentPicker.profile.C").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(target("agentPicker.apply").getAttribute("title")).toBe(REDUNDANT_TOOLTIP);

      // Selecting the local agent-default letter "B" is a genuine change away from the
      // origin default → enabled. (The buggy memo treated "B" as current → disabled.)
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);
      expect(target("agentPicker.apply").getAttribute("title")).toBeNull();

      dispose();
    });

    it("keeps assign enabled for a never-assigned replica on its preferred-agent hint (FIX 2)", async () => {
      // Gray replica that was never assigned a coding agent (no currentCodingAgentId)
      // but carries a lastCodingAgent/preferredAgentId hint ("codex"). The picker
      // opens pre-selected on that hint, but with no EXPLICIT current agent the assign
      // is a genuine first pin — never a redundant no-op — so it must stay enabled.
      const { dispose } = renderPicker({
        currentAgentId: "codex", // preferredAgentId hint → pre-selects the picker
        explicitCurrentAgentId: null, // never assigned → no explicit current agent
        currentRequestedProfile: null,
        disableRedundantReplicaAssign: true,
      });
      await settle();

      // Pre-selected on the preferred agent + its default profile, yet enabled.
      const apply = target<HTMLButtonElement>("agentPicker.apply");
      expect(apply.disabled).toBe(false);
      expect(apply.getAttribute("title")).toBeNull();

      // Re-affirming the preferred agent and its default profile keeps it enabled —
      // the user can pin the hinted pair in one click.
      target<HTMLButtonElement>("agentPicker.provider.codex").click();
      target<HTMLButtonElement>("agentPicker.profile.A").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });
  });

});
