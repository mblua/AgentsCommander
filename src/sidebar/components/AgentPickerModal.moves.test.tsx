// @vitest-environment jsdom
// The testkit import comes first: it registers the shared ipc mock before anything loads ipc.
import {
  REPO_PATH,
  WG_REPLICA_PATH,
  WG_SCOPE_CONTEXT,
  agent,
  installPickerHooks,
  maybe,
  mockSettingsApi,
  renderPicker,
  settings,
  settle,
  state,
  target,
  text,
} from "./AgentPickerModal.testkit";
import { describe, expect, it, vi } from "vitest";
import type { AgentConfig, AppSettings } from "../../shared/types";
import { dispatchPointer, stubAgentRowGeometry } from "./settings/agentReorderDnd.testkit";

describe("AgentPickerModal", () => {
  installPickerHooks();

  describe("launch lines and left-only filter", () => {
    async function setAgentFilter(value: string): Promise<void> {
      const input = target<HTMLInputElement>("agentPicker.agentFilter");
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await settle();
    }

    function renderWgPicker(overrides: Parameters<typeof renderPicker>[0] = {}) {
      return renderPicker({
        agentPath: WG_REPLICA_PATH,
        scopeContext: WG_SCOPE_CONTEXT,
        currentRequestedProfile: "A",
        ...overrides,
      });
    }

    it("L1 shows each agent's full launch line instead of configured peer", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      expect(text("agentPicker.comparison.row.codex.launchLine")).toBe("codex codex --model gpt-5");
      expect(text("agentPicker.comparison.row.claude.launchLine")).toBe(
        "claude claude --dangerously-skip-permissions",
      );
      const comparison = text("agentPicker.comparison");
      expect(comparison).not.toContain("configured peer");
      expect(comparison).not.toContain("selected coding agent");

      dispose();
    });

    it("L2 launch line follows the effective profile after fallback", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      target<HTMLButtonElement>("agentPicker.profile.C").click();
      await settle();

      // codex C -> B (B is the last enabled cell below C); claude C -> B -> A.
      expect(text("agentPicker.comparison.row.codex.launchLine")).toBe("codex codex --profile fast");
      expect(text("agentPicker.comparison.row.claude.launchLine")).toBe(
        "claude claude --dangerously-skip-permissions",
      );
      expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-profile-status")).toBe(
        "fallback",
      );
      expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-profile-status")).toBe(
        "fallback",
      );

      dispose();
    });

    it("L3 launch line ignores a disabled cell and mirrors the backend fallback case", async () => {
      const base = settings();
      state.currentSettings = settings({
        codingAgentProfiles: {
          ...base.codingAgentProfiles,
          profileSlots: { ...base.codingAgentProfiles.profileSlots, D: { label: "" } },
          profilesByAgent: {
            ...base.codingAgentProfiles.profilesByAgent,
            codex: {
              A: { enabled: true, command: "codex --a", env: {}, notes: "" },
              C: { enabled: true, command: "codex --c", env: {}, notes: "" },
              D: { enabled: false, command: "codex --d", env: {}, notes: "" },
            },
          },
        },
      });
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      target<HTMLButtonElement>("agentPicker.profile.D").click();
      await settle();

      // The disabled D is skipped by resolution, never composed into the line.
      expect(text("agentPicker.comparison.row.codex.launchLine")).toBe("codex codex --c");

      dispose();
    });

    it("L4 launch line drops a disabled profile A to the bare command", async () => {
      const base = settings();
      state.currentSettings = settings({
        codingAgentProfiles: {
          ...base.codingAgentProfiles,
          profilesByAgent: {
            ...base.codingAgentProfiles.profilesByAgent,
            codex: { A: { enabled: false, command: "codex --a", env: {}, notes: "" } },
          },
        },
      });
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      // Resolution always stops at A, so only `enabled` can hide `--a`.
      expect(target("agentPicker.profile.A").getAttribute("data-ac-state")).toBe("active");
      expect(text("agentPicker.comparison.row.codex.launchLine")).toBe("codex");

      dispose();
    });

    it("F1 filters only the left list by name, executable, argument and case", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      const present = (id: string) => maybe(`agentPicker.provider.${id}`) !== null;

      await setAgentFilter("CLAUDE");
      expect(present("claude")).toBe(true);
      expect(present("codex")).toBe(false);

      // Case-folding kill: only the lowercased label carries "claude code".
      await setAgentFilter("CLAUDE CODE");
      expect(present("claude")).toBe(true);
      expect(present("codex")).toBe(false);

      await setAgentFilter("codex");
      expect(present("claude")).toBe(false);
      expect(present("codex")).toBe(true);

      // Argument-only match: "--model" is in the line, never in the label.
      await setAgentFilter("--model");
      expect(present("claude")).toBe(false);
      expect(present("codex")).toBe(true);

      await setAgentFilter("Dangerously");
      expect(present("claude")).toBe(true);
      expect(present("codex")).toBe(false);

      dispose();
    });

    it("F2 right panel is byte-identical under every filter query, including zero matches", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      const before = target("agentPicker.comparison").outerHTML;
      for (const query of ["claude", "--model"]) {
        await setAgentFilter(query);
        expect(target("agentPicker.comparison").outerHTML).toBe(before);
      }

      await setAgentFilter("zzz-no-match");
      expect(target("agentPicker.comparison").outerHTML).toBe(before);
      expect(maybe("agentPicker.provider.codex")).toBeNull();
      expect(maybe("agentPicker.provider.claude")).toBeNull();
      expect(text("agentPicker.agentFilterStatus")).toBe(
        'No coding agent matches "zzz-no-match". Clear the filter to see all 2.',
      );

      await setAgentFilter("");
      expect(target("agentPicker.comparison").outerHTML).toBe(before);
      expect(maybe("agentPicker.provider.codex")).toBeTruthy();
      expect(maybe("agentPicker.provider.claude")).toBeTruthy();
      expect(text("agentPicker.agentFilterStatus")).toBe("2 agents");

      dispose();
    });

    it("F2 filter status counts partial matches and returns to the total when cleared", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      await setAgentFilter("claude");
      expect(text("agentPicker.agentFilterStatus")).toBe('1 of 2 agents match "claude".');

      await setAgentFilter("");
      expect(text("agentPicker.agentFilterStatus")).toBe("2 agents");

      dispose();
    });

    it("F3 filtering never changes selection, profile, radios or buttons", async () => {
      const { dispose, onSelect } = renderWgPicker();
      await settle();

      target<HTMLButtonElement>("agentPicker.provider.claude").click();
      await settle();
      target<HTMLButtonElement>("agentPicker.profile.B").click();
      await settle();

      const previewCalls = mockSettingsApi.previewCodingAgentProfileSelection.mock.calls.length;
      const applyCalls = mockSettingsApi.applyCodingAgentProfileSelection.mock.calls.length;
      const applyDisabled = target<HTMLButtonElement>("agentPicker.apply").disabled;
      const stateSnapshot = () => {
        const map = new Map<string, string | null>();
        document.querySelectorAll<HTMLElement>('[data-ac-testid^="agentPicker."]').forEach((element) => {
          const id = element.getAttribute("data-ac-testid")!;
          if (id.startsWith("agentPicker.provider.") || id.startsWith("agentPicker.agentFilter")) return;
          map.set(id, element.getAttribute("data-ac-state"));
        });
        return map;
      };
      const before = stateSnapshot();

      await setAgentFilter("codex");
      expect(maybe("agentPicker.provider.claude")).toBeNull();
      expect(target("agentPicker.provider.codex").getAttribute("data-ac-state")).not.toBe("active");
      expect(target("agentPicker.provider.codex").getAttribute("aria-pressed")).toBe("false");
      expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).toBe("active");

      await setAgentFilter("");
      expect(stateSnapshot()).toEqual(before);
      expect(target<HTMLButtonElement>("agentPicker.apply").disabled).toBe(applyDisabled);
      expect(target("agentPicker.provider.claude").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.provider.claude").getAttribute("aria-pressed")).toBe("true");
      expect(mockSettingsApi.previewCodingAgentProfileSelection).toHaveBeenCalledTimes(previewCalls);
      expect(mockSettingsApi.applyCodingAgentProfileSelection).toHaveBeenCalledTimes(applyCalls);
      expect(onSelect).not.toHaveBeenCalled();

      dispose();
    });

    it("F3b clicking a card while filtered selects that card", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      // Sorted order is Claude Code (0), Codex (1): an index taken from the
      // filtered list would select claude instead of the clicked codex card.
      await setAgentFilter("codex");
      target<HTMLButtonElement>("agentPicker.provider.codex").click();
      await settle();

      expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.comparison.row.claude").getAttribute("data-ac-state")).not.toBe("active");
      expect(target("agentPicker.provider.codex").getAttribute("aria-pressed")).toBe("true");

      dispose();
    });

    it("F4 keys typed in the filter do not move profile or selection", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      const input = target<HTMLInputElement>("agentPicker.agentFilter");
      input.focus();
      for (const key of ["ArrowRight", "ArrowLeft", "ArrowDown", "Enter"]) {
        input.dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true }));
        await settle();
        // Checked after EVERY key: ArrowRight then ArrowLeft would otherwise
        // cancel each other out and hide a moved profile.
        expect(target("agentPicker.profile.A").getAttribute("data-ac-state")).toBe("active");
        expect(target("agentPicker.provider.codex").getAttribute("data-ac-state")).toBe("active");
        expect(target("agentPicker.comparison.row.codex").getAttribute("data-ac-state")).toBe("active");
      }
      expect(mockSettingsApi.applyCodingAgentProfileSelection).not.toHaveBeenCalled();

      dispose();
    });

    it("F5 filter is labelled and placed right before the first card", async () => {
      const { dispose } = renderPicker({ agentPath: REPO_PATH });
      await settle();

      const input = target<HTMLInputElement>("agentPicker.agentFilter");
      expect(input.labels && input.labels[0]?.textContent).toBe("Filter by name or start line");
      const wrapper = input.closest(".agent-profile-provider-filter");
      expect(wrapper?.nextElementSibling).toBe(target("agentPicker.providers"));

      dispose();
    });
  });

  describe("P3 coding-agent moves", () => {
    const FIVE: AgentConfig[] = ["a", "b", "c", "d", "e"].map((id, index) =>
      agent({ id, label: `Agent ${id.toUpperCase()}`, command: `cmd-${id}`, order: index }),
    );
    const byId = (id: string): AgentConfig => FIVE.find((candidate) => candidate.id === id)!;
    const orderOf = (ids: string[]): AgentConfig[] => ids.map((id) => byId(id));

    const orderedSnapshot = (
      agents: AgentConfig[],
      overlayOwnsAgents = false,
      overrides: Partial<AppSettings> = {},
    ): AppSettings =>
      ({
        ...settings({ ...overrides, agents }),
        overlayOwnsAgents,
      }) as unknown as AppSettings;

    const cardIds = (): string[] =>
      [
        ...target("agentPicker.providers").querySelectorAll<HTMLElement>(
          ".agent-profile-provider-card",
        ),
      ].map((card) => card.getAttribute("data-ac-agent-id") ?? "");

    const comparisonIds = (): string[] =>
      [
        ...target("agentPicker.comparison").querySelectorAll<HTMLElement>("[data-ac-agent-id]"),
      ].map((row) => row.getAttribute("data-ac-agent-id") ?? "");

    const setFilter = async (value: string): Promise<void> => {
      const input = target<HTMLInputElement>("agentPicker.agentFilter");
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await settle();
    };

    const installReorder = (): void => {
      mockSettingsApi.reorderCodingAgent.mockImplementation(
        async (request: { id: string; expectedIds: string[]; targetIndex: number }) => {
          const ids = [...request.expectedIds];
          ids.splice(request.targetIndex, 0, ids.splice(ids.indexOf(request.id), 1)[0]!);
          state.currentSettings = orderedSnapshot(orderOf(ids));
          mockSettingsApi.get.mockResolvedValue(state.currentSettings);
          return ids;
        },
      );
    };

    const fireSettingsUpdate = (): (() => void) => {
      let fire!: () => void;
      mockSettingsApi.onCodingAgentSettingsUpdated.mockImplementation((cb: () => void) => {
        fire = cb;
        return Promise.resolve(() => {});
      });
      return () => fire();
    };

    it("renders backend vector order and selects currentAgentId without an alphabetical remap", async () => {
      state.currentSettings = orderedSnapshot([byId("b"), byId("a")]);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "a" });
      await settle();

      expect(cardIds()).toEqual(["b", "a"]);
      expect(target("agentPicker.provider.a").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.provider.b").getAttribute("data-ac-state")).toBe("inactive");
      expect(comparisonIds()).toEqual(["b", "a"]);

      dispose();
    });

    const grip = (id: string): HTMLButtonElement =>
      target<HTMLButtonElement>(`agentPicker.provider.${id}.dragHandle`);
    const allGrips = (): HTMLButtonElement[] =>
      [
        ...document.querySelectorAll<HTMLButtonElement>(".agent-profile-provider-drag-handle"),
      ].filter((node) => node.hasAttribute("data-ac-testid"));

    const altKey = (id: string, key: "ArrowUp" | "ArrowDown"): KeyboardEvent => {
      const event = new KeyboardEvent("keydown", { key, altKey: true, bubbles: true, cancelable: true });
      grip(id).dispatchEvent(event);
      return event;
    };

    /** Stubs geometry (40 px rows) and presses the grip of `id` at its row middle. */
    const pressGrip = (id: string): { handle: HTMLButtonElement; y: number } => {
      const rows = [
        ...target("agentPicker.providers").querySelectorAll<HTMLElement>(
          ".agent-profile-provider-card-wrap",
        ),
      ];
      const handle = grip(id);
      stubAgentRowGeometry(rows, handle);
      const index = rows.findIndex((row) => row.contains(handle));
      const y = index * 40 + 20;
      dispatchPointer(handle, "pointerdown", 5, y);
      return { handle, y };
    };

    const expectNoMoveCommand = (): void => {
      expect(mockSettingsApi.moveCodingAgent).not.toHaveBeenCalled();
    };

    it("pointer drag reorders an agent through reorderCodingAgent", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      installReorder();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      const { handle } = pressGrip("c");
      dispatchPointer(handle, "pointermove", 5, 70);
      dispatchPointer(handle, "pointermove", 5, 50);
      expect(document.body.querySelector(".drag-ghost")).not.toBeNull();
      expect(document.body.classList.contains("is-dragging")).toBe(true);
      expect(maybe("agentPicker.providers.dropIndicator")).not.toBeNull();
      expect(target("agentPicker.providerWrap.c").classList.contains("is-drag-source")).toBe(true);
      dispatchPointer(handle, "pointerup", 5, 50);
      await settle();

      expect(mockSettingsApi.reorderCodingAgent).toHaveBeenCalledTimes(1);
      expect(mockSettingsApi.reorderCodingAgent).toHaveBeenCalledWith({
        id: "c",
        expectedIds: ["a", "b", "c", "d", "e"],
        targetIndex: 1,
      });
      expect(cardIds()).toEqual(["a", "c", "b", "d", "e"]);
      expect(text("agentPicker.moveStatus")).toBe("Moved Agent C to position 2 of 5.");
      expect(document.body.querySelector(".drag-ghost")).toBeNull();
      expect(maybe("agentPicker.providers.dropIndicator")).toBeNull();
      expectNoMoveCommand();

      dispose();
    });

    it("pointer drag downward sends the post-removal target index", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      installReorder();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      // A (row 0) dropped below C: y=110 is past C's midpoint (100) and above D's (140).
      const { handle } = pressGrip("a");
      dispatchPointer(handle, "pointermove", 5, 60);
      dispatchPointer(handle, "pointermove", 5, 110);
      dispatchPointer(handle, "pointerup", 5, 110);
      await settle();

      expect(mockSettingsApi.reorderCodingAgent).toHaveBeenCalledTimes(1);
      expect(mockSettingsApi.reorderCodingAgent).toHaveBeenCalledWith({
        id: "a",
        expectedIds: ["a", "b", "c", "d", "e"],
        targetIndex: 2,
      });
      expect(cardIds()).toEqual(["b", "c", "a", "d", "e"]);
      expect(text("agentPicker.moveStatus")).toBe("Moved Agent A to position 3 of 5.");
      expectNoMoveCommand();

      dispose();
    });

    it("Alt+Arrow on a grip moves one step and announces first and last", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      installReorder();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      altKey("c", "ArrowDown");
      await settle();
      expect(mockSettingsApi.reorderCodingAgent).toHaveBeenCalledWith({
        id: "c",
        expectedIds: ["a", "b", "c", "d", "e"],
        targetIndex: 3,
      });
      expect(cardIds()).toEqual(["a", "b", "d", "c", "e"]);
      expect(text("agentPicker.moveStatus")).toBe("Moved Agent C to position 4 of 5.");

      altKey("a", "ArrowUp");
      await settle();
      expect(text("agentPicker.moveStatus")).toBe("Agent A is already first.");
      altKey("e", "ArrowDown");
      await settle();
      expect(text("agentPicker.moveStatus")).toBe("Agent E is already last.");
      expect(mockSettingsApi.reorderCodingAgent).toHaveBeenCalledTimes(1);
      expectNoMoveCommand();

      dispose();
    });

    it("Alt+Arrow on a grip does not move the highlighted card", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      const event = altKey("a", "ArrowUp");
      await settle();

      expect(event.defaultPrevented).toBe(true);
      expect(target("agentPicker.provider.c").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.provider.b").getAttribute("data-ac-state")).toBe("inactive");
      expect(mockSettingsApi.reorderCodingAgent).not.toHaveBeenCalled();
      expectNoMoveCommand();

      dispose();
    });

    it("filter text disables every grip", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      expect(allGrips().every((node) => !node.disabled)).toBe(true);
      expect(grip("c").getAttribute("aria-label")).toBe("Reorder Agent C, position 3 of 5");
      expect(grip("c").getAttribute("title")).toBe("Drag to reorder (Alt+Up / Alt+Down)");

      await setFilter("Agent B");
      expect(cardIds()).toEqual(["b"]);
      expect(allGrips()).toHaveLength(1);
      for (const node of allGrips()) {
        expect(node.disabled).toBe(true);
        expect(node.getAttribute("aria-label")).toMatch(/Clear the filter to reorder\.$/);
        expect(node.getAttribute("title")).toBe("Clear the filter to reorder.");
      }
      altKey("b", "ArrowDown");
      dispatchPointer(grip("b"), "pointerdown", 5, 20);
      await settle();
      expect(mockSettingsApi.reorderCodingAgent).not.toHaveBeenCalled();

      await setFilter("");
      expect(allGrips().every((node) => !node.disabled)).toBe(true);
      expectNoMoveCommand();

      dispose();
    });

    it("grips stay disabled while a reorder is in flight", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      let resolveReorder!: (ids: string[]) => void;
      mockSettingsApi.reorderCodingAgent.mockImplementation(
        () => new Promise<string[]>((resolve) => { resolveReorder = resolve; }),
      );
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      altKey("c", "ArrowUp");
      await settle();
      expect(mockSettingsApi.reorderCodingAgent).toHaveBeenCalledTimes(1);
      expect(allGrips()).toHaveLength(5);
      expect(allGrips().every((node) => node.disabled)).toBe(true);

      altKey("d", "ArrowUp");
      const { handle } = pressGrip("e");
      dispatchPointer(handle, "pointermove", 5, 10);
      dispatchPointer(handle, "pointerup", 5, 10);
      await settle();
      expect(mockSettingsApi.reorderCodingAgent).toHaveBeenCalledTimes(1);

      const ids = ["a", "c", "b", "d", "e"];
      state.currentSettings = orderedSnapshot(orderOf(ids));
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      resolveReorder(ids);
      await settle();

      expect(cardIds()).toEqual(ids);
      expect(allGrips().every((node) => !node.disabled)).toBe(true);
      expectNoMoveCommand();

      dispose();
    });

    it("overlay ownership disables every grip", async () => {
      state.currentSettings = orderedSnapshot([byId("d"), byId("b"), byId("a")], true);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "b" });
      await settle();

      expect(cardIds()).toEqual(["d", "b", "a"]);
      expect(allGrips()).toHaveLength(3);
      for (const node of allGrips()) {
        expect(node.disabled).toBe(true);
        expect(node.getAttribute("aria-label")).toContain("local settings overlay");
        expect(node.getAttribute("title")).toContain("local settings overlay");
      }
      expect(text("agentPicker.overlayReason")).toContain("local settings overlay");

      altKey("b", "ArrowUp");
      dispatchPointer(grip("b"), "pointerdown", 5, 60);
      await settle();
      expect(mockSettingsApi.reorderCodingAgent).not.toHaveBeenCalled();
      expectNoMoveCommand();

      dispose();
    });

    it("command rejection refetches and shows the move error", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      mockSettingsApi.reorderCodingAgent.mockRejectedValue(new Error("settings lock busy"));
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();
      const fetchesBefore = mockSettingsApi.get.mock.calls.length;

      altKey("c", "ArrowUp");
      await settle();

      expect(mockSettingsApi.reorderCodingAgent).toHaveBeenCalledTimes(1);
      expect(mockSettingsApi.get.mock.calls).toHaveLength(fetchesBefore + 1);
      expect(cardIds()).toEqual(["a", "b", "c", "d", "e"]);
      const error = target("agentPicker.moveError");
      expect(error.getAttribute("aria-live")).toBe("polite");
      expect(error.textContent).toContain("settings lock busy");
      expect(text("agentPicker.moveStatus")).toBe("");
      expectNoMoveCommand();

      dispose();
    });

    it("unexpected returned order is reported and the refetch wins", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      mockSettingsApi.reorderCodingAgent.mockResolvedValue(["a", "b", "c", "d", "e"]);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      altKey("c", "ArrowUp");
      await settle();

      expect(cardIds()).toEqual(["a", "b", "c", "d", "e"]);
      expect(text("agentPicker.moveError")).toContain("unexpected agent order");
      expectNoMoveCommand();

      dispose();
    });

    it("refetch failure after a reorder keeps the last order", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValueOnce(state.currentSettings);
      mockSettingsApi.get.mockImplementation(() => Promise.reject(new Error("offline")));
      mockSettingsApi.reorderCodingAgent.mockResolvedValue(["a", "c", "b", "d", "e"]);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      altKey("c", "ArrowUp");
      await settle();

      expect(cardIds()).toEqual(["a", "b", "c", "d", "e"]);
      expect(text("agentPicker.moveError")).toContain("offline");
      expectNoMoveCommand();

      dispose();
    });

    it("keyboard move keeps focus on the moved grip", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      installReorder();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      grip("c").focus();
      altKey("c", "ArrowUp");
      await settle();

      expect(cardIds()).toEqual(["a", "c", "b", "d", "e"]);
      expect(document.activeElement).toBe(grip("c"));
      expectNoMoveCommand();

      dispose();
    });

    it("Escape mid-drag cancels without closing the modal", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose, onClose } = renderPicker({ currentAgentId: "c" });
      await settle();

      const { handle } = pressGrip("c");
      dispatchPointer(handle, "pointermove", 5, 50);
      expect(document.body.querySelector(".drag-ghost")).not.toBeNull();
      handle.dispatchEvent(
        new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }),
      );
      dispatchPointer(handle, "pointerup", 5, 50);
      await settle();

      expect(mockSettingsApi.reorderCodingAgent).not.toHaveBeenCalled();
      expect(text("agentPicker.moveStatus")).toBe("Move cancelled.");
      expect(document.body.querySelector(".drag-ghost")).toBeNull();
      expect(document.body.classList.contains("is-dragging")).toBe(false);
      expect(onClose).not.toHaveBeenCalled();
      expectNoMoveCommand();

      dispose();
    });

    it("drop at the source slot or under the threshold sends nothing", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      // Source slot: a started drag that ends over its own row.
      const first = pressGrip("c");
      dispatchPointer(first.handle, "pointermove", 5, first.y + 5);
      expect(document.body.querySelector(".drag-ghost")).not.toBeNull();
      expect(maybe("agentPicker.providers.dropIndicator")).toBeNull();
      dispatchPointer(first.handle, "pointerup", 5, first.y + 5);
      await settle();

      // Under the threshold: the drag never starts, even over another row's slot.
      const second = pressGrip("c");
      dispatchPointer(second.handle, "pointermove", 5, second.y - 2);
      expect(document.body.querySelector(".drag-ghost")).toBeNull();
      dispatchPointer(second.handle, "pointerup", 5, second.y - 2);
      await settle();

      expect(mockSettingsApi.reorderCodingAgent).not.toHaveBeenCalled();
      expectNoMoveCommand();

      dispose();
    });

    it("no arrow move buttons remain", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      expect(document.querySelectorAll('[data-ac-testid$=".moveUp"]')).toHaveLength(0);
      expect(document.querySelectorAll('[data-ac-testid$=".moveDown"]')).toHaveLength(0);
      expect(document.querySelectorAll(".agent-profile-provider-moves")).toHaveLength(0);
      expect(document.querySelectorAll("#agentPickerAgentList button button")).toHaveLength(0);
      expect(allGrips()).toHaveLength(5);
      expect(target("agentPicker.provider.c").contains(grip("c"))).toBe(false);
      expectNoMoveCommand();

      dispose();
    });

    it("unmount mid-drag removes the ghost and is-dragging", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      const { handle } = pressGrip("c");
      dispatchPointer(handle, "pointermove", 5, 50);
      expect(document.body.querySelector(".drag-ghost")).not.toBeNull();
      expect(document.body.classList.contains("is-dragging")).toBe(true);

      dispose();

      expect(document.body.querySelector(".drag-ghost")).toBeNull();
      expect(document.body.classList.contains("is-dragging")).toBe(false);
      expect(mockSettingsApi.reorderCodingAgent).not.toHaveBeenCalled();
      expectNoMoveCommand();
    });

    it("each card wrap holds the grip then the card button as siblings", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      const wraps = [
        ...target("agentPicker.providers").querySelectorAll<HTMLElement>(".agent-profile-provider-card-wrap"),
      ];
      expect(wraps).toHaveLength(5);
      for (const wrap of wraps) {
        expect(wrap.children).toHaveLength(2);
        const id = wrap.children[1].getAttribute("data-ac-testid")?.replace(/^agentPicker\.provider\./, "");
        expect(id).toBeTruthy();
        expect(wrap.children[0]).toBe(grip(id as string));
        expect(wrap.children[1]).toBe(target(`agentPicker.provider.${id}`));
        expect(wrap.children[1].contains(wrap.children[0])).toBe(false);
      }

      await setFilter("Agent B");
      const list = document.getElementById("agentPickerAgentList") as HTMLElement;
      const filtered = list.querySelectorAll(".agent-profile-provider-card-wrap");
      expect(filtered).toHaveLength(1);
      expect(list.firstElementChild).toBe(filtered[0]);
      expect(filtered[0].children[1]).toBe(target("agentPicker.provider.b"));
      expectNoMoveCommand();

      dispose();
    });

    it("refetches on coding_agent_settings_updated and keeps a surviving ID at its new position", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const fire = fireSettingsUpdate();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      const reordered = orderOf(["e", "d", "c", "b", "a"]);
      state.currentSettings = orderedSnapshot(reordered);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      fire();
      await settle();

      expect(cardIds()).toEqual(["e", "d", "c", "b", "a"]);
      expect(target("agentPicker.provider.c").getAttribute("data-ac-state")).toBe("active");
      expect(target("agentPicker.provider.c").getAttribute("aria-pressed")).toBe("true");

      dispose();
    });

    it("selects the survivor at the removed tool's old position when it disappears", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const fire = fireSettingsUpdate();
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();

      state.currentSettings = orderedSnapshot(orderOf(["a", "b", "d", "e"]));
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      fire();
      await settle();

      expect(cardIds()).toEqual(["a", "b", "d", "e"]);
      expect(target("agentPicker.provider.d").getAttribute("data-ac-state")).toBe("active");

      dispose();
    });

    it("clamps a disappeared selection to the new final item when the list shrinks past it", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const fire = fireSettingsUpdate();
      const { dispose } = renderPicker({ currentAgentId: "e" });
      await settle();

      state.currentSettings = orderedSnapshot(orderOf(["a", "b", "c"]));
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      fire();
      await settle();

      expect(cardIds()).toEqual(["a", "b", "c"]);
      expect(target("agentPicker.provider.c").getAttribute("data-ac-state")).toBe("active");

      dispose();
    });

    it("keeps a surviving selection across a live add and renders the incoming ID in place", async () => {
      state.currentSettings = orderedSnapshot(orderOf(["a", "b"]));
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const fire = fireSettingsUpdate();
      const { dispose } = renderPicker({ currentAgentId: "b" });
      await settle();

      state.currentSettings = orderedSnapshot([
        byId("a"),
        agent({ id: "x", label: "Agent X", command: "cmd-x", order: 1 }),
        byId("b"),
      ]);
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      fire();
      await settle();

      expect(cardIds()).toEqual(["a", "x", "b"]);
      expect(target("agentPicker.provider.b").getAttribute("data-ac-state")).toBe("active");

      dispose();
    });

    it("coalesces concurrent settings events into one queued refetch and unsubscribes on cleanup", async () => {
      state.currentSettings = orderedSnapshot(FIVE);
      const unlisten = vi.fn();
      let fire!: () => void;
      mockSettingsApi.onCodingAgentSettingsUpdated.mockImplementation((cb: () => void) => {
        fire = cb;
        return Promise.resolve(unlisten);
      });
      let getCalls = 0;
      let releaseDeferred!: () => void;
      mockSettingsApi.get.mockImplementation(() => {
        getCalls += 1;
        if (getCalls === 2) {
          return new Promise<AppSettings>((resolve) => {
            releaseDeferred = () => resolve(state.currentSettings);
          });
        }
        return Promise.resolve(state.currentSettings);
      });
      const { dispose } = renderPicker({ currentAgentId: "c" });
      await settle();
      expect(getCalls).toBe(1);

      fire();
      fire();
      await settle();
      // The first event's fetch is still in flight; the second only queued.
      expect(getCalls).toBe(2);

      releaseDeferred();
      await settle();
      expect(getCalls).toBe(3);

      dispose();
      await settle();
      expect(unlisten).toHaveBeenCalledTimes(1);
    });

    it("keeps the environment-key sort while the tool list stays in vector order", async () => {
      const base = settings();
      state.currentSettings = orderedSnapshot([byId("b"), byId("a")], false, {
        codingAgentProfiles: {
          ...base.codingAgentProfiles,
          profilesByAgent: {
            ...base.codingAgentProfiles.profilesByAgent,
            b: {
              A: {
                enabled: true,
                command: "",
                env: { ZETA: "1", ALPHA: "2" },
                notes: "",
              },
            },
          },
        },
      });
      mockSettingsApi.get.mockResolvedValue(state.currentSettings);
      const { dispose } = renderPicker({ agentPath: REPO_PATH, currentAgentId: "b" });
      await settle();

      expect(cardIds()).toEqual(["b", "a"]);
      const envKeys = [
        ...target("agentPicker.profile.A.env").querySelectorAll<HTMLElement>(
          ".agent-profile-declared-env-key",
        ),
      ].map((node) => node.textContent);
      expect(envKeys).toEqual(["ALPHA", "ZETA"]);

      dispose();
    });
  });
});
