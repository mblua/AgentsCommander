// @vitest-environment jsdom
// The testkit import comes first: it registers the shared ipc mock before anything loads ipc.
import {
  WG_REPLICA_PATH,
  WG_SCOPE_CONTEXT,
  clickRadio,
  defaultBackendResolve,
  defaultResult,
  installPickerHooks,
  lockAwareRemovalPreviews,
  maybe,
  mockSettingsApi,
  previewResult,
  removePreview,
  renderPicker,
  resolution,
  scopeAwarePreview,
  settings,
  settle,
  state,
  target,
  text,
} from "./AgentPickerModal.testkit";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clickToSetup, startSettleTimer } from "./AgentPickerModal";
import type {
  AppSettings,
  PreviewCodingAgentProfileSelectionResult,
  PreviewSelectionLockRemovalResult,
  ProfileAssignmentScope,
  ScopeFault,
} from "../../shared/types";
import { sessionsStore } from "../stores/sessions";

describe("AgentPickerModal", () => {
  installPickerHooks();

  describe("picker open runs one preview round", () => {
    function backendTotal(): number {
      return (
        mockSettingsApi.previewCodingAgentProfileSelection.mock.calls.length +
        mockSettingsApi.previewSelectionLockRemoval.mock.calls.length +
        mockSettingsApi.getReplicaSelectionDefault.mock.calls.length
      );
    }

    function renderWgPicker(overrides: Parameters<typeof renderPicker>[0] = {}) {
      return renderPicker({
        agentPath: WG_REPLICA_PATH,
        scopeContext: WG_SCOPE_CONTEXT,
        currentRequestedProfile: "A",
        ...overrides,
      });
    }

    it("issue_2475_duplicate_triggering_open_collapses_to_one_batch", async () => {
      // Fixture B: the snapshot selects agent index 1 and the requested profile
      // differs from the initial "A", so both writes feed the preview effect.
      mockSettingsApi.resolveCodingAgentProfile.mockImplementation(() =>
        Promise.resolve(resolution({ requestedProfile: "C", effectiveProfile: "A" })),
      );
      const { dispose } = renderWgPicker({ currentAgentId: "claude", currentRequestedProfile: "C" });
      await settle();

      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(3);
      expect(backendTotal()).toBe(7);

      dispose();
    });

    it("issue_2475_already_clean_open_keeps_seven_backend_calls", async () => {
      // Fixture A: requested "A" equals the initial selection, one round before and after.
      const { dispose } = renderWgPicker({ currentAgentId: "codex", currentRequestedProfile: "A" });
      await settle();

      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(3);
      expect(backendTotal()).toBe(7);

      dispose();
    });

    async function microtasks(count = 8): Promise<void> {
      // Microtask flushes only: a regression that defers to a timer task stays unobserved.
      for (let index = 0; index < count; index += 1) await Promise.resolve();
    }

    function controlledSettings(): () => void {
      let resolveSettings: (value: AppSettings) => void = () => {};
      mockSettingsApi.get.mockImplementation(
        () => new Promise<AppSettings>((resolve) => { resolveSettings = resolve; }),
      );
      return () => resolveSettings(state.currentSettings);
    }

    it("issue_2475_loading_message_appears_in_the_same_tick", async () => {
      mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
        () => new Promise<PreviewCodingAgentProfileSelectionResult>(() => {}),
      );
      const releaseSettings = controlledSettings();
      const { dispose } = renderWgPicker();
      await microtasks();
      expect(maybe("agentPicker.previewBusy")).toBeNull();
      expect(mockSettingsApi.previewCodingAgentProfileSelection).not.toHaveBeenCalled();

      releaseSettings();
      await microtasks();

      // No timer task ran and no preview promise resolved: the message comes from the open itself.
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(3);
      expect(text("agentPicker.previewBusy")).toBe("Loading targets…");

      dispose();
    });

    it("issue_2475_scope_counts_are_never_absent_longer_than_today", async () => {
      const removeIds = [
        "agentPicker.removeScopeCount.replica",
        "agentPicker.removeScopeCount.kind",
        "agentPicker.removeScopeCount.workgroup",
      ];
      const assignIds = ["agentPicker.scope.kind", "agentPicker.scope.workgroup"];
      const ids = [...removeIds, ...assignIds];
      const samples: Record<string, string[]> = Object.fromEntries(ids.map((id) => [id, []]));
      // One sample per microtask boundary; absence is a recorded value, not a skip.
      const sample = () => {
        for (const id of ids) {
          const element = maybe(id);
          samples[id].push(element ? element.textContent?.replace(/\s+/g, " ").trim() ?? "" : "absent");
        }
      };
      const releaseSettings = controlledSettings();
      const { dispose } = renderWgPicker();
      sample();
      releaseSettings();
      for (let index = 0; index < 12; index += 1) {
        await Promise.resolve();
        sample();
      }

      const finals: Record<string, string> = {
        "agentPicker.removeScopeCount.replica": "0 protected",
        "agentPicker.removeScopeCount.kind": "2 of 3 protected",
        "agentPicker.removeScopeCount.workgroup": "3 of 4 protected",
      };
      for (const id of removeIds) {
        const seen = samples[id];
        // "not counted yet" is the pre-settings placeholder: allowed only in the sample taken before release.
        const phases = seen.map((value, index) =>
          (index === 0 && value === "not counted yet") ? 0 : value === "counting…" ? 1 : value === finals[id] ? 2 : -1,
        );
        expect(seen[1], `${id}: first tick after settings`).toBe("counting…");
        expect(phases, `${id}: ${seen.join(" | ")}`).not.toContain(-1);
        expect(phases, `${id}: ${seen.join(" | ")}`).toEqual([...phases].sort((a, b) => a - b));
        expect(seen[seen.length - 1], id).toBe(finals[id]);
      }
      const assignFinals: Record<string, string> = {
        "agentPicker.scope.kind": "3 replicas",
        "agentPicker.scope.workgroup": "4 replicas",
      };
      for (const id of assignIds) {
        const seen = samples[id];
        const phases = seen.map((value) =>
          value === "absent" ? 0 : value.includes(" 0 replicas") ? 1 : value.includes(assignFinals[id]) ? 2 : -1,
        );
        expect(phases, `${id}: ${seen.join(" | ")}`).not.toContain(-1);
        expect(phases, `${id}: ${seen.join(" | ")}`).toEqual([...phases].sort((a, b) => a - b));
        expect(seen[seen.length - 1], id).toContain(assignFinals[id]);
      }

      dispose();
    });

    it("issue_2475_preview_error_text_is_unchanged", async () => {
      const message = "preview exploded: backend said no";
      mockSettingsApi.previewCodingAgentProfileSelection.mockImplementation(
        (req: { scope: string }) =>
          req.scope === "replica"
            ? Promise.reject(new Error(message))
            : Promise.resolve(previewResult({ scope: req.scope as ProfileAssignmentScope })),
      );
      const { dispose } = renderWgPicker();
      await settle();

      expect(text("agentPicker.previewError")).toBe(message);

      dispose();
    });

    const removeCountIds = [
      "agentPicker.removeScopeCount.replica",
      "agentPicker.removeScopeCount.kind",
      "agentPicker.removeScopeCount.workgroup",
    ];

    it("issue_2556_first_load_says_counting", async () => {
      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        () => new Promise<PreviewSelectionLockRemovalResult>(() => {}),
      );
      const { dispose } = renderWgPicker();
      await microtasks();

      for (const id of removeCountIds) expect(text(id), id).toBe("counting\u2026");
      expect(text("agentPicker.removeNote")).toBe("Counting replicas in this scope\u2026");

      dispose();
    });

    it("issue_2556_before_settings_says_not_counted_yet", async () => {
      controlledSettings();
      const { dispose } = renderWgPicker();

      // Same tick as the render: settings never released, so nothing was counted.
      for (const id of removeCountIds) {
        expect(maybe(id), id).not.toBeNull();
        expect(text(id), id).toBe("not counted yet");
      }
      expect(maybe("agentPicker.removeNote")).not.toBeNull();
      expect(text("agentPicker.removeNote")).toBe("Not counted yet.");

      dispose();
    });

    it("issue_2556_counts_complete_false_text_is_unchanged", async () => {
      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        (req: { scope: ProfileAssignmentScope }) =>
          Promise.resolve(removePreview(req.scope, { countsComplete: false })),
      );
      const { dispose } = renderWgPicker();
      await settle();

      for (const id of removeCountIds) expect(text(id), id).toBe("count unknown");
      expect(text("agentPicker.removeNote")).toBe(
        "Scope totals could not be established here; nothing is offered for removal.",
      );

      dispose();
    });

    it("issue_2556_established_counts_are_unchanged", async () => {
      const { dispose } = renderWgPicker();
      await settle();

      expect(text("agentPicker.removeScopeCount.replica")).toBe("0 protected");
      expect(text("agentPicker.removeScopeCount.kind")).toBe("2 of 3 protected");
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("3 of 4 protected");
      expect(text("agentPicker.removeNote")).toBe(
        "No protected replicas in this scope \u2014 nothing to remove.",
      );
      clickRadio("agentPicker.removeScope.workgroup");
      await settle();
      expect(text("agentPicker.removeNote")).toBe("Keeps Coding Agent + Profile. No restart.");

      dispose();
    });

    it("issue_2556_failed_preview_says_count_failed", async () => {
      const message = "removal preview exploded";
      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        (req: { scope: ProfileAssignmentScope }) =>
          req.scope === "replica"
            ? Promise.reject(new Error(message))
            : Promise.resolve(removePreview(req.scope)),
      );
      const { dispose } = renderWgPicker();
      await settle();

      expect(text("agentPicker.removeScopeCount.replica")).toBe("count failed");
      expect(text("agentPicker.removeNote")).toBe(message);

      dispose();
    });

    it("issue_2556_refresh_keeps_the_established_count", async () => {
      let externalUpdate: (() => void) | null = null;
      mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockImplementation(
        (callback: () => void) => {
          externalUpdate = callback;
          return Promise.resolve(() => {});
        },
      );
      const { dispose } = renderWgPicker();
      await settle();
      clickRadio("agentPicker.removeScope.workgroup");
      await settle();
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("3 of 4 protected");
      expect(text("agentPicker.removeNote")).toBe("Keeps Coding Agent + Profile. No restart.");

      mockSettingsApi.previewSelectionLockRemoval.mockClear();
      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        () => new Promise<PreviewSelectionLockRemovalResult>(() => {}),
      );
      expect(externalUpdate).toBeTruthy();
      externalUpdate!();
      await settle();

      // Busy again, yet the established count and note stay.
      expect(mockSettingsApi.previewSelectionLockRemoval).toHaveBeenCalled();
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("3 of 4 protected");
      expect(text("agentPicker.removeNote")).toBe("Keeps Coding Agent + Profile. No restart.");

      dispose();
    });

    function faultPreview(overrides: Partial<PreviewSelectionLockRemovalResult>): void {
      mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
        (req: { scope: ProfileAssignmentScope }) =>
          Promise.resolve(removePreview(req.scope, overrides)),
      );
    }

    function fault(code: string, replicaName: string): ScopeFault {
      return {
        code: code as ScopeFault["code"],
        replicaName,
        replicaPath: `C:/ws/__agent_${replicaName}`,
      };
    }

    function faultLines(): string[] {
      return Array.from(target("agentPicker.scopeFaults").children).map(
        (child) => child.textContent ?? "",
      );
    }

    it("issue_2572_each_fault_code_has_its_own_line", async () => {
      const rows: Array<[string, string, string]> = [
        ["configUnreadable", "r1", "r1 has no readable config.json. It is not counted."],
        ["configNotJson", "r2", "r2 has an invalid config.json (not valid JSON). It is not counted."],
        ["configNotObject", "r3", "r3 has a config.json that is not a JSON object. It is not counted."],
        ["identityMissing", "r4", "r4 has no identity in its config.json. It is not counted."],
        ["identityMismatch", "r5", "r5 points at another agent (identity mismatch). It is not counted."],
        ["locationInvalid", "r6", "r6 is not in a valid replica location. It is not counted."],
        ["pathUnreadable", "r7", "r7 could not be read from disk. It is not counted."],
        [
          "folderUnreadable",
          "room-9",
          "The folder room-9 could not be read. Replicas inside it are not counted.",
        ],
      ];
      for (const [code, name, expected] of rows) {
        faultPreview({ countsComplete: false, scopeFaults: [fault(code, name)] });
        const { dispose } = renderWgPicker();
        await settle();
        expect(maybe("agentPicker.scopeFaults"), code).not.toBeNull();
        expect(faultLines(), code).toEqual([expected]);
        dispose();
      }
    });

    it("issue_2572_over_three_faults_collapse_into_a_counter", async () => {
      faultPreview({
        countsComplete: false,
        scopeFaults: [
          fault("configUnreadable", "a"),
          fault("identityMissing", "b"),
          fault("pathUnreadable", "c"),
          fault("configNotJson", "d"),
          fault("locationInvalid", "e"),
        ],
      });
      let view = renderWgPicker();
      await settle();
      expect(faultLines()).toEqual([
        "a has no readable config.json. It is not counted.",
        "b has no identity in its config.json. It is not counted.",
        "c could not be read from disk. It is not counted.",
        "+2 more not counted.",
      ]);
      view.dispose();

      faultPreview({
        countsComplete: false,
        scopeFaults: [
          fault("configUnreadable", "a"),
          fault("identityMissing", "b"),
          fault("pathUnreadable", "c"),
        ],
      });
      view = renderWgPicker();
      await settle();
      const lines = faultLines();
      expect(lines).toHaveLength(3);
      expect(lines.filter((line) => line.startsWith("+"))).toEqual([]);
      view.dispose();
    });

    it("issue_2572_the_note_and_the_lines_coexist", async () => {
      faultPreview({ countsComplete: false, scopeFaults: [fault("identityMismatch", "x")] });
      const { dispose } = renderWgPicker();
      await settle();

      expect(text("agentPicker.removeNote")).toBe(
        "Scope totals could not be established here; nothing is offered for removal.",
      );
      expect(text("agentPicker.scopeFaults")).toBe(
        "x points at another agent (identity mismatch). It is not counted.",
      );

      dispose();
    });

    it("issue_2572_a_complete_scope_shows_no_fault_block", async () => {
      faultPreview({ countsComplete: true, scopeFaults: [] });
      let view = renderWgPicker();
      await settle();
      expect(maybe("agentPicker.scopeFaults")).toBeNull();
      view.dispose();

      // Positive control: the same construction plus one fault shows the block.
      faultPreview({ countsComplete: true, scopeFaults: [fault("configUnreadable", "p")] });
      view = renderWgPicker();
      await settle();
      expect(maybe("agentPicker.scopeFaults")).not.toBeNull();
      view.dispose();
    });

    it("issue_2572_an_unknown_code_is_still_named", async () => {
      faultPreview({ countsComplete: false, scopeFaults: [fault("somethingNew", "future")] });
      const { dispose } = renderWgPicker();
      await settle();

      expect(text("agentPicker.scopeFaults")).toBe(
        "future could not be checked. It is not counted.",
      );

      dispose();
    });

    it("issue_2572_a_missing_field_is_an_empty_list", async () => {
      faultPreview({ countsComplete: false });
      let view = renderWgPicker();
      await settle();
      expect(maybe("agentPicker.scopeFaults")).toBeNull();
      expect(text("agentPicker.removeScopeCount.workgroup")).toBe("count unknown");
      expect(text("agentPicker.removeNote")).toBe(
        "Scope totals could not be established here; nothing is offered for removal.",
      );
      view.dispose();

      // Positive control: the same construction plus one fault shows the block.
      faultPreview({ countsComplete: false, scopeFaults: [fault("configUnreadable", "p")] });
      view = renderWgPicker();
      await settle();
      expect(maybe("agentPicker.scopeFaults")).not.toBeNull();
      view.dispose();
    });

    describe("picker timing instrumentation", () => {
      const DOM_MARK = "agentPicker.countsDom";
      const FRAME_MARK = "agentPicker.countsFrame";
      let infoSpy: { mock: { calls: unknown[][] }; mockRestore: () => void };
      let frames: FrameRequestCallback[] = [];
      let nextFrameId = 0;
      const cancelFrame = vi.fn();

      beforeEach(() => {
        infoSpy = vi.spyOn(console, "info").mockImplementation(() => {});
        performance.clearMarks(DOM_MARK);
        performance.clearMarks(FRAME_MARK);
        frames = [];
        nextFrameId = 0;
        cancelFrame.mockReset();
        vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
          frames.push(callback);
          nextFrameId += 1;
          return nextFrameId;
        });
        vi.stubGlobal("cancelAnimationFrame", cancelFrame);
      });

      afterEach(() => {
        vi.unstubAllGlobals();
        infoSpy.mockRestore();
      });

      function drainFrames(): void {
        const pending = frames;
        frames = [];
        for (const callback of pending) callback(performance.now());
      }

      function detail(): string | null {
        return target("agentPicker.overlay").getAttribute("data-ac-detail");
      }

      function lines(command: string): string[] {
        return infoSpy.mock.calls
          .map((call: unknown[]) => call[0])
          .filter(
            (first: unknown): first is string =>
              typeof first === "string" && first.startsWith(`[selection-timing] command=${command} `),
          );
      }

      function markCount(name: string): number {
        return performance.getEntriesByName(name, "mark").length;
      }

      // Every preview call waits until released; release() resolves one call with its default result.
      function deferPreviews() {
        const pending: Array<() => void> = [];
        for (const api of [
          mockSettingsApi.previewCodingAgentProfileSelection,
          mockSettingsApi.previewSelectionLockRemoval,
        ]) {
          const original = api.getMockImplementation()!;
          api.mockImplementation(
            (req: unknown) =>
              new Promise((resolve, reject) => {
                pending.push(() => {
                  (original(req) as Promise<unknown>).then(resolve, reject);
                });
              }),
          );
        }
        return {
          pending,
          release(count = pending.length) {
            for (const run of pending.splice(0, count)) run();
          },
        };
      }

      async function openAndSettleCounts() {
        const previews = deferPreviews();
        const view = renderWgPicker();
        await microtasks();
        expect(previews.pending).toHaveLength(6);
        previews.release();
        await microtasks();
        return { previews, view };
      }

      it("issue_2555_settings_get_interval_and_detail", async () => {
        const releaseSettings = controlledSettings();
        const { dispose } = renderWgPicker();

        expect(lines("picker_open")).toHaveLength(1);
        expect(lines("picker_open")[0]).toMatch(/ click_to_setup_ms=na .* open_event=none click_clock=na$/);
        expect(lines("settings_get")).toHaveLength(0);
        expect(detail()).toMatch(/^clickToSetupMs=na clickClock=na setupToMountMs=\d+\.\d$/);

        releaseSettings();
        await microtasks();

        expect(lines("settings_get")).toHaveLength(1);
        expect(lines("settings_get")[0]).toMatch(/ work_ms=\d+\.\d ok=true$/);
        expect(detail()).toMatch(/^clickToSetupMs=na clickClock=na setupToMountMs=\d+\.\d settingsGetMs=\d+\.\d$/);

        dispose();
      });

      it("issue_2555_counts_dom_then_frame", async () => {
        const previews = deferPreviews();
        const { dispose } = renderWgPicker();
        await microtasks();
        expect(previews.pending).toHaveLength(6);

        previews.release(5);
        await microtasks();
        drainFrames();
        expect(lines("picker_counts")).toHaveLength(0);
        expect(markCount(DOM_MARK)).toBe(0);
        expect(markCount(FRAME_MARK)).toBe(0);
        expect(frames).toHaveLength(0);

        previews.release();
        await microtasks();
        expect(markCount(DOM_MARK)).toBe(1);
        expect(text("agentPicker.removeScopeCount.replica")).toBe("0 protected");
        expect(text("agentPicker.removeScopeCount.kind")).toBe("2 of 3 protected");
        expect(text("agentPicker.removeScopeCount.workgroup")).toBe("3 of 4 protected");
        expect(text("agentPicker.scope.kind")).toContain("3 replicas");
        expect(text("agentPicker.scope.workgroup")).toContain("4 replicas");
        const assignCounts = Array.from(document.querySelectorAll(".agent-scope-count")).map(
          (element) => element.textContent?.replace(/\s+/g, " ").trim(),
        );
        expect(assignCounts).toEqual([
          "1 replica", "3 replicas", "4 replicas", // ordinary replica / kind / workgroup
          "1 replica", "3 replicas", "4 replicas", // + lock replica / kind / workgroup
        ]);
        expect(lines("picker_counts")).toHaveLength(0);
        expect(frames).toHaveLength(1);

        drainFrames();
        const counts = lines("picker_counts");
        expect(counts).toHaveLength(1);
        const match = counts[0].match(
          /^\[selection-timing\] command=picker_counts dom_ms=(\d+\.\d) frame_ms=(\d+\.\d) ok=true$/,
        );
        expect(match).not.toBeNull();
        expect(Number(match![2])).toBeGreaterThanOrEqual(Number(match![1]));
        expect(markCount(FRAME_MARK)).toBe(1);
        expect(detail()).toMatch(/ countsDomMs=\d+\.\d countsFrameMs=\d+\.\d countsOk=true$/);

        dispose();
      });

      it("issue_2555_counts_reports_failed_preview", async () => {
        const message = "removal preview exploded";
        mockSettingsApi.previewSelectionLockRemoval.mockImplementation(
          (req: { scope: ProfileAssignmentScope }) =>
            req.scope === "replica"
              ? Promise.reject(new Error(message))
              : Promise.resolve(removePreview(req.scope)),
        );
        const { dispose } = renderWgPicker();
        await settle();
        drainFrames();

        const counts = lines("picker_counts");
        expect(counts).toHaveLength(1);
        expect(counts[0]).toMatch(/ ok=false$/);
        expect(detail()).toMatch(/ countsOk=false$/);
        expect(text("agentPicker.removeScopeCount.replica")).toBe("count failed");
        expect(text("agentPicker.removeNote")).toBe(message);

        dispose();
      });

      it("issue_2555_counts_mark_once_per_open", async () => {
        const { previews, view } = await openAndSettleCounts();
        drainFrames();
        expect(lines("picker_counts")).toHaveLength(1);

        target<HTMLButtonElement>("agentPicker.profile.B").click();
        await microtasks();
        previews.release();
        await microtasks();
        previews.release();
        await microtasks();
        drainFrames();

        expect(lines("picker_counts")).toHaveLength(1);
        expect(markCount(DOM_MARK)).toBe(1);
        expect(markCount(FRAME_MARK)).toBe(1);
        expect(frames).toHaveLength(0);

        view.dispose();
      });

      it("issue_2555_unmount_before_frame", async () => {
        const { view } = await openAndSettleCounts();
        expect(frames).toHaveLength(1);

        view.dispose();
        expect(cancelFrame).toHaveBeenCalledTimes(1);
        expect(cancelFrame).toHaveBeenCalledWith(nextFrameId);

        expect(() => drainFrames()).not.toThrow();
        expect(lines("picker_counts")).toHaveLength(0);
        expect(markCount(FRAME_MARK)).toBe(0);
      });

      it("issue_2555_non_wg_logs_open_and_settings_only", async () => {
        const { dispose } = renderPicker();
        await settle();
        drainFrames();

        expect(lines("picker_open")).toHaveLength(1);
        expect(lines("settings_get")).toHaveLength(1);
        expect(lines("picker_counts")).toHaveLength(0);
        expect(frames).toHaveLength(0);

        dispose();
      });

      it("issue_2555_click_boundary", async () => {
        const button = document.createElement("button");
        document.body.append(button);
        let dispose: (() => void) | undefined;
        button.addEventListener("click", () => {
          dispose = renderWgPicker().dispose;
        });
        button.dispatchEvent(new MouseEvent("click", { bubbles: true }));

        expect(lines("picker_open")).toHaveLength(1);
        expect(lines("picker_open")[0]).toMatch(
          / click_to_setup_ms=(\d+\.\d|na) .* open_event=click click_clock=epoch$/,
        );

        dispose?.();
      });

      it("issue_2555_settle_timer_boundaries", () => {
        const now = vi.spyOn(performance, "now").mockReturnValueOnce(10).mockReturnValueOnce(35.25);
        const onSettle = vi.fn();
        const done = startSettleTimer(onSettle);
        expect(onSettle).not.toHaveBeenCalled();
        expect(now).toHaveBeenCalledTimes(1);
        done(true);
        expect(onSettle).toHaveBeenCalledTimes(1);
        expect(onSettle).toHaveBeenCalledWith(25.25, true);
        expect(now).toHaveBeenCalledTimes(2);

        now.mockReset();
        now.mockReturnValueOnce(100).mockReturnValueOnce(142);
        const failed = vi.fn();
        const doneFailed = startSettleTimer(failed);
        doneFailed(false);
        expect(failed).toHaveBeenCalledTimes(1);
        expect(failed).toHaveBeenCalledWith(42, false);
        now.mockRestore();
      });

      it("issue_2555_click_clock_normalization", () => {
        const origin = 1_700_000_000_000;
        expect(clickToSetup(130, 100, origin)).toEqual({ ms: 30, clock: "perf" });
        expect(clickToSetup(130, origin + 100, origin)).toEqual({ ms: 30, clock: "epoch" });
        expect(clickToSetup(99.5, origin + 100, origin)).toEqual({ ms: null, clock: "epoch" });
        expect(clickToSetup(50, 60, origin)).toEqual({ ms: null, clock: "perf" });
        expect(clickToSetup(130, undefined, origin)).toEqual({ ms: null, clock: "na" });
        expect(clickToSetup(130, 0, origin)).toEqual({ ms: null, clock: "na" });
        expect(clickToSetup(130, NaN, origin)).toEqual({ ms: null, clock: "na" });
      });
    });
  });
});


// Weekly quota remaining next to the agent name in the provider list.
describe("AgentPickerModal weekly quota remaining", () => {
  beforeEach(() => {
    state.currentSettings = settings();
    mockSettingsApi.get.mockResolvedValue(state.currentSettings);
    mockSettingsApi.resolveCodingAgentProfile.mockImplementation(defaultBackendResolve);
    scopeAwarePreview();
    lockAwareRemovalPreviews();
    mockSettingsApi.getReplicaSelectionDefault.mockImplementation(() => Promise.resolve(defaultResult()));
    mockSettingsApi.onCodingAgentProfileSelectionUpdated.mockImplementation(() => Promise.resolve(() => {}));
    mockSettingsApi.onCodingAgentSettingsUpdated.mockImplementation(() => Promise.resolve(() => {}));
  });

  afterEach(() => {
    sessionsStore.resetQuotaReadingsForTests();
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  const cardOrder = () =>
    Array.from(document.querySelectorAll("[data-ac-agent-id]")).map((el) => el.getAttribute("data-ac-agent-id"));

  it("shows_the_label_only_without_a_reading", async () => {
    const { dispose } = renderPicker();
    await settle();

    const card = target("agentPicker.provider.claude");
    expect(card.querySelector(".agent-profile-provider-name")?.textContent).toBe("Claude Code");
    expect(card.textContent?.startsWith("Claude Code")).toBe(true);
    expect(card.textContent).not.toContain("% left");
    expect(card.querySelector(".agent-quota-remaining")).toBeNull();

    dispose();
  });

  it("shows_remaining_for_the_agent_with_a_reading_and_updates_live", async () => {
    const { dispose } = renderPicker();
    await settle();
    const orderBefore = cardOrder();

    sessionsStore.setAgentQuota("claude", 28);
    const claude = target("agentPicker.provider.claude");
    const codex = target("agentPicker.provider.codex");
    expect(claude.textContent).toContain("72% left");
    expect(codex.querySelector(".agent-quota-remaining")).toBeNull();

    const badge = claude.querySelector(".agent-quota-remaining")!;
    const name = claude.querySelector(".agent-profile-provider-name")!;
    expect(badge.parentElement).toBe(name.parentElement);
    expect(badge.parentElement?.classList.contains("agent-quota-name-line")).toBe(true);
    expect(badge.parentElement?.querySelector(".agent-profile-provider-command")).toBeNull();

    sessionsStore.setAgentQuota("claude", 40);
    expect(target("agentPicker.provider.claude").textContent).toContain("60% left");
    sessionsStore.setAgentQuota("claude", null);
    expect(target("agentPicker.provider.claude").querySelector(".agent-quota-remaining")).toBeNull();

    expect(cardOrder()).toEqual(orderBefore);
    dispose();
  });
});
