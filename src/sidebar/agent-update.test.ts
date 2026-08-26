// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../shared/testing/fake-transport";
import { __setTransportForTests } from "../shared/ipc";
import { toastStore } from "../shared/stores/toasts";
import type {
  AgentUpdateNode,
  AgentUpdatePrompt,
  AgentUpdateResult,
  AgentUpdateStatus,
  InstallState,
} from "../shared/types";
import {
  agentUpdateInitialState,
  agentUpdateStore,
  dismissAgentUpdateSummary,
  markPromptClosed,
  mergeSnapshot,
  resetAgentUpdateForTests,
  wireAgentUpdateListeners,
} from "./agent-update";

const [store, setAgentUpdateStore] = agentUpdateStore;

const FAILING: AgentUpdateResult = {
  command: "claude",
  label: "Claude",
  ok: false,
  error: "exit code 1",
};

// Frozen: a Solid path write (`set("prompt", next)`) merges `next` INTO the object the store
// already holds, so an unfrozen shared constant would be mutated by the next prompt event.
const PROMPT: AgentUpdatePrompt = Object.freeze({ command: "claude", label: "Claude" });
const CODEX_PROMPT: AgentUpdatePrompt = Object.freeze({ command: "codex", label: "Codex" });

function node(command: string, installBefore?: InstallState): AgentUpdateNode {
  const label = command.charAt(0).toUpperCase() + command.slice(1);
  return installBefore
    ? { command, label, updateCommands: [`${command} update`], installBefore }
    : { command, label, updateCommands: [`${command} update`] };
}

function ok(command: string): AgentUpdateResult {
  return { command, label: command.charAt(0).toUpperCase() + command.slice(1), ok: true };
}

function failed(command: string, error = "exit code 1"): AgentUpdateResult {
  return {
    command,
    label: command.charAt(0).toUpperCase() + command.slice(1),
    ok: false,
    error,
  };
}

function installed(version: string, seq: number): InstallState {
  return { status: "installed", version, path: `C:\\bin\\${version}.cmd`, seq };
}

function status(overrides: Partial<AgentUpdateStatus> = {}): AgentUpdateStatus {
  return { inProgress: false, prompt: null, results: [], running: [], ...overrides };
}

/** Apply a snapshot through the pure merge exactly as the wiring does. */
function applySnapshot(snapshot: AgentUpdateStatus): void {
  setAgentUpdateStore(mergeSnapshot(store, snapshot));
}

/** One macrotask: every pending microtask (the getStatus chain included) has run. */
async function settle(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

describe("wireAgentUpdateListeners (#1327)", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
    resetAgentUpdateForTests();
  });

  afterEach(() => {
    toastStore.clear();
    resetAgentUpdateForTests();
    restore();
    vi.restoreAllMocks();
  });

  it("subscribe-then-snapshot dedups: event + snapshot with the same failing command toast exactly once", async () => {
    fake.resolve("get_agent_update_status", status({ results: [FAILING] }));
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_updates_finished", { results: [FAILING] });
    await settle(); // flush the snapshot promise

    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].kind).toBe("error");
    expect(toastStore.items[0].message).toBe(
      "Auto-update failed for Claude (claude): exit code 1"
    );
  });

  it("agent_updates_started sets inProgress; agent_updates_finished clears it and closes the prompt", async () => {
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_updates_started", null);
    expect(store.inProgress).toBe(true);

    fake.emitFromBackend("agent_update_prompt", PROMPT);
    expect(store.prompt).toEqual(PROMPT);

    fake.emitFromBackend("agent_updates_finished", { results: [] });
    expect(store.inProgress).toBe(false);
    expect(store.prompt).toBeNull();
  });

  it("agent_update_prompt_closed clears the prompt but keeps the splash (F4)", async () => {
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_updates_started", null);
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt_closed", { command: "claude", label: "Claude" });

    expect(store.prompt).toBeNull();
    expect(store.inProgress).toBe(true);
  });

  it("snapshot restores a prompt emitted before wiring (F3) and the inProgress splash", async () => {
    fake.resolve("get_agent_update_status", status({ inProgress: true, prompt: PROMPT }));
    await wireAgentUpdateListeners();
    await settle();

    expect(store.inProgress).toBe(true);
    expect(store.prompt).toEqual(PROMPT);
  });

  it("getStatus failure never breaks the live listeners (F8)", async () => {
    fake.reject("get_agent_update_status", "boom");
    const unlisteners = await wireAgentUpdateListeners();
    expect(unlisteners).toHaveLength(8);

    fake.emitFromBackend("agent_updates_finished", { results: [FAILING] });
    expect(toastStore.items).toHaveLength(1);
  });
});

describe("agent-update store: per-command events and the monotonic snapshot merge (#1551)", () => {
  let fake: FakeTransport;
  let restore: () => void;

  beforeEach(() => {
    fake = new FakeTransport();
    restore = __setTransportForTests(fake);
    toastStore.clear();
    resetAgentUpdateForTests();
  });

  afterEach(() => {
    toastStore.clear();
    resetAgentUpdateForTests();
    restore();
    vi.restoreAllMocks();
  });

  it("command_started/finished maintain running and results", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });

    fake.emitFromBackend("agent_update_command_started", node("claude"));
    fake.emitFromBackend("agent_update_command_started", node("codex"));
    // a duplicate started for a running command is ignored
    fake.emitFromBackend("agent_update_command_started", node("claude"));
    expect(store.running).toEqual([
      { command: "claude", label: "Claude" },
      { command: "codex", label: "Codex" },
    ]);

    fake.emitFromBackend("agent_update_command_finished", ok("claude"));
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
    expect(store.results).toEqual([ok("claude")]);

    fake.emitFromBackend("agent_update_command_finished", failed("codex"));
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([ok("claude"), failed("codex")]);
  });

  it("agent_updates_finished clears running even without command_finished", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    fake.emitFromBackend("agent_update_command_started", node("claude"));
    expect(store.running).toHaveLength(1);

    fake.emitFromBackend("agent_updates_finished", { results: [ok("claude")] });
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([ok("claude")]);
    expect(store.inProgress).toBe(false);
    expect(store.finishedSeen).toBe(true);
  });

  it("snapshot restores running and results", async () => {
    fake.resolve(
      "get_agent_update_status",
      status({
        inProgress: true,
        running: [{ command: "codex", label: "Codex" }],
        results: [ok("claude")],
      })
    );
    await wireAgentUpdateListeners();
    await settle();

    expect(store.inProgress).toBe(true);
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
    expect(store.results).toEqual([ok("claude")]);
    // a mid-pass snapshot toasts nothing (round 6, Grinch R3)
    expect(toastStore.items).toHaveLength(0);
  });

  it("snapshot unions running with newer command_started events", () => {
    setAgentUpdateStore({ inProgress: true, running: [{ command: "codex", label: "Codex" }] });
    // event first: the snapshot (computed before the mark) reports no running command
    applySnapshot(status({ inProgress: true, running: [] }));
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
  });

  it("snapshot adds running commands the store missed", () => {
    setAgentUpdateStore({ inProgress: true, running: [{ command: "codex", label: "Codex" }] });
    applySnapshot(
      status({
        inProgress: true,
        running: [{ command: "pi", label: "Pi" }, { command: "codex", label: "Codex" }],
      })
    );
    expect(store.running).toEqual([
      { command: "codex", label: "Codex" },
      { command: "pi", label: "Pi" },
    ]);
  });

  it("results in either source remove a command from running (both directions)", () => {
    // the store has the result, the snapshot still reports the command running
    setAgentUpdateStore({ inProgress: true, results: [ok("codex")] });
    applySnapshot(status({ inProgress: true, running: [{ command: "codex", label: "Codex" }] }));
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([ok("codex")]);

    // the store still has the command running, the snapshot carries its result
    resetAgentUpdateForTests();
    setAgentUpdateStore({ inProgress: true, running: [{ command: "codex", label: "Codex" }] });
    applySnapshot(status({ inProgress: true, results: [failed("codex")] }));
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([failed("codex")]);
  });

  it("a snapshot after agent_updates_finished never resurrects inProgress, running, or prompt", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });
    fake.emitFromBackend("agent_updates_finished", { results: [ok("claude")] });

    applySnapshot(
      status({
        inProgress: true,
        running: [{ command: "codex", label: "Codex" }],
        prompt: PROMPT,
        results: [ok("claude"), failed("codex")],
      })
    );
    expect(store.inProgress).toBe(false);
    expect(store.running).toEqual([]);
    expect(store.prompt).toBeNull();
    expect(store.results).toEqual([ok("claude"), failed("codex")]);
  });

  it("agent_updates_started resets running, results, finishedSeen, closedPrompts and skippedNodes", async () => {
    await wireAgentUpdateListeners();
    setAgentUpdateStore({
      running: [{ command: "codex", label: "Codex" }],
      results: [ok("claude")],
      finishedSeen: true,
      closedPrompts: ["claude"],
      skippedNodes: ["pi"],
      prompt: CODEX_PROMPT,
    });

    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    expect(store.inProgress).toBe(true);
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([]);
    expect(store.finishedSeen).toBe(false);
    expect(store.closedPrompts).toEqual([]);
    expect(store.skippedNodes).toEqual([]);
    expect(store.nodes).toEqual([node("claude")]);
    // the prompt is untouched by started
    expect(store.prompt).toEqual(CODEX_PROMPT);
  });

  it("a snapshot without running (older backend) merges as empty", () => {
    setAgentUpdateStore({ inProgress: true, running: [{ command: "codex", label: "Codex" }] });
    const older = { inProgress: true, prompt: null, results: [] } as unknown as AgentUpdateStatus;
    applySnapshot(older);
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
    expect(store.nodes).toEqual([]);
  });

  it("prompt_closed carries the command: an older snapshot cannot resurrect that prompt", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt_closed", { command: "claude", label: "Claude" });
    expect(store.prompt).toBeNull();
    expect(store.closedPrompts).toContain("claude");

    applySnapshot(status({ inProgress: true, prompt: PROMPT }));
    expect(store.prompt).toBeNull();
  });

  it("a new prompt after a closure still appears from an event and from a snapshot", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt_closed", { command: "claude", label: "Claude" });

    fake.emitFromBackend("agent_update_prompt", CODEX_PROMPT);
    expect(store.prompt).toEqual(CODEX_PROMPT);

    setAgentUpdateStore("prompt", null);
    applySnapshot(status({ inProgress: true, prompt: CODEX_PROMPT }));
    expect(store.prompt).toEqual(CODEX_PROMPT);
    expect(store.closedPrompts).toEqual(["claude"]);
  });

  it("a null prompt_closed payload (older backend) still clears the prompt", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt_closed", null);
    expect(store.prompt).toBeNull();
    expect(store.closedPrompts).toEqual([]);
  });

  it("a delayed prompt_closed(A) does not clear a newer prompt B", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_update_prompt", PROMPT);
    fake.emitFromBackend("agent_update_prompt", CODEX_PROMPT);
    expect(store.prompt).toEqual(CODEX_PROMPT);

    fake.emitFromBackend("agent_update_prompt_closed", { command: "claude", label: "Claude" });
    expect(store.prompt?.command).toBe("codex");
    expect(store.closedPrompts).toEqual(["claude"]);

    fake.emitFromBackend("agent_update_prompt_closed", { command: "codex", label: "Codex" });
    expect(store.prompt).toBeNull();
    expect(store.closedPrompts).toEqual(["claude", "codex"]);
  });

  it("markPromptClosed protects the prompt the overlay answered itself", () => {
    setAgentUpdateStore({ inProgress: true, prompt: null });
    markPromptClosed("claude");
    markPromptClosed("claude");
    expect(store.closedPrompts).toEqual(["claude"]);

    applySnapshot(status({ inProgress: true, prompt: PROMPT }));
    expect(store.prompt).toBeNull();
  });

  it("agent_updates_started carries the pass nodes in order and resets installAfter and summary", async () => {
    await wireAgentUpdateListeners();
    setAgentUpdateStore({ installAfter: { claude: installed("1.0", 4) }, summary: "shown" });

    fake.emitFromBackend("agent_updates_started", {
      nodes: [node("claude"), node("pi"), node("codex")],
    });
    expect(store.nodes.map((n) => n.command)).toEqual(["claude", "pi", "codex"]);
    expect(store.installAfter).toEqual({});
    expect(store.summary).toBe("none");
  });

  it("a null started payload (older backend) leaves nodes empty", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", null);
    expect(store.inProgress).toBe(true);
    expect(store.nodes).toEqual([]);
  });

  it("command_skipped removes exactly that node", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", {
      nodes: [node("claude"), node("codex"), node("pi")],
    });

    fake.emitFromBackend("agent_update_command_skipped", { command: "codex", label: "Codex" });
    expect(store.nodes.map((n) => n.command)).toEqual(["claude", "pi"]);
    expect(store.skippedNodes).toEqual(["codex"]);
    expect(store.running).toEqual([]);
    expect(store.results).toEqual([]);
  });

  it("command_started records installBefore on its node and appends to running", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });

    const before = installed("1.0", 0);
    fake.emitFromBackend("agent_update_command_started", node("codex", before));
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
    expect(store.nodes.map((n) => n.command)).toEqual(["claude", "codex"]);
    expect(store.nodes[1].installBefore).toEqual(before);
    expect(store.nodes[0].installBefore).toBeUndefined();
  });

  it("command_started for a command absent from nodes appends a defensive node", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", null);

    fake.emitFromBackend("agent_update_command_started", node("codex"));
    expect(store.nodes).toEqual([node("codex")]);
    expect(store.running).toEqual([{ command: "codex", label: "Codex" }]);
  });

  it("install_state_changed is kept by max seq for a pass node, before or after its result, and ignored for commands outside the pass", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", {
      nodes: [node("claude", installed("1.0", 0)), node("codex", installed("2.0", 0))],
    });

    // an event for a pass node BEFORE its command_finished is kept (FE N2)
    fake.emitFromBackend("agent_update_command_started", node("codex"));
    fake.emitFromBackend("agent_install_state_changed", { command: "codex", install: installed("2.1", 1) });
    expect(store.installAfter.codex).toEqual(installed("2.1", 1));
    fake.emitFromBackend("agent_update_command_finished", ok("codex"));
    expect(store.installAfter.codex).toEqual(installed("2.1", 1));

    // after the result: seq 3 is kept, a later seq 2 is ignored
    fake.emitFromBackend("agent_update_command_finished", ok("claude"));
    fake.emitFromBackend("agent_install_state_changed", { command: "claude", install: installed("1.1", 3) });
    expect(store.installAfter.claude).toEqual(installed("1.1", 3));
    fake.emitFromBackend("agent_install_state_changed", { command: "claude", install: installed("0.9", 2) });
    expect(store.installAfter.claude).toEqual(installed("1.1", 3));

    // a command outside the pass never enters installAfter
    fake.emitFromBackend("agent_install_state_changed", { command: "pi", install: installed("5.0", 9) });
    expect(store.installAfter).toEqual({ codex: installed("2.1", 1), claude: installed("1.1", 3) });

    // a newer state REPLACES the entry: keys the new state omits (version, path) do not survive
    const gone: InstallState = { status: "missing", detail: "'claude' was not found on PATH", seq: 4 };
    fake.emitFromBackend("agent_install_state_changed", { command: "claude", install: gone });
    expect(store.installAfter.claude).toEqual(gone);
    expect(store.installAfter.claude).not.toHaveProperty("version");
    expect(store.installAfter.claude).not.toHaveProperty("path");
  });

  it("finished enters the summary only when the pass was shown and results are non-empty", async () => {
    await wireAgentUpdateListeners();

    // shown + results -> shown and no toast yet
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude")] });
    fake.emitFromBackend("agent_updates_finished", { results: [failed("claude")] });
    expect(store.summary).toBe("shown");
    expect(store.inProgress).toBe(false);
    expect(toastStore.items).toHaveLength(0);

    // not shown -> none and toasts at once
    resetAgentUpdateForTests();
    toastStore.clear();
    fake.emitFromBackend("agent_updates_finished", { results: [failed("claude")] });
    expect(store.summary).toBe("none");
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("Auto-update failed for Claude (claude): exit code 1");

    // shown + empty results -> none
    resetAgentUpdateForTests();
    toastStore.clear();
    fake.emitFromBackend("agent_updates_started", { nodes: [] });
    fake.emitFromBackend("agent_updates_finished", { results: [] });
    expect(store.summary).toBe("none");
    expect(toastStore.items).toHaveLength(0);
  });

  it("dismissAgentUpdateSummary toasts each failure once and marks dismissed", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("claude"), node("codex")] });
    fake.emitFromBackend("agent_updates_finished", {
      results: [failed("claude"), failed("codex", "timed out after 300s (killed)")],
    });
    expect(store.summary).toBe("shown");
    expect(toastStore.items).toHaveLength(0);

    dismissAgentUpdateSummary();
    expect(store.summary).toBe("dismissed");
    expect(toastStore.items.map((item) => item.kind)).toEqual(["error", "error"]);
    expect(toastStore.items.map((item) => item.message)).toEqual([
      "Auto-update failed for Claude (claude): exit code 1",
      "Auto-update failed for Codex (codex): timed out after 300s (killed)",
    ]);

    dismissAgentUpdateSummary();
    expect(store.summary).toBe("dismissed");
    expect(toastStore.items).toHaveLength(2);
  });

  it("a snapshot seeds nodes when the store has none and never resurrects a skipped node", async () => {
    await wireAgentUpdateListeners();
    fake.emitFromBackend("agent_updates_started", { nodes: [node("a"), node("b")] });
    fake.emitFromBackend("agent_update_command_skipped", { command: "b", label: "B" });
    setAgentUpdateStore({ installAfter: { a: installed("1.1", 2) }, summary: "shown" });

    applySnapshot(status({ inProgress: true, nodes: [node("a"), node("b")] }));
    expect(store.nodes.map((n) => n.command)).toEqual(["a"]);
    // a snapshot never changes summary or installAfter
    expect(store.summary).toBe("shown");
    expect(store.installAfter).toEqual({ a: installed("1.1", 2) });

    resetAgentUpdateForTests();
    applySnapshot(status({ inProgress: true, nodes: [node("a"), node("b")] }));
    expect(store.nodes.map((n) => n.command)).toEqual(["a", "b"]);
    expect(store.summary).toBe("none");
    expect(store.installAfter).toEqual({});
  });

  it("a snapshot computed before the pass started never shrinks the nodes", () => {
    const started = { ...agentUpdateInitialState(), inProgress: true, nodes: [node("a"), node("b")] };

    const prePass = mergeSnapshot(started, status({ inProgress: false, nodes: [], running: [], results: [] }));
    expect(prePass.nodes.map((n) => n.command)).toEqual(["a", "b"]);

    const inPass = mergeSnapshot(started, status({ inProgress: true, nodes: [node("a")] }));
    expect(inPass.nodes.map((n) => n.command)).toEqual(["a"]);

    const seeded = mergeSnapshot(
      agentUpdateInitialState(),
      status({ inProgress: false, nodes: [node("a"), node("b")] })
    );
    expect(seeded.nodes.map((n) => n.command)).toEqual(["a", "b"]);
  });

  it("a command_skipped seen before an older snapshot never resurrects the node", async () => {
    await wireAgentUpdateListeners();

    fake.emitFromBackend("agent_update_command_skipped", { command: "b", label: "B" });
    expect(store.skippedNodes).toEqual(["b"]);
    expect(store.nodes).toEqual([]);

    applySnapshot(status({ inProgress: true, nodes: [node("a"), node("b")] }));
    expect(store.nodes.map((n) => n.command)).toEqual(["a"]);

    setAgentUpdateStore("nodes", []);
    applySnapshot(status({ inProgress: false, nodes: [node("a"), node("b")] }));
    expect(store.nodes.map((n) => n.command)).toEqual(["a"]);

    fake.emitFromBackend("agent_updates_started", { nodes: [node("a"), node("b")] });
    expect(store.skippedNodes).toEqual([]);
    expect(store.nodes.map((n) => n.command)).toEqual(["a", "b"]);
  });

  it("a mid-pass snapshot defers the failure toasts to the summary close, a post-pass snapshot toasts at once", async () => {
    // (i) a surface mounted in the middle of the pass
    fake.resolve(
      "get_agent_update_status",
      status({
        inProgress: true,
        nodes: [node("a"), node("b")],
        running: [{ command: "b", label: "B" }],
        results: [failed("a")],
        answered: {},
      })
    );
    await wireAgentUpdateListeners();
    await settle();
    expect(store.inProgress).toBe(true);
    expect(store.results).toEqual([failed("a")]);
    expect(toastStore.items).toHaveLength(0);

    fake.emitFromBackend("agent_updates_finished", { results: [failed("a"), ok("b")] });
    expect(store.summary).toBe("shown");
    expect(toastStore.items).toHaveLength(0);

    dismissAgentUpdateSummary();
    expect(toastStore.items).toHaveLength(1);
    expect(toastStore.items[0].message).toBe("Auto-update failed for A (a): exit code 1");

    // (ii) a surface mounted after the pass
    resetAgentUpdateForTests();
    toastStore.clear();
    fake.resolve("get_agent_update_status", status({ inProgress: false, results: [failed("a")] }));
    await wireAgentUpdateListeners();
    await settle();
    expect(toastStore.items).toHaveLength(1);
    expect(store.summary).toBe("none");
  });
});
