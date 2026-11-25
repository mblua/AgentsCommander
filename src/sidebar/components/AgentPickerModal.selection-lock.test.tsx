// @vitest-environment jsdom
// The testkit import comes first: it registers the shared ipc mock before anything loads ipc.
import {
  WG_REPLICA_PATH,
  WG_SCOPE_CONTEXT,
  clickRadio,
  defaultResult,
  installPickerHooks,
  maybe,
  mockSettingsApi,
  previewResult,
  removalApplyResult,
  removePreview,
  renderPicker,
  settle,
  target,
  text,
} from "./AgentPickerModal.testkit";
import { describe, expect, it, vi } from "vitest";
import { createSignal } from "solid-js";
import { render } from "solid-js/web";
import AgentPickerModal from "./AgentPickerModal";
import type {
  ApplySelectionLockRemovalResult,
  PreviewCodingAgentProfileSelectionResult,
  PreviewSelectionLockRemovalResult,
  ProfileAssignmentScope,
  ReplicaSelectionDefaultResult,
  SelectionState,
} from "../../shared/types";

describe("AgentPickerModal", () => {
  installPickerHooks();

  // The approved selection-lock picker. The fingerprint echoed with each
  // policy is the whole safety mechanism, so every assertion here pins exactly
  // what the backend receives (and, for Cancel, that it receives nothing).
  describe("selection lock picker", () => {
    function renderLockPicker(overrides: Parameters<typeof renderPicker>[0] = {}) {
      return renderPicker({
        agentPath: WG_REPLICA_PATH,
        scopeContext: WG_SCOPE_CONTEXT,
        currentRequestedProfile: "A",
        disableRedundantReplicaAssign: true,
        ...overrides,
      });
    }

    function scopeRadios(): HTMLInputElement[] {
      return Array.from(
        document.querySelectorAll<HTMLInputElement>('input[name="agentPickerScope"]'),
      );
    }

    async function armKindLockAndReview(): Promise<void> {
      clickRadio("agentPicker.scope.lock.kind");
      await settle();
      target<HTMLInputElement>("agentPicker.armToggle").click();
      await settle();
      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();
    }

    it("renders six radios as ONE selection while keeping the ordinary scope row", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      // The ordinary row keeps its three test ids and its live counts.
      expect(text("agentPicker.scope.replica")).toContain("1 replica");
      expect(text("agentPicker.scope.kind")).toContain("3 replicas");
      expect(text("agentPicker.scope.workgroup")).toContain("4 replicas");
      // The lock row repeats exactly the same three scopes.
      expect(text("agentPicker.scope.lock.replica")).toContain("This replica + lock");
      expect(text("agentPicker.scope.lock.kind")).toContain("All replicas of this kind + lock");
      expect(text("agentPicker.scope.lock.workgroup")).toContain("Entire room + lock");

      expect(scopeRadios()).toHaveLength(6);
      expect(scopeRadios().filter((input) => input.checked)).toHaveLength(1);

      clickRadio("agentPicker.scope.lock.kind");
      await settle();

      // Still one selection, and the label states the policy it will apply.
      expect(scopeRadios().filter((input) => input.checked)).toHaveLength(1);
      expect(target("agentPicker.scope.lock.kind").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.scope.kind").getAttribute("data-ac-state")).toBe("inactive");
      expect(text("agentPicker.apply")).toContain("Overwrite 3 of this kind + lock");
      // Every scope is re-previewed for the new mode, keeping all six counts real.
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({ scope: "kind", assignmentMode: "assignAndLock" }),
      );

      dispose();
    });

    it("keeps same-pair assignAndLock enabled when the ordinary redundant assign is disabled", async () => {
      const { dispose } = renderLockPicker({ currentAgentId: "codex", currentRequestedProfile: "A" });
      await settle();

      // Ordinary replica scope on the already-current pair is the no-op.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      clickRadio("agentPicker.scope.lock.replica");
      await settle();

      // Writing the same pair PLUS the lock is a real change, never a no-op.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);
      expect(text("agentPicker.apply")).toContain("Assign + lock this replica");

      dispose();
    });

    it("sends the replica preview fingerprint for This replica + lock", async () => {
      const { dispose, onSelect } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.scope.lock.replica");
      await settle();
      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();

      // The replica preview IS the confirmation: echoing its own
      // fingerprint is what makes the one-step lock apply possible.
      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "replica",
          assignmentMode: "assignAndLock",
          restartSessions: false,
          confirmedTargetFingerprint: "fp-replica",
        }),
      );
      expect(onSelect).toHaveBeenCalled();

      dispose();
    });

    it("blocks This replica + lock until the replica preview is loaded", async () => {
      let resolveReplica: (value: PreviewCodingAgentProfileSelectionResult) => void = () => {};
      mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
        (req: { scope: string }) => {
          if (req.scope === "replica") {
            return new Promise<PreviewCodingAgentProfileSelectionResult>((resolve) => {
              resolveReplica = resolve;
            });
          }
          return Promise.resolve(previewResult({ scope: "kind", targetFingerprint: "fp-kind" }));
        },
      );
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.scope.lock.replica");
      await settle();

      // The fingerprint is the confirmation, so the button cannot be
      // clicked before the preview that mints it has landed.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(text("agentPicker.previewBusy")).toContain("Loading targets");

      resolveReplica(previewResult({ scope: "replica", targetFingerprint: "fp-replica-fresh" }));
      await settle();

      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });

    it("re-previews replica scope after a rejected replica + lock apply", async () => {
      const staleFingerprintMessage =
        "stalePreview: Target selection changed. Rerun preview before applying profile selection.";
      mockSettingsApi.applyCodingAgentProfileSelection.mockRejectedValue(
        new Error(staleFingerprintMessage),
      );
      const { dispose, onSelect } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.scope.lock.replica");
      await settle();

      mockSettingsApi.previewCodingAgentProfileSelection.mockClear();
      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();

      // A rejected fingerprint is never replayed: the replica preview is
      // refreshed (D5) so the next click carries the current value.
      expect(onSelect).not.toHaveBeenCalled();
      expect(text("agentPicker.toast")).toContain(staleFingerprintMessage);
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(1);
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({ scope: "replica", assignmentMode: "assignAndLock" }),
      );

      dispose();
    });

    it("hashes the replica preview with the restart value the apply sends", async () => {
      const calls: Array<{ scope: string; restartSessions?: boolean }> = [];
      const basePreview =
        mockSettingsApi.previewCodingAgentProfileSelection.getMockImplementation();
      mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
        (req: { scope: string; restartSessions?: boolean }) => {
          calls.push({ scope: req.scope, restartSessions: req.restartSessions });
          if (req.scope === "replica") {
            // Mimic the backend: the restart flag is part of the hashed tuple.
            return Promise.resolve(
              previewResult({
                scope: "replica",
                targetFingerprint: req.restartSessions ? "fp-replica-restart" : "fp-replica",
              }),
            );
          }
          return basePreview?.(req) ?? Promise.resolve(previewResult({ scope: "kind" }));
        },
      );
      const { dispose } = renderLockPicker();
      await settle();

      // A restart choice made on a bulk scope must not leak into the replica
      // hash once the hidden toggle survives the switch.
      clickRadio("agentPicker.scope.kind");
      await settle();
      target<HTMLInputElement>("agentPicker.restartToggle").click();
      await settle();
      clickRadio("agentPicker.scope.lock.replica");
      await settle();

      const replicaCalls = calls.filter((call) => call.scope === "replica");
      expect(replicaCalls.length).toBeGreaterThan(0);
      for (const call of replicaCalls) {
        expect(call.restartSessions).toBe(false);
      }

      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();

      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "replica",
          restartSessions: false,
          confirmedTargetFingerprint: "fp-replica",
        }),
      );

      dispose();
    });

    it("opens an accessible review for a conflicting bulk lock and sends nothing on Cancel", async () => {
      const { dispose, onSelect } = renderLockPicker();
      await settle();

      await armKindLockAndReview();

      // The review opens INSTEAD of writing: no policy has been chosen yet.
      expect(maybe("agentPicker.conflict")).toBeTruthy();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      const dialog = target("agentPicker.conflict");
      expect(dialog.querySelector('[role="alertdialog"]')).toBeTruthy();
      const rows = dialog.querySelectorAll('[data-ac-role="row"]');
      // Every protected candidate is listed, INCLUDING the one already on the
      // requested pair.
      expect(rows).toHaveLength(2);
      expect(dialog.textContent).toContain("2 replicas are already locked");

      target<HTMLButtonElement>("agentPicker.conflict.cancel").click();
      await settle();

      expect(maybe("agentPicker.conflict")).toBeNull();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();
      expect(onSelect).not.toHaveBeenCalled();

      dispose();
    });

    it("closes the review on Escape without sending anything", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      await armKindLockAndReview();
      expect(maybe("agentPicker.conflict")).toBeTruthy();

      const overlay = target("agentPicker.overlay");
      overlay.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await settle();

      expect(maybe("agentPicker.conflict")).toBeNull();
      expect(maybe("agentPicker.modal")).toBeTruthy();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("sends only unlocked candidates with the fingerprint of that exact policy", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      await armKindLockAndReview();
      target<HTMLButtonElement>("agentPicker.conflict.unlockedOnly").click();
      await settle();

      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "kind",
          assignmentMode: "assignAndLock",
          conflictDecision: "unlockedOnly",
          confirmedTargetFingerprint: "fp-unlocked-only",
        }),
      );

      dispose();
    });

    it("sends the force fingerprint only after the force was actually reviewed", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      await armKindLockAndReview();
      target<HTMLButtonElement>("agentPicker.conflict.forceAll").click();
      await settle();

      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledWith(
        expect.objectContaining({
          scope: "kind",
          assignmentMode: "assignAndLock",
          conflictDecision: "forceReviewed",
          confirmedTargetFingerprint: "fp-force-all",
        }),
      );

      dispose();
    });

    it("drops the reviewed force and the arming when the operation changes", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      await armKindLockAndReview();
      target<HTMLButtonElement>("agentPicker.conflict.forceAll").click();
      await settle();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenLastCalledWith(
        expect.objectContaining({ conflictDecision: "forceReviewed" }),
      );

      // A new profile is a NEW operation: the reviewed force cannot be replayed.
      mockSettingsApi.applyCodingAgentProfileSelection.mockClear();
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      expect(target<HTMLInputElement>("agentPicker.armToggle").checked).toBe(false);
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      // Re-arming is not enough: the fresh preview demands a fresh review.
      target<HTMLInputElement>("agentPicker.armToggle").click();
      await settle();
      expect(target<HTMLInputElement>("agentPicker.armToggle").disabled).toBe(false);
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);
      target<HTMLButtonElement>("agentPicker.apply").click();
      await settle();

      expect(maybe("agentPicker.conflict")).toBeTruthy();
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("lists only protected rows for a bulk lock while the totals stay complete", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.scope.lock.kind");
      await settle();

      const list = target("agentPicker.lockKindList");
      // Two protected rows of three candidates: the unprotected one is not drawn.
      expect(list.querySelectorAll('[data-ac-role="row"]')).toHaveLength(2);
      // The head still reports the complete total, and so does the radio count.
      expect(text("agentPicker.lockKindList")).toContain("3 replica(s) of this kind");
      expect(text("agentPicker.lockKindList")).toContain("3 room(s)");
      expect(text("agentPicker.scope.lock.kind")).toContain("3 replicas");

      dispose();
    });

    it("counts protection per removal scope, independent of the focused replica", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      // The focused replica is unlocked, so ITS scope has nothing to remove...
      expect(text("agentPicker.removeScopeCount.replica")).toBe("0 protected");
      expect(text("agentPicker.removeLock")).toBe("Nothing to remove");
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(true);
      expect(text("agentPicker.removeNote")).toContain("No protected replicas in this scope");

      // ...while a scope with protected peers stays actionable: the focused
      // replica never decides for the whole scope.
      clickRadio("agentPicker.removeScope.kind");
      await settle();
      expect(text("agentPicker.removeScopeCount.kind")).toBe("2 of 3 protected");
      expect(text("agentPicker.removeLock")).toBe("Remove lock from 2 replicas");
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(false);

      clickRadio("agentPicker.removeScope.workgroup");
      await settle();
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("3 of 4 protected");
      expect(text("agentPicker.removeLock")).toBe("Remove lock from 3 replicas");

      // The removal scope never moves the Apply to selection.
      expect(target("agentPicker.scope.replica").getAttribute("data-ac-state")).toBe("active");
      expect(text("agentPicker.apply")).toContain("Assign to this replica");

      dispose();
    });

    it("reports a complete room removal with the count, the kept pair and no restart", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.removeScope.workgroup");
      await settle();
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("3 of 4 protected");

      // After the removal the scope is re-enumerated from the backend, so the
      // chip shows the authoritative 0 of 4 instead of a local guess.
      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        (req: { scope: ProfileAssignmentScope }) =>
          Promise.resolve(
            removePreview(req.scope, {
              protectedCount: 0,
              alreadyUnlockedCount: req.scope === "workgroup" ? 4 : 1,
            }),
          ),
      );
      mockSettingsApi.applySelectionLockRemoval.mockResolvedValue(
        removalApplyResult({
          scope: "workgroup",
          targetFingerprint: "fp-remove-workgroup",
          removedCount: 3,
          removedReplicaPaths: [WG_REPLICA_PATH, "p2", "p3"],
          candidateCount: 4,
          remainingProtectedCount: 0,
        }),
      );

      target<HTMLButtonElement>("agentPicker.removeLock").click();
      await settle();

      // A removal carries no pair, no restart and no decision.
      expect(mockSettingsApi.applySelectionLockRemoval).toHaveBeenCalledWith({
        targetReplicaPath: WG_REPLICA_PATH,
        scope: "workgroup",
        confirmedTargetFingerprint: "fp-remove-workgroup",
      });
      expect(text("agentPicker.removeDone")).toBe(
        "Lock removed from 3 replicas · Coding Agent + Profile kept · no restart",
      );
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("0 of 4 protected");
      expect(text("agentPicker.removeLock")).toBe("Nothing to remove");
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("reports the count that landed and never claims 0 of N on an unknown total", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.removeScope.workgroup");
      await settle();

      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        (req: { scope: ProfileAssignmentScope }) =>
          Promise.resolve(
            removePreview(req.scope, {
              countsComplete: req.scope !== "workgroup",
            }),
          ),
      );
      mockSettingsApi.applySelectionLockRemoval.mockResolvedValue(
        removalApplyResult({
          scope: "workgroup",
          targetFingerprint: "fp-remove-workgroup",
          removedCount: 2,
          candidateCount: 4,
          remainingProtectedCount: null,
          failedReplicaPaths: ["p4"],
          errors: [
            {
              code: "configWriteFailed",
              message: "p4 could not be written",
              sessionIds: [],
              replicaPaths: ["p4"],
            },
          ],
        }),
      );

      target<HTMLButtonElement>("agentPicker.removeLock").click();
      await settle();

      expect(text("agentPicker.removeDone")).toContain("Lock removed from 2 replicas");
      expect(text("agentPicker.removeErrors")).toContain("p4 could not be written");
      // An incomplete enumeration is never rendered as a confident zero.
      expect(text("agentPicker.lockState")).toBe("Protection unknown");
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("count unknown");
      expect(text("agentPicker.removeLock")).toBe("Nothing to remove");

      dispose();
    });

    it("never draws an invalid protection state as unlocked, and disables the lock controls", async () => {
      const { dispose } = renderLockPicker({
        scopeContext: {
          ...WG_SCOPE_CONTEXT,
          selectionState: "invalid",
          selectionError: "identity unreadable",
        },
      });
      await settle();

      expect(text("agentPicker.lockDiagnostic")).toContain("identity unreadable");
      expect(text("agentPicker.lockState")).toBe("Check state");
      expect(text("agentPicker.lockState")).not.toBe("Unlocked");
      expect(
        target("agentPicker.scope.lock.replica").querySelector<HTMLInputElement>("input")?.disabled,
      ).toBe(true);
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(true);
      expect(target<HTMLButtonElement>("agentPicker.defaultSave").disabled).toBe(true);

      dispose();
    });

    it("blocks an already-selected + lock when the protection state stops being readable", async () => {
      // The scope context is reactive here on purpose: the reviewer's scenario is
      // an external update that turns a usable state into an unreadable one AFTER
      // `+ lock` was already chosen.
      const [lockState, setLockState] = createSignal<SelectionState | undefined>("unlocked");
      const root = document.createElement("div");
      document.body.append(root);
      const dispose = render(
        () => (
          <AgentPickerModal
            sessionName="wg-7-dev-team/dev-webpage-ui"
            agentPath={WG_REPLICA_PATH}
            currentAgentId="codex"
            explicitCurrentAgentId="codex"
            currentRequestedProfile="A"
            scopeContext={{ ...WG_SCOPE_CONTEXT, selectionState: lockState() }}
            disableRedundantReplicaAssign
            onSelect={vi.fn()}
            onClose={vi.fn()}
          />
        ),
        root,
      );
      await settle();

      clickRadio("agentPicker.scope.lock.kind");
      await settle();
      target<HTMLInputElement>("agentPicker.armToggle").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      // Another window invalidates the protection and discovery reloads.
      setLockState("invalid");
      await settle();

      expect(text("agentPicker.lockState")).toBe("Check state");
      // The lock operation is BLOCKED, never silently demoted to an ordinary
      // assignment: demoting would change the policy the user chose.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(target("agentPicker.scope.lock.kind").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.scope.kind").getAttribute("data-ac-state")).toBe("inactive");
      const lockRadio = target("agentPicker.scope.lock.kind").querySelector<HTMLInputElement>("input");
      expect(lockRadio?.checked).toBe(true);
      expect(lockRadio?.disabled).toBe(true);

      // Re-arming cannot smuggle the lock operation through either.
      target<HTMLInputElement>("agentPicker.armToggle").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
      document.body.replaceChildren();
    });

    it("diagnoses a protection state this build did not receive", async () => {
      const { dispose } = renderLockPicker({
        // Redundancy is disabled here so `apply` reports ONLY the lock state.
        disableRedundantReplicaAssign: false,
        scopeContext: { ...WG_SCOPE_CONTEXT, selectionState: undefined, savedPair: undefined },
      });
      await settle();

      expect(text("agentPicker.lockDiagnostic")).toContain("was not reported");
      // The chip must never claim the reassuring state while the diagnostic right
      // beside it says the protection state was not reported at all.
      expect(text("agentPicker.lockState")).toBe("State unknown");
      expect(text("agentPicker.lockState")).not.toBe("Unlocked");
      // A bulk count is not a claim about the focused replica, so it survives.
      expect(text("agentPicker.removeScopeCount.kind")).toBe("2 of 3 protected");
      expect(
        target("agentPicker.scope.lock.replica").querySelector<HTMLInputElement>("input")?.disabled,
      ).toBe(true);
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(true);
      // Assignment itself is untouched by an unknown lock state.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });

    it("shows the stored Matrix default and saves the draft only when asked", async () => {
      mockSettingsApi.setReplicaSelectionDefault.mockImplementation(
        (req: { codingAgentId: string; requestedProfile: string; selectionLocked: boolean }) =>
          Promise.resolve(
            defaultResult({
              default: {
                codingAgentId: req.codingAgentId,
                requestedProfile: req.requestedProfile,
                selectionLocked: req.selectionLocked,
              },
              defaultFingerprint: "fp-default-2",
            }),
          ),
      );
      const { dispose } = renderLockPicker();
      await settle();

      expect(text("agentPicker.defaultPersisted")).toContain("Codex · Profile A");
      expect(text("agentPicker.defaultPersisted")).toContain("Start unlocked");

      // Neither a picker change nor the draft toggle saves anything.
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      target<HTMLInputElement>("agentPicker.defaultStartLocked").click();
      await settle();
      expect(mockSettingsApi.setReplicaSelectionDefault).not.toHaveBeenCalled();
      expect(text("agentPicker.defaultPersisted")).toContain("Codex · Profile A");

      target<HTMLButtonElement>("agentPicker.defaultSave").click();
      await settle();

      // The SAVE writes the currently selected pair with the draft flag, against
      // the latest fingerprint the backend gave us.
      expect(mockSettingsApi.setReplicaSelectionDefault).toHaveBeenCalledWith({
        targetReplicaPath: WG_REPLICA_PATH,
        codingAgentId: "codex",
        requestedProfile: "B",
        selectionLocked: true,
        confirmedDefaultFingerprint: "fp-default-1",
      });
      // Only the confirmed payload replaces the stored view.
      expect(text("agentPicker.defaultPersisted")).toContain("Codex · Profile B");
      expect(text("agentPicker.defaultPersisted")).toContain("Start locked");
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("keeps the stored default and the draft when the default save is stale", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();
      target<HTMLInputElement>("agentPicker.defaultStartLocked").click();
      await settle();

      mockSettingsApi.setReplicaSelectionDefault.mockRejectedValue(
        new Error("staleDefaultFingerprint"),
      );
      target<HTMLButtonElement>("agentPicker.defaultSave").click();
      await settle();

      expect(text("agentPicker.defaultNotice")).toContain(
        "Review the refreshed default before retrying",
      );
      // The stored view keeps the CONFIRMED pair, not the unsaved draft.
      expect(text("agentPicker.defaultPersisted")).toContain("Codex · Profile A");
      expect(text("agentPicker.defaultPersisted")).toContain("Start unlocked");
      // The draft survives for the retry, and a refreshed default was fetched.
      expect(target<HTMLInputElement>("agentPicker.defaultStartLocked").checked).toBe(true);
      expect(mockSettingsApi.getReplicaSelectionDefault.mock.calls.length).toBeGreaterThan(1);

      dispose();
    });

    it("keeps the mutating controls disabled until the removal settles", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      clickRadio("agentPicker.removeScope.kind");
      await settle();

      let resolveRemoval!: (value: ApplySelectionLockRemovalResult) => void;
      mockSettingsApi.applySelectionLockRemoval.mockImplementation(
        () =>
          new Promise<ApplySelectionLockRemovalResult>((resolve) => {
            resolveRemoval = resolve;
          }),
      );

      target<HTMLButtonElement>("agentPicker.removeLock").click();
      await settle();

      // The promise stays open until the operation really settles.
      expect(target<HTMLButtonElement>("agentPicker.removeLock").disabled).toBe(true);

      resolveRemoval(removalApplyResult({ scope: "kind", removedCount: 2, candidateCount: 3 }));
      await settle();

      expect(text("agentPicker.removeDone")).toContain("Lock removed from 2 replicas");
      expect(mockSettingsApi.applySelectionLockRemoval).toHaveBeenCalledTimes(1);

      dispose();
    });

    it("blocks a second mutation while a removal or a default save is in flight", async () => {
      // The redundant-pair opt-in is off so the ONLY thing that can disable
      // Apply in this test is the pending mutation itself.
      const { dispose } = renderLockPicker({ disableRedundantReplicaAssign: false });
      await settle();

      // Baseline: an ordinary replica assign needs no arming and no lock state.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      let resolveRemoval!: (value: ApplySelectionLockRemovalResult) => void;
      mockSettingsApi.applySelectionLockRemoval.mockImplementation(
        () =>
          new Promise<ApplySelectionLockRemovalResult>((resolve) => {
            resolveRemoval = resolve;
          }),
      );
      clickRadio("agentPicker.removeScope.kind");
      await settle();
      target<HTMLButtonElement>("agentPicker.removeLock").click();
      await settle();

      // A removal is an owned backend operation: no concurrent assignment.
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      resolveRemoval(removalApplyResult({ scope: "kind", removedCount: 2, candidateCount: 3 }));
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      // And the same rule for a default save that has not settled.
      let resolveDefault!: (value: ReplicaSelectionDefaultResult) => void;
      mockSettingsApi.setReplicaSelectionDefault.mockImplementation(
        () =>
          new Promise<ReplicaSelectionDefaultResult>((resolve) => {
            resolveDefault = resolve;
          }),
      );
      target<HTMLButtonElement>("agentPicker.defaultSave").click();
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(true);

      resolveDefault(defaultResult());
      await settle();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });

    it("reloads its own previews and default when another window updates the selection", async () => {
      let externalUpdate: (() => void) | null = null;
      mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockImplementation(
        (callback: () => void) => {
          externalUpdate = callback;
          return Promise.resolve(() => {});
        },
      );
      const { dispose } = renderLockPicker();
      await settle();
      expect(externalUpdate).toBeTruthy();

      mockSettingsApi.previewSelectionLockRemoval.mockClear();
      mockSettingsApi.getReplicaSelectionDefault.mockClear();
      mockSettingsApi.previewCodingAgentProfileSelection.mockClear();

      externalUpdate!();
      await settle();

      expect(mockSettingsApi.previewSelectionLockRemoval).toHaveBeenCalledTimes(3);
      expect(mockSettingsApi.getReplicaSelectionDefault).toHaveBeenCalledTimes(1);
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(3);

      dispose();
    });

    it("releases the external-update listener on unmount", async () => {
      const unlisten = vi.fn();
      mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockImplementation(() =>
        Promise.resolve(unlisten),
      );
      const { dispose } = renderLockPicker();
      await settle();

      dispose();
      await settle();

      expect(unlisten).toHaveBeenCalledTimes(1);
    });

    it("drops a stale removal preview so the newest scope state wins", async () => {
      const pending: Array<{
        scope: string;
        resolve: (value: PreviewSelectionLockRemovalResult) => void;
      }> = [];
      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        (req: { scope: string }) =>
          new Promise<PreviewSelectionLockRemovalResult>((resolve) => {
            pending.push({ scope: req.scope, resolve });
          }),
      );
      let externalUpdate: (() => void) | null = null;
      mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockImplementation(
        (callback: () => void) => {
          externalUpdate = callback;
          return Promise.resolve(() => {});
        },
      );
      const { dispose } = renderLockPicker();
      await settle();
      expect(pending).toHaveLength(3);

      externalUpdate!();
      await settle();
      expect(pending).toHaveLength(6);

      const stale = pending[0];
      const fresh = pending[3];
      expect(stale.scope).toBe("replica");
      expect(fresh.scope).toBe("replica");

      // The newest request lands first; the older reply arrives LAST and must be
      // dropped by the per-scope sequence token.
      fresh.resolve(removePreview("replica", { protectedCount: 1, alreadyUnlockedCount: 0 }));
      await settle();
      stale.resolve(removePreview("replica", { protectedCount: 0, alreadyUnlockedCount: 1 }));
      await settle();

      expect(text("agentPicker.removeScopeCount.replica")).toBe("1 protected");

      dispose();
    });

    it("places the lock bar below the three-panel body and keeps the review over the modal", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      const modal = target("agentPicker.modal");
      const bar = target("agentPicker.lockBar");
      const body = modal.querySelector(".agent-profile-assignment-body");
      expect(body).toBeTruthy();
      // Layout A: the bar is a flex child of the modal BELOW the
      // three-panel body, so DOM order IS the layout contract.
      expect(bar.parentElement).toBe(modal);
      expect(
        body!.compareDocumentPosition(bar) & Node.DOCUMENT_POSITION_FOLLOWING,
      ).toBeTruthy();

      await armKindLockAndReview();
      const dialog = target("agentPicker.conflict");
      // The review covers the whole overlay instead of being clipped inside the
      // modal's own overflow, which is why it is a sibling of the modal.
      expect(modal.parentElement?.classList.contains("modal-overlay")).toBe(true);
      expect(dialog.parentElement).toBe(modal.parentElement);

      dispose();
    });

    it("O1 layout A order: body, lock bar, Matrix default, Apply to, action bar last", async () => {
      const { dispose } = renderLockPicker();
      await settle();

      const modal = target("agentPicker.modal");
      const follows = (first: Element, second: Element) =>
        (first.compareDocumentPosition(second) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0;

      const header = modal.querySelector(".agent-picker-modal-header")!;
      const body = modal.querySelector(".agent-profile-assignment-body")!;
      const lockBar = target("agentPicker.lockBar");
      const defaultSection = target("agentPicker.defaultSection");
      const botonera = modal.querySelector(".agent-picker-botonera")!;

      expect(follows(header, body)).toBe(true);
      expect(follows(body, lockBar)).toBe(true);
      expect(follows(lockBar, defaultSection)).toBe(true);
      expect(follows(defaultSection, botonera)).toBe(true);
      expect(modal.lastElementChild).toBe(botonera);
      expect(botonera.lastElementChild?.classList.contains("agent-picker-bar")).toBe(true);
      // Cancel and Assign exist exactly once; the action bar is the last block.
      expect(document.querySelectorAll('[data-ac-testid="agentPicker.cancel"]')).toHaveLength(1);
      expect(document.querySelectorAll('[data-ac-testid="agentPicker.apply"]')).toHaveLength(1);

      dispose();
    });

    it("leaves the root/origin picker untouched and never calls a lock command", async () => {
      const { dispose } = renderPicker({ scopeContext: undefined });
      await settle();

      expect(maybe("agentPicker.lockBar")).toBeNull();
      expect(maybe("agentPicker.scope.lock.replica")).toBeNull();
      expect(maybe("agentPicker.defaultSection")).toBeNull();
      expect(maybe("agentPicker.conflict")).toBeNull();
      expect(mockSettingsApi.previewSelectionLockRemoval).not.toHaveBeenCalled();
      expect(mockSettingsApi.applySelectionLockRemoval).not.toHaveBeenCalled();
      expect(mockSettingsApi.getReplicaSelectionDefault).not.toHaveBeenCalled();
      expect(mockSettingsApi.setReplicaSelectionDefault).not.toHaveBeenCalled();
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(false);

      dispose();
    });
  });

});
