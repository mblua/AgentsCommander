// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { UnlistenFn } from "../../shared/transport";
import type { AgentUpdateOverviewRow, AgentUpdateResult, InstallState } from "../../shared/types";
import { agentUpdateStore, resetAgentUpdateForTests } from "../agent-update";
import { agentUpdateOverviewStore } from "../stores/agent-update-overview";
import AgentAutoUpdateStatusList from "./AgentAutoUpdateStatusList";

const CMD = "get_agent_update_overview";
const [, setAgentUpdateStore] = agentUpdateStore;

const checking: InstallState = { status: "checking", seq: 0 };

/**
 * #1691 - the canonical result shape. This table's live column reads the agent-update store,
 * so its fixtures must carry the required `outcome`/probe/`change` keys like any other result.
 */
function okResult(command: string, label: string): AgentUpdateResult {
  return {
    command,
    label,
    ok: true,
    outcome: "succeeded",
    installBefore: null,
    installAfter: null,
    change: "unknown",
  };
}

function failedResult(command: string, label: string, error: string): AgentUpdateResult {
  return { ...okResult(command, label), ok: false, outcome: "failed", error };
}

function installed(version: string, seq = 1): InstallState {
  return { status: "installed", version, path: `C:\\bin\\${version}.cmd`, seq };
}

function missing(seq = 1, detail = "'codex' was not found on PATH"): InstallState {
  return { status: "missing", detail, seq };
}

function probeFailed(seq = 1): InstallState {
  return { status: "probeFailed", detail: "exit code 3: boom", path: "C:\\bin\\pi.cmd", seq };
}

function unprobed(seq = 1): InstallState {
  return { status: "unprobed", detail: "explicit path: version not probed", path: "C:\\tools\\opencode.exe", seq };
}

function row(key: string, command: string, install: InstallState = checking): AgentUpdateOverviewRow {
  return { key, label: key, command, color: "#10b981", updateCommands: [`${command} update`], install };
}

const SIX_ROWS: AgentUpdateOverviewRow[] = [
  row("claude", "claude", installed("2.1.245", 1)),
  row("codex", "codex", checking),
  row("hermes", "hermes", missing(2, "'hermes' was not found on PATH")),
  row("pi", "pi", probeFailed(3)),
  row("opencode", "opencode", unprobed(4)),
  row("antigravity", "agy", installed("1.1.20", 5)),
];

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

async function settle(): Promise<void> {
  await new Promise<void>((resolve) => setTimeout(resolve, 0));
}

function q(root: HTMLElement, testId: string): HTMLElement | null {
  return root.querySelector<HTMLElement>(`[data-ac-testid="${testId}"]`);
}

function cell(root: HTMLElement, key: string, column: "agent" | "configured" | "installed" | "live"): HTMLElement | null {
  return q(root, `settings.autoUpdate.row.${key}.${column}`);
}

function rowIds(root: HTMLElement): string[] {
  return Array.from(
    root.querySelectorAll<HTMLElement>('[data-ac-testid^="settings.autoUpdate.row."][data-ac-role="row"]')
  ).map((element) => element.getAttribute("data-ac-testid") ?? "");
}

function mount(
  fake: FakeTransport,
  props: { autoUpdateByCommand?: Record<string, boolean>; registered?: string[] } = {}
) {
  return renderWithFakeTransport(
    () => (
      <AgentAutoUpdateStatusList
        autoUpdateByCommand={() => props.autoUpdateByCommand ?? {}}
        registeredCommands={() => props.registered ?? []}
      />
    ),
    fake
  );
}

describe("AgentAutoUpdateStatusList (#1551)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
    resetAgentUpdateForTests();
    agentUpdateOverviewStore.resetForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    agentUpdateOverviewStore.resetForTests();
    resetAgentUpdateForTests();
    resetUiStoresForTests();
    document.body.replaceChildren();
    vi.restoreAllMocks();
  });

  it("listeners are registered before the first overview invoke", async () => {
    const fake = new FakeTransport();
    let listensAtInvoke: [number, number] | null = null;
    fake.onInvoke(CMD, () => {
      listensAtInvoke = [
        fake.listensFor("agent_install_state_changed").length,
        fake.listensFor("agent_updates_finished").length,
      ];
      return Promise.resolve(SIX_ROWS);
    });
    const rendered = mount(fake);
    try {
      await waitFor(() => expect(fake.callsFor(CMD)).toHaveLength(1));
      expect(listensAtInvoke).toEqual([1, 1]);
      expect(fake.callsFor(CMD)).toEqual([{ cmd: CMD, args: {} }]);
    } finally {
      rendered.cleanup();
    }
  });

  it("a fast missing command announced before the response paints Not installed", async () => {
    const fake = new FakeTransport();
    fake.onInvoke(CMD, () => {
      // the probe scheduled by this very call commits before the response is returned
      fake.emitFromBackend("agent_install_state_changed", { command: "codex", install: missing(1) });
      return Promise.resolve([row("codex", "codex", checking)]);
    });
    const rendered = mount(fake);
    try {
      await waitFor(() => expect(cell(rendered.root, "codex", "installed")).toBeTruthy());
      const installedCell = cell(rendered.root, "codex", "installed")!;
      expect(installedCell.getAttribute("data-ac-state")).toBe("missing");
      expect(installedCell.textContent).toBe("Not installed");
      expect(installedCell.getAttribute("title")).toBe("'codex' was not found on PATH");
    } finally {
      rendered.cleanup();
    }
  });

  it("an event after the response updates one row, and both rows of a duplicate command", async () => {
    const fake = new FakeTransport();
    fake.onInvoke(CMD, () =>
      Promise.resolve([row("codex", "codex", checking), row("pi", "pi", checking), row("pi-alt", "pi", checking)])
    );
    const rendered = mount(fake);
    try {
      await waitFor(() => expect(cell(rendered.root, "pi-alt", "installed")).toBeTruthy());
      expect(cell(rendered.root, "pi", "installed")!.getAttribute("data-ac-state")).toBe("checking");
      expect(cell(rendered.root, "pi", "installed")!.textContent).toBe("Checking...");

      fake.emitFromBackend("agent_install_state_changed", { command: "pi", install: installed("0.84.3", 1) });
      expect(cell(rendered.root, "pi", "installed")!.getAttribute("data-ac-state")).toBe("installed");
      expect(cell(rendered.root, "pi", "installed")!.textContent).toBe("0.84.3");
      expect(cell(rendered.root, "pi-alt", "installed")!.getAttribute("data-ac-state")).toBe("installed");
      expect(cell(rendered.root, "pi-alt", "installed")!.textContent).toBe("0.84.3");
      expect(cell(rendered.root, "codex", "installed")!.getAttribute("data-ac-state")).toBe("checking");
    } finally {
      rendered.cleanup();
    }
  });

  it("remount re-invokes the overview and never paints the previous rows first", async () => {
    const fake = new FakeTransport();
    const gate = deferred<AgentUpdateOverviewRow[]>();
    let respond = (): Promise<AgentUpdateOverviewRow[]> => Promise.resolve(SIX_ROWS);
    fake.onInvoke(CMD, () => respond());

    const first = mount(fake);
    await waitFor(() => expect(rowIds(first.root)).toHaveLength(6));
    first.cleanup();

    respond = () => gate.promise;
    const second = mount(fake);
    try {
      // synchronously after the second render: loading, and NO row of the previous mount
      expect(q(second.root, "settings.autoUpdate.loading")).toBeTruthy();
      expect(rowIds(second.root)).toHaveLength(0);
      expect(q(second.root, "settings.autoUpdate.list")).toBeNull();

      await waitFor(() => expect(fake.callsFor(CMD)).toHaveLength(2));
      expect(rowIds(second.root)).toHaveLength(0);
      gate.resolve(SIX_ROWS);
      await waitFor(() => expect(rowIds(second.root)).toHaveLength(6));
      expect(q(second.root, "settings.autoUpdate.loading")).toBeNull();
      expect(fake.callsFor(CMD)).toHaveLength(2);
    } finally {
      second.cleanup();
    }
  });

  it("agent_updates_finished triggers a re-fetch", async () => {
    const fake = new FakeTransport();
    fake.onInvoke(CMD, () => Promise.resolve(SIX_ROWS));
    const rendered = mount(fake);
    try {
      await waitFor(() => expect(rowIds(rendered.root)).toHaveLength(6));
      expect(fake.callsFor(CMD)).toHaveLength(1);
      fake.emitFromBackend("agent_updates_finished", { results: [] });
      await waitFor(() => expect(fake.callsFor(CMD)).toHaveLength(2));
    } finally {
      rendered.cleanup();
    }
  });

  it("loading -> table, and an empty catalog renders the explicit empty note", async () => {
    const fake = new FakeTransport();
    const gate = deferred<AgentUpdateOverviewRow[]>();
    fake.onInvoke(CMD, () => gate.promise);
    const rendered = mount(fake);
    try {
      expect(q(rendered.root, "settings.autoUpdate.block")!.getAttribute("data-ac-role")).toBe("region");
      expect(q(rendered.root, "settings.autoUpdate.loading")!.textContent).toBe("Loading auto-update status...");
      expect(q(rendered.root, "settings.autoUpdate.list")).toBeNull();
      expect(q(rendered.root, "settings.autoUpdate.hint")!.textContent).toBe(
        "Only registered coding agents are updated at startup. Change a setting with the Auto-update dropdown of the corresponding agent above."
      );

      gate.resolve(SIX_ROWS);
      await waitFor(() => expect(q(rendered.root, "settings.autoUpdate.list")).toBeTruthy());
      expect(q(rendered.root, "settings.autoUpdate.loading")).toBeNull();
      const table = q(rendered.root, "settings.autoUpdate.list")!;
      expect(table.getAttribute("aria-label")).toBe("Auto-update status");
      expect(Array.from(table.querySelectorAll("th")).map((th) => th.textContent)).toEqual([
        "Agent",
        "Command",
        "Auto-update",
        "Installed",
        "Status",
      ]);
      expect(table.querySelector("tbody")!.getAttribute("aria-live")).toBe("polite");
      expect(rowIds(rendered.root)).toEqual([
        "settings.autoUpdate.row.claude",
        "settings.autoUpdate.row.codex",
        "settings.autoUpdate.row.hermes",
        "settings.autoUpdate.row.pi",
        "settings.autoUpdate.row.opencode",
        "settings.autoUpdate.row.antigravity",
      ]);
      expect(q(rendered.root, "settings.autoUpdate.row.antigravity")!.getAttribute("data-ac-command")).toBe("agy");
      expect(q(rendered.root, "settings.autoUpdate.row.antigravity")!.querySelector("code")!.textContent).toBe("agy");
    } finally {
      rendered.cleanup();
    }

    const empty = new FakeTransport();
    empty.onInvoke(CMD, () => Promise.resolve([]));
    const emptyRender = mount(empty);
    try {
      await waitFor(() => expect(q(emptyRender.root, "settings.autoUpdate.empty")).toBeTruthy());
      expect(q(emptyRender.root, "settings.autoUpdate.empty")!.textContent).toBe(
        "No coding agent in the catalog supports auto-update."
      );
      expect(q(emptyRender.root, "settings.autoUpdate.list")).toBeNull();
      expect(q(emptyRender.root, "settings.autoUpdate.loading")).toBeNull();
    } finally {
      emptyRender.cleanup();
    }
  });

  it("data-ac-state per cell covers ask/yes/no, every install state and every live state", async () => {
    setAgentUpdateStore({
      running: [{ command: "hermes", label: "Hermes" }],
      results: [
        okResult("claude", "Claude"),
        failedResult("pi", "Pi", "exit code 1"),
      ],
    });
    const fake = new FakeTransport();
    fake.onInvoke(CMD, () => Promise.resolve(SIX_ROWS));
    const rendered = mount(fake, {
      autoUpdateByCommand: { claude: true, codex: false },
      registered: ["claude", "codex", "hermes", "pi"],
    });
    try {
      await waitFor(() => expect(rowIds(rendered.root)).toHaveLength(6));
      const root = rendered.root;
      const state = (key: string, column: "agent" | "configured" | "installed" | "live") =>
        cell(root, key, column)!.getAttribute("data-ac-state");
      const text = (key: string, column: "configured" | "installed" | "live") => cell(root, key, column)!.textContent;

      expect(["claude", "codex", "hermes", "pi", "opencode", "antigravity"].map((key) => state(key, "configured"))).toEqual([
        "yes",
        "no",
        "ask",
        "ask",
        "ask",
        "ask",
      ]);
      expect(text("claude", "configured")).toBe("Yes");
      expect(text("codex", "configured")).toBe("No");
      expect(text("hermes", "configured")).toBe("Will ask at startup");

      expect(state("claude", "installed")).toBe("installed");
      expect(text("claude", "installed")).toBe("2.1.245");
      expect(cell(root, "claude", "installed")!.getAttribute("title")).toBe("C:\\bin\\2.1.245.cmd");
      expect(state("codex", "installed")).toBe("checking");
      expect(text("codex", "installed")).toBe("Checking...");
      expect(state("hermes", "installed")).toBe("missing");
      expect(text("hermes", "installed")).toBe("Not installed");
      expect(cell(root, "hermes", "installed")!.getAttribute("title")).toBe("'hermes' was not found on PATH");
      expect(state("pi", "installed")).toBe("probe-failed");
      expect(text("pi", "installed")).toBe("Not installed");
      expect(cell(root, "pi", "installed")!.getAttribute("title")).toBe(
        "Version check failed: exit code 3: boom (C:\\bin\\pi.cmd)"
      );
      expect(state("opencode", "installed")).toBe("unprobed");
      expect(text("opencode", "installed")).toBe("Installed");
      expect(cell(root, "opencode", "installed")!.getAttribute("title")).toBe(
        "explicit path: version not probed (C:\\tools\\opencode.exe)"
      );

      expect(state("claude", "live")).toBe("ok");
      expect(text("claude", "live")).toBe("Updated");
      expect(cell(root, "claude", "live")!.hasAttribute("title")).toBe(false);
      expect(state("hermes", "live")).toBe("updating");
      expect(text("hermes", "live")).toBe("Updating...");
      expect(state("pi", "live")).toBe("failed");
      expect(text("pi", "live")).toBe("Update failed");
      expect(cell(root, "pi", "live")!.getAttribute("title")).toBe("exit code 1");
      expect(state("codex", "live")).toBe("idle");
      expect(text("codex", "live")).toBe("-");

      // the live cells follow the agent-update store
      setAgentUpdateStore({
        running: [],
        results: [
          okResult("claude", "Claude"),
          failedResult("pi", "Pi", "exit code 1"),
          okResult("hermes", "Hermes"),
        ],
      });
      expect(state("hermes", "live")).toBe("ok");
      expect(text("hermes", "live")).toBe("Updated");
    } finally {
      rendered.cleanup();
    }
  });

  it("marks unregistered rows with the (not registered) note and its title", async () => {
    const fake = new FakeTransport();
    fake.onInvoke(CMD, () => Promise.resolve(SIX_ROWS));
    const rendered = mount(fake, { registered: ["claude", "agy"] });
    try {
      await waitFor(() => expect(rowIds(rendered.root)).toHaveLength(6));
      const agent = (key: string) => cell(rendered.root, key, "agent")!;
      expect(agent("claude").getAttribute("data-ac-state")).toBe("registered");
      expect(agent("claude").querySelector(".settings-auto-update-note")).toBeNull();
      expect(agent("claude").textContent).toBe("claude");
      expect(agent("antigravity").getAttribute("data-ac-state")).toBe("registered");
      expect(agent("codex").getAttribute("data-ac-state")).toBe("unregistered");
      const note = agent("codex").querySelector<HTMLElement>(".settings-auto-update-note")!;
      expect(note.textContent).toBe("(not registered)");
      expect(note.getAttribute("title")).toBe("Only registered coding agents are updated at startup");
      expect(agent("codex").querySelector<HTMLElement>(".settings-color-dot")!.style.background).toBe("rgb(16, 185, 129)");
    } finally {
      rendered.cleanup();
    }
  });

  it("a rejected invoke renders settings.autoUpdate.error and never an empty table", async () => {
    const fake = new FakeTransport();
    fake.reject(CMD, "overview failure");
    const rendered = mount(fake);
    try {
      await waitFor(() => expect(q(rendered.root, "settings.autoUpdate.error")).toBeTruthy());
      const note = q(rendered.root, "settings.autoUpdate.error")!;
      expect(note.getAttribute("data-ac-state")).toBe("error");
      expect(note.textContent).toContain("Auto-update status unavailable: ");
      expect(note.textContent).toContain("overview failure");
      expect(q(rendered.root, "settings.autoUpdate.list")).toBeNull();
      expect(q(rendered.root, "settings.autoUpdate.empty")).toBeNull();
      expect(q(rendered.root, "settings.autoUpdate.loading")).toBeNull();
      expect(q(rendered.root, "settings.autoUpdate.hint")).toBeTruthy();
    } finally {
      rendered.cleanup();
    }
  });

  it("unlisten on dispose, including a listen that resolves after dispose", async () => {
    const fake = new FakeTransport();
    fake.onInvoke(CMD, () => Promise.resolve(SIX_ROWS));
    const rendered = mount(fake);
    // dispose synchronously right after render: the listen promises settle only afterwards
    rendered.cleanup();
    await settle();

    const spy = vi.spyOn(agentUpdateOverviewStore, "applyInstallState");
    fake.emitFromBackend("agent_install_state_changed", { command: "codex", install: missing(1) });
    expect(spy).not.toHaveBeenCalled();
    // disposed before the listeners settled: the overview was never invoked
    expect(fake.callsFor(CMD)).toHaveLength(0);
  });

  it("one listener rejects, the other resolves: the overview still loads, the survivor works and is unlistened on disposal", async () => {
    const fake = new FakeTransport();
    fake.onInvoke(CMD, () => Promise.resolve([row("codex", "codex", checking)]));
    const originalListen = fake.listen.bind(fake) as (...args: unknown[]) => Promise<UnlistenFn>;
    vi.spyOn(fake, "listen").mockImplementation(((...args: unknown[]) => {
      const [event] = args as [string];
      if (event === "agent_updates_finished") return Promise.reject(new Error("listen unavailable"));
      return originalListen(...args);
    }) as unknown as typeof fake.listen);
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    const rendered = mount(fake);
    try {
      await waitFor(() => expect(fake.callsFor(CMD)).toHaveLength(1));
      expect(errorSpy).toHaveBeenCalledTimes(1);
      expect(String(errorSpy.mock.calls[0][0])).toContain("agent_updates_finished");
      await waitFor(() => expect(cell(rendered.root, "codex", "installed")).toBeTruthy());
      expect(cell(rendered.root, "codex", "installed")!.getAttribute("data-ac-state")).toBe("checking");

      fake.emitFromBackend("agent_install_state_changed", { command: "codex", install: missing(1) });
      expect(cell(rendered.root, "codex", "installed")!.getAttribute("data-ac-state")).toBe("missing");
    } finally {
      rendered.cleanup();
    }

    const spy = vi.spyOn(agentUpdateOverviewStore, "applyInstallState");
    fake.emitFromBackend("agent_install_state_changed", { command: "codex", install: missing(2) });
    expect(spy).not.toHaveBeenCalled();
  });

  it("a rejection settles only together with a late resolution", async () => {
    const fake = new FakeTransport();
    fake.onInvoke(CMD, () => Promise.resolve([row("codex", "codex", checking)]));
    const late = deferred<void>();
    const originalListen = fake.listen.bind(fake) as (...args: unknown[]) => Promise<UnlistenFn>;
    vi.spyOn(fake, "listen").mockImplementation(((...args: unknown[]) => {
      const [event] = args as [string];
      if (event === "agent_updates_finished") return Promise.reject(new Error("listen unavailable"));
      const registered = originalListen(...args);
      return late.promise.then(() => registered);
    }) as unknown as typeof fake.listen);
    const errorSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    const rendered = mount(fake);
    try {
      await settle();
      await settle();
      // the rejection alone settles nothing: no invoke until the deferred listen resolves
      expect(fake.callsFor(CMD)).toHaveLength(0);
      expect(q(rendered.root, "settings.autoUpdate.loading")).toBeTruthy();

      late.resolve();
      await waitFor(() => expect(fake.callsFor(CMD)).toHaveLength(1));
      expect(errorSpy).toHaveBeenCalledTimes(1);
      await waitFor(() => expect(cell(rendered.root, "codex", "installed")).toBeTruthy());

      // the late-registered listener works while mounted
      fake.emitFromBackend("agent_install_state_changed", { command: "codex", install: missing(1) });
      expect(cell(rendered.root, "codex", "installed")!.getAttribute("data-ac-state")).toBe("missing");
    } finally {
      rendered.cleanup();
    }

    // ...and its unlistener ran on disposal
    const spy = vi.spyOn(agentUpdateOverviewStore, "applyInstallState");
    fake.emitFromBackend("agent_install_state_changed", { command: "codex", install: missing(2) });
    expect(spy).not.toHaveBeenCalled();
  });
});
