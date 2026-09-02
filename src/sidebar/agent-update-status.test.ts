import { describe, expect, it } from "vitest";
import type {
  AgentUpdateCommandRef,
  AgentUpdateNode,
  AgentUpdateOverviewRow,
  AgentUpdateResult,
  InstallState,
} from "../shared/types";
import {
  CANCELLED_LABEL,
  CONFIGURED_LABELS,
  FAILED_LABEL,
  LIVE_LABELS,
  NODE_STATE_LABELS,
  NOTHING_TO_UPDATE_SUFFIX,
  NOT_INSTALLED_LABEL,
  UNKNOWN_ERROR_LABEL,
  UPDATE_UNVERIFIED_LABEL,
  configuredState,
  deriveAutoUpdateRows,
  deriveTimelineHeader,
  deriveTimelineNodes,
  describeInstall,
  installedView,
  liveView,
  outcomeState,
  outcomeText,
} from "./agent-update-status";

const checking: InstallState = { status: "checking", seq: 0 };

function installed(version: string, seq = 1, path = `C:\\bin\\${version}.cmd`): InstallState {
  return { status: "installed", version, path, seq };
}

function missing(detail = "'x' was not found on PATH", seq = 1): InstallState {
  return { status: "missing", detail, seq };
}

function probeFailed(detail = "exit code 3: boom", seq = 1, path = "C:\\bin\\x.cmd"): InstallState {
  return { status: "probeFailed", detail, path, seq };
}

function unprobed(detail = "explicit path: version not probed", seq = 1, path = "C:\\tools\\x.exe"): InstallState {
  return { status: "unprobed", detail, path, seq };
}

interface CatalogEntry {
  key: string;
  label: string;
  command: string;
  color: string;
  updateCommands: string[];
}

/** The embedded default catalog (plan 3.2): seven entries, cursor ships no update command. */
const DEFAULT_CATALOG: CatalogEntry[] = [
  { key: "claude", label: "Claude Code", command: "claude", color: "#d97706", updateCommands: ["claude --update"] },
  { key: "codex", label: "Codex", command: "codex", color: "#10b981", updateCommands: ["codex update"] },
  { key: "hermes", label: "Hermes", command: "hermes", color: "#8b5cf6", updateCommands: ["hermes update --yes"] },
  { key: "cursor", label: "Cursor", command: "agent", color: "#0ea5e9", updateCommands: [] },
  { key: "pi", label: "Pi", command: "pi", color: "#ec4899", updateCommands: ["pi update"] },
  { key: "opencode", label: "OpenCode", command: "opencode", color: "#64748b", updateCommands: ["opencode upgrade"] },
  { key: "antigravity", label: "Antigravity", command: "agy", color: "#f97316", updateCommands: ["agy update"] },
];

/**
 * Mirrors the backend row builder (plan 5.4 step 6): one row per entry with a non-empty
 * sequence, catalog order, no dedup; commands without an install entry are `checking`.
 */
function overviewRows(
  catalog: CatalogEntry[],
  install: Record<string, InstallState> = {}
): AgentUpdateOverviewRow[] {
  return catalog
    .filter((entry) => entry.updateCommands.length > 0)
    .map((entry) => ({ ...entry, install: install[entry.command] ?? checking }));
}

function ref(command: string): AgentUpdateCommandRef {
  return { command, label: command.toUpperCase() };
}

/** #1691 - the canonical succeeded result: both probe keys present, no version claim. */
function ok(command: string): AgentUpdateResult {
  return {
    command,
    label: command.toUpperCase(),
    ok: true,
    outcome: "succeeded",
    installBefore: null,
    installAfter: null,
    change: "unknown",
  };
}

function failed(command: string, error?: string): AgentUpdateResult {
  const base: AgentUpdateResult = { ...ok(command), ok: false, outcome: "failed" };
  return error === undefined ? base : { ...base, error };
}

/** #1691 - a cancelled result: `ok=false`, but never a failure. */
function cancelled(command: string): AgentUpdateResult {
  return { ...ok(command), ok: false, outcome: "cancelled" };
}

function changed(command: string, before: string, after: string): AgentUpdateResult {
  return {
    ...ok(command),
    change: "changed",
    installBefore: installed(before, 0),
    installAfter: installed(after, 1),
  };
}

function unchanged(command: string, version: string): AgentUpdateResult {
  return {
    ...ok(command),
    change: "unchanged",
    installBefore: installed(version, 0),
    installAfter: installed(version, 1),
  };
}

function node(command: string, installBefore?: InstallState): AgentUpdateNode {
  const base = { command, label: command.toUpperCase(), updateCommands: [`${command} update`] };
  return installBefore ? { ...base, installBefore } : base;
}

const NO_LIVE = { running: [] as AgentUpdateCommandRef[], results: [] as AgentUpdateResult[] };

describe("deriveAutoUpdateRows and its cell derivations (#1551)", () => {
  it("keeps catalog order, keeps every update-capable row and never lists cursor", () => {
    const views = deriveAutoUpdateRows(overviewRows(DEFAULT_CATALOG), {
      autoUpdateByCommand: {},
      registeredCommands: [],
      ...NO_LIVE,
    });
    expect(views.map((view) => view.key)).toEqual([
      "claude",
      "codex",
      "hermes",
      "pi",
      "opencode",
      "antigravity",
    ]);
    expect(views.some((view) => view.key === "cursor" || view.command === "agent")).toBe(false);
    expect(views[1]).toMatchObject({ key: "codex", label: "Codex", command: "codex", color: "#10b981" });
  });

  it("keeps duplicate-command rows (pi and pi-alt) with identical command-keyed install state", () => {
    const catalog: CatalogEntry[] = [
      ...DEFAULT_CATALOG,
      { key: "pi-alt", label: "Pi (alt)", command: "pi", color: "#ec4899", updateCommands: ["pi update"] },
    ];
    const views = deriveAutoUpdateRows(overviewRows(catalog, { pi: installed("0.84.3", 2) }), {
      autoUpdateByCommand: {},
      registeredCommands: [],
      ...NO_LIVE,
    });
    const piRows = views.filter((view) => view.command === "pi");
    expect(piRows.map((view) => view.key)).toEqual(["pi", "pi-alt"]);
    expect(piRows.map((view) => view.installed.label)).toEqual(["0.84.3", "0.84.3"]);
    expect(views).toHaveLength(7);
  });

  it("maps the tri-state from the map: true -> yes, false -> no, absent -> ask (even when unregistered)", () => {
    expect(configuredState({ claude: true }, "claude")).toBe("yes");
    expect(configuredState({ claude: false }, "claude")).toBe("no");
    expect(configuredState({}, "claude")).toBe("ask");
    expect(CONFIGURED_LABELS).toEqual({ yes: "Yes", no: "No", ask: "Will ask at startup" });

    const views = deriveAutoUpdateRows(overviewRows(DEFAULT_CATALOG), {
      autoUpdateByCommand: { claude: true, codex: false },
      registeredCommands: ["claude", "codex"],
      ...NO_LIVE,
    });
    const byKey = Object.fromEntries(views.map((view) => [view.key, view]));
    expect(byKey.claude).toMatchObject({ configured: "yes", registered: true });
    expect(byKey.codex).toMatchObject({ configured: "no", registered: true });
    // absent key on an unregistered row: the stored policy is shown, registration by the marker
    expect(byKey.hermes).toMatchObject({ configured: "ask", registered: false });
    expect(byKey.pi).toMatchObject({ configured: "ask", registered: false });
  });

  it("installedView covers every branch and label", () => {
    expect(installedView(checking)).toEqual({ state: "checking", label: "Checking..." });
    expect(installedView(installed("1.2.3", 1, "C:\\bin\\codex.cmd"))).toEqual({
      state: "installed",
      label: "1.2.3",
      title: "C:\\bin\\codex.cmd",
    });
    expect(installedView(missing("'codex' was not found on PATH"))).toEqual({
      state: "missing",
      label: NOT_INSTALLED_LABEL,
      title: "'codex' was not found on PATH",
    });
    expect(installedView(probeFailed("exit code 3: boom", 1, "C:\\bin\\codex.cmd"))).toEqual({
      state: "probe-failed",
      label: NOT_INSTALLED_LABEL,
      title: "Version check failed: exit code 3: boom (C:\\bin\\codex.cmd)",
    });
    expect(installedView(unprobed("explicit path: version not probed", 1, "C:\\tools\\x.exe"))).toEqual({
      state: "unprobed",
      label: "Installed",
      title: "explicit path: version not probed (C:\\tools\\x.exe)",
    });
    expect(NOT_INSTALLED_LABEL).toBe("Not installed");
  });

  it("liveView covers every branch: running wins, then the result, else idle", () => {
    expect(LIVE_LABELS).toEqual({ idle: "-", updating: "Updating...", ok: "Updated", failed: "Update failed" });
    expect(liveView("codex", [ref("codex")], [])).toEqual({ state: "updating", label: "Updating..." });
    expect(liveView("codex", [ref("codex")], [ok("codex")])).toEqual({ state: "updating", label: "Updating..." });
    expect(liveView("codex", [], [ok("codex")])).toEqual({ state: "ok", label: "Updated" });
    expect(liveView("codex", [], [failed("codex", "exit code 1")])).toEqual({
      state: "failed",
      label: "Update failed",
      title: "exit code 1",
    });
    expect(liveView("codex", [], [failed("codex")])).toEqual({
      state: "failed",
      label: "Update failed",
      title: UNKNOWN_ERROR_LABEL,
    });
    expect(liveView("codex", [ref("pi")], [ok("pi")])).toEqual({ state: "idle", label: "-" });

    const views = deriveAutoUpdateRows(overviewRows(DEFAULT_CATALOG), {
      autoUpdateByCommand: {},
      registeredCommands: [],
      running: [ref("codex")],
      results: [ok("claude"), failed("pi", "timed out after 300s (killed)")],
    });
    expect(views.map((view) => view.live.state)).toEqual(["ok", "updating", "idle", "failed", "idle", "idle"]);
    expect(views[3].live.title).toBe("timed out after 300s (killed)");
  });
});

describe("timeline derivations (#1551 round 5, #1691)", () => {
  it("describeInstall claims a version only for a nonempty installed probe", () => {
    expect(describeInstall(installed("1.0"))).toBe("1.0");
    // #1691 - `missing` and `probeFailed` are no longer comparable display values
    expect(describeInstall(missing())).toBeNull();
    expect(describeInstall(probeFailed())).toBeNull();
    expect(describeInstall(unprobed())).toBeNull();
    expect(describeInstall(checking)).toBeNull();
    expect(describeInstall({ status: "installed", seq: 1 })).toBeNull();
    expect(describeInstall(null)).toBeNull();
    expect(describeInstall(undefined)).toBeNull();
  });

  it("outcomeText prints the exact terminal string of every outcome, from the result's own fields", () => {
    // unchanged: the version plus the fixed suffix, from either probe
    expect(outcomeText(unchanged("a", "1.2.3"))).toBe("1.2.3 (Nothing to update)");
    expect(outcomeText({ ...unchanged("a", "1.2.3"), installAfter: null })).toBe("1.2.3 (Nothing to update)");
    // unchanged without any comparable version falls back to the unverified string
    expect(outcomeText({ ...ok("a"), change: "unchanged" })).toBe(UPDATE_UNVERIFIED_LABEL);

    // changed: ASCII arrow, both versions
    expect(outcomeText(changed("a", "1.2.3", "1.2.4"))).toBe("Ready - 1.2.3 -> 1.2.4");
    // a `changed` claim with only one comparable version never invents the other
    expect(outcomeText({ ...changed("a", "1.2.3", "1.2.4"), installBefore: missing() })).toBe(
      UPDATE_UNVERIFIED_LABEL
    );

    // unknown
    expect(outcomeText(ok("a"))).toBe("Update completed - Version could not be verified");

    // cancelled and failed
    expect(outcomeText(cancelled("a"))).toBe("Cancelled");
    expect(outcomeText(failed("a", "exit code 1"))).toBe("Failed - exit code 1");
    expect(outcomeText(failed("a"))).toBe("Failed");
    expect(outcomeText({ ...failed("a"), error: null })).toBe("Failed");
    // a cancelled result never prints a version, whatever its probes say
    expect(outcomeText({ ...cancelled("a"), change: "changed", installBefore: installed("1.0", 0), installAfter: installed("1.1", 1) })).toBe(
      "Cancelled"
    );

    expect(CANCELLED_LABEL).toBe("Cancelled");
    expect(FAILED_LABEL).toBe("Failed");
    expect(NOTHING_TO_UPDATE_SUFFIX).toBe("(Nothing to update)");
    expect(UPDATE_UNVERIFIED_LABEL).toBe("Update completed - Version could not be verified");
  });

  it("every visible string is ASCII: no Unicode arrow, no middle dot, no Spanish literal", () => {
    const strings = [
      ...Object.values(NODE_STATE_LABELS),
      ...Object.values(LIVE_LABELS),
      ...Object.values(CONFIGURED_LABELS),
      NOT_INSTALLED_LABEL,
      UNKNOWN_ERROR_LABEL,
      NOTHING_TO_UPDATE_SUFFIX,
      UPDATE_UNVERIFIED_LABEL,
      CANCELLED_LABEL,
      FAILED_LABEL,
      outcomeText(changed("a", "1.2.3", "1.2.4")),
      outcomeText(unchanged("a", "1.2.3")),
      outcomeText(failed("a", "exit code 1")),
      outcomeText(cancelled("a")),
      deriveTimelineHeader(deriveTimelineNodes([node("a"), node("b")], [], [], [failed("a", "x")])).text,
    ];
    for (const value of strings) {
      expect(value).toMatch(/^[\x20-\x7E]*$/);
      expect(value).not.toContain("→");
      expect(value).not.toContain("·");
    }
  });

  it("outcomeState maps each outcome to its terminal row state", () => {
    expect(outcomeState(ok("a"))).toBe("ok");
    expect(outcomeState(failed("a", "boom"))).toBe("failed");
    expect(outcomeState(cancelled("a"))).toBe("cancelled");
  });

  it("deriveTimelineNodes derives every node state in node order with the four nonterminal words", () => {
    const views = deriveTimelineNodes(
      [node("a"), node("b"), node("c"), node("d"), node("e"), node("f")],
      [ref("b"), ref("e")],
      [ref("c")],
      [ok("a"), failed("d", "exit code 1")],
      new Set(["e"])
    );
    expect(views.map((view) => view.command)).toEqual(["a", "b", "c", "d", "e", "f"]);
    expect(views.map((view) => view.state)).toEqual([
      "ok",
      "updating",
      "verifying",
      "failed",
      "cancelling",
      "pending",
    ]);
    // a nonterminal row shows the state word and no detail; a terminal row the reverse
    expect(views.map((view) => view.stateText)).toEqual([
      null,
      "Updating...",
      "Verifying...",
      null,
      "Cancelling...",
      "Pending",
    ]);
    expect(views.map((view) => view.detail)).toEqual([
      "Update completed - Version could not be verified",
      null,
      null,
      "Failed - exit code 1",
      null,
      null,
    ]);
    expect(views.map((view) => view.terminal)).toEqual([true, false, false, true, false, false]);
    // cancellable: every nonterminal row EXCEPT one already cancelling
    expect(views.map((view) => view.cancellable)).toEqual([false, true, true, false, false, true]);
    for (const view of views) expect(view.detailTitle).toBe(view.detail);

    expect(NODE_STATE_LABELS).toEqual({
      pending: "Pending",
      updating: "Updating...",
      verifying: "Verifying...",
      cancelling: "Cancelling...",
    });
    expect(views[1]).toMatchObject({ label: "B", updateCommands: ["b update"] });
  });

  it("cancellation outranks verification, and a terminal result outranks both", () => {
    // a verifying row whose cancellation was requested says Cancelling..., not Verifying...
    const requested = deriveTimelineNodes([node("a")], [], [ref("a")], [], new Set(["a"]));
    expect(requested[0]).toMatchObject({ state: "cancelling", stateText: "Cancelling...", cancellable: false });

    // the terminal result wins over every in-progress collection, cancelling included
    const terminal = deriveTimelineNodes(
      [node("a")],
      [ref("a")],
      [ref("a")],
      [cancelled("a")],
      new Set(["a"])
    );
    expect(terminal[0]).toMatchObject({
      state: "cancelled",
      stateText: null,
      detail: "Cancelled",
      terminal: true,
      cancellable: false,
    });
  });

  it("the terminal text comes from the result's own probes, never from a running row's node", () => {
    const views = deriveTimelineNodes(
      [node("a", installed("9.9", 0)), node("b")],
      [],
      [],
      [unchanged("a", "1.2.3"), changed("b", "1.2.3", "1.2.4")]
    );
    // the node's `installBefore` (9.9) is NOT the source: the result's own probe is
    expect(views[0].detail).toBe("1.2.3 (Nothing to update)");
    expect(views[1].detail).toBe("Ready - 1.2.3 -> 1.2.4");
  });

  it("appends running, verifying and finished commands absent from nodes, in that order, never twice", () => {
    const views = deriveTimelineNodes(
      [node("a")],
      [ref("b"), ref("a")],
      [ref("v"), ref("b")],
      [ok("c"), ok("a"), failed("b", "exit code 2")]
    );
    expect(views.map((view) => view.command)).toEqual(["a", "b", "v", "c"]);
    expect(views[0]).toMatchObject({ state: "ok", updateCommands: ["a update"] });
    // a finished command wins over its running AND verifying entry, exactly as for a pass node
    expect(views[1]).toMatchObject({
      label: "B",
      state: "failed",
      updateCommands: [],
      detail: "Failed - exit code 2",
    });
    expect(views[2]).toMatchObject({ label: "V", state: "verifying", stateText: "Verifying...", detail: null });
    expect(views[3]).toMatchObject({ label: "C", state: "ok", updateCommands: [] });

    const running = deriveTimelineNodes([], [ref("x")], [], []);
    expect(running).toEqual([
      {
        command: "x",
        label: "X",
        updateCommands: [],
        state: "updating",
        stateText: "Updating...",
        detail: null,
        detailTitle: null,
        terminal: false,
        cancellable: true,
      },
    ]);
  });

  it("deriveTimelineHeader counts every terminal row as done and only `failed` as failed", () => {
    const pending = deriveTimelineNodes([node("a"), node("b"), node("c")], [], [], []);
    expect(deriveTimelineHeader(pending)).toEqual({
      total: 3,
      done: 0,
      failed: 0,
      percent: 0,
      text: "0 of 3 completed",
    });

    const oneFailed = deriveTimelineNodes(
      [node("a"), node("b"), node("c")],
      [ref("c")],
      [],
      [ok("a"), failed("b", "exit code 1")]
    );
    expect(deriveTimelineHeader(oneFailed)).toEqual({
      total: 3,
      done: 2,
      failed: 1,
      percent: 67,
      text: "2 of 3 completed, 1 failed",
    });

    const twoFailed = deriveTimelineNodes(
      [node("a"), node("b"), node("c")],
      [],
      [],
      [ok("a"), failed("b", "exit code 1"), failed("c", "exit code 1")]
    );
    expect(deriveTimelineHeader(twoFailed)).toEqual({
      total: 3,
      done: 3,
      failed: 2,
      percent: 100,
      text: "3 of 3 completed, 2 failed",
    });

    // #1691 - a cancelled row is done and never failed; a verifying row is neither
    const cancelledAndVerifying = deriveTimelineNodes(
      [node("a"), node("b"), node("c"), node("d")],
      [],
      [ref("c")],
      [cancelled("a"), failed("b", "exit code 1")],
      new Set(["c"])
    );
    expect(deriveTimelineHeader(cancelledAndVerifying)).toEqual({
      total: 4,
      done: 2,
      failed: 1,
      percent: 50,
      text: "2 of 4 completed, 1 failed",
    });

    // only cancellations: done, and no failure clause at all
    const allCancelled = deriveTimelineNodes(
      [node("a"), node("b")],
      [],
      [],
      [cancelled("a"), cancelled("b")]
    );
    expect(deriveTimelineHeader(allCancelled)).toEqual({
      total: 2,
      done: 2,
      failed: 0,
      percent: 100,
      text: "2 of 2 completed",
    });

    const oneOfThree = deriveTimelineNodes([node("a"), node("b"), node("c")], [], [], [ok("a")]);
    expect(deriveTimelineHeader(oneOfThree).percent).toBe(33);

    expect(deriveTimelineHeader([])).toEqual({
      total: 0,
      done: 0,
      failed: 0,
      percent: 0,
      text: "0 of 0 completed",
    });
  });
});
