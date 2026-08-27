import { describe, expect, it } from "vitest";
import type {
  AgentUpdateCommandRef,
  AgentUpdateNode,
  AgentUpdateOverviewRow,
  AgentUpdateResult,
  InstallState,
} from "../shared/types";
import {
  CONFIGURED_LABELS,
  LIVE_LABELS,
  NODE_STATE_LABELS,
  NOT_INSTALLED_LABEL,
  UNKNOWN_ERROR_LABEL,
  VERSION_MISSING_LABEL,
  VERSION_UNDETECTED_LABEL,
  configuredState,
  deriveAutoUpdateRows,
  deriveTimelineHeader,
  deriveTimelineNodes,
  describeInstall,
  installedView,
  liveView,
  versionTransitionText,
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

function ok(command: string): AgentUpdateResult {
  return { command, label: command.toUpperCase(), ok: true };
}

function failed(command: string, error?: string): AgentUpdateResult {
  return error === undefined
    ? { command, label: command.toUpperCase(), ok: false }
    : { command, label: command.toUpperCase(), ok: false, error };
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

describe("timeline derivations (#1551 round 5)", () => {
  it("describeInstall claims a version only when one was read", () => {
    expect(describeInstall(installed("1.0"))).toBe("1.0");
    expect(describeInstall(missing())).toBe(VERSION_MISSING_LABEL);
    expect(describeInstall(probeFailed())).toBe(VERSION_UNDETECTED_LABEL);
    expect(describeInstall(unprobed())).toBeNull();
    expect(describeInstall(checking)).toBeNull();
    expect(describeInstall({ status: "installed", seq: 1 })).toBeNull();
    expect(describeInstall(null)).toBeNull();
    expect(describeInstall(undefined)).toBeNull();
    expect(VERSION_MISSING_LABEL).toBe("no instalada");
    expect(VERSION_UNDETECTED_LABEL).toBe("versión no detectada");
  });

  it("versionTransitionText prints both, one, or nothing, never an invented value", () => {
    expect(versionTransitionText(installed("1.0"), installed("1.1"))).toBe("1.0 → 1.1");
    expect(versionTransitionText(installed("1.1"), installed("1.1"))).toBe("1.1 → 1.1");
    expect(versionTransitionText(installed("1.0"), missing())).toBe("1.0 → no instalada");
    expect(versionTransitionText(missing(), installed("1.1"))).toBe("no instalada → 1.1");
    expect(versionTransitionText(installed("1.0"), undefined)).toBe("1.0");
    expect(versionTransitionText(probeFailed(), installed("1.1"))).toBe("versión no detectada → 1.1");
    expect(versionTransitionText(unprobed(), unprobed())).toBeNull();
    expect(versionTransitionText(undefined, undefined)).toBeNull();
    expect(versionTransitionText(undefined, installed("1.1"))).toBe("1.1");
  });

  it("deriveTimelineNodes derives the state of every node in node order with the state texts", () => {
    const views = deriveTimelineNodes(
      [node("a"), node("b"), node("c"), node("d")],
      [ref("b")],
      [ok("a"), failed("d", "exit code 1")],
      {}
    );
    expect(views.map((view) => view.command)).toEqual(["a", "b", "c", "d"]);
    expect(views.map((view) => view.state)).toEqual(["ok", "updating", "pending", "failed"]);
    expect(views.map((view) => view.stateText)).toEqual(["Listo", "Actualizando...", "Pendiente", "Falló"]);
    expect(NODE_STATE_LABELS).toEqual({
      pending: "Pendiente",
      updating: "Actualizando...",
      ok: "Listo",
      failed: "Falló",
    });
    expect(views[1]).toMatchObject({ label: "B", updateCommands: ["b update"], detail: null, detailTitle: null });
    expect(views[2]).toMatchObject({ detail: null, detailTitle: null });
  });

  it("detail: the version transition for ok, the reason plus the transition for failed, title equal to detail", () => {
    const views = deriveTimelineNodes(
      [
        node("a", installed("1.0", 0)),
        node("b"),
        node("c", installed("1.0", 0)),
        node("d"),
        node("e"),
      ],
      [],
      [ok("a"), ok("b"), failed("c", "exit code 1"), failed("d", "exit code 1"), failed("e")],
      { a: installed("1.1", 3), c: missing("'c' was not found on PATH", 4) }
    );
    const byCommand = Object.fromEntries(views.map((view) => [view.command, view]));
    // ok with a version text
    expect(byCommand.a.detail).toBe("1.0 → 1.1");
    // ok without any version source: no detail (the T4 stubs)
    expect(byCommand.b.detail).toBeNull();
    // failed with a version text: the 2026-08-25 incident line
    expect(byCommand.c.detail).toBe("exit code 1 · 1.0 → no instalada");
    // failed without a version text
    expect(byCommand.d.detail).toBe("exit code 1");
    // failed without a reason (practically unreachable)
    expect(byCommand.e.detail).toBe(UNKNOWN_ERROR_LABEL);
    for (const view of views) expect(view.detailTitle).toBe(view.detail);
  });

  it("appends running and finished commands absent from nodes, in that order, never twice", () => {
    const views = deriveTimelineNodes(
      [node("a")],
      [ref("b"), ref("a")],
      [ok("c"), ok("a"), failed("b", "exit code 2")],
      {}
    );
    expect(views.map((view) => view.command)).toEqual(["a", "b", "c"]);
    expect(views[0]).toMatchObject({ state: "ok", updateCommands: ["a update"] });
    // a finished command wins over its running entry, exactly as for a pass node
    expect(views[1]).toMatchObject({ label: "B", state: "failed", updateCommands: [], detail: "exit code 2" });
    expect(views[2]).toMatchObject({ label: "C", state: "ok", updateCommands: [], detail: null });

    const running = deriveTimelineNodes([], [ref("x")], [], {});
    expect(running).toEqual([
      {
        command: "x",
        label: "X",
        updateCommands: [],
        state: "updating",
        stateText: "Actualizando...",
        detail: null,
        detailTitle: null,
      },
    ]);
  });

  it("deriveTimelineHeader counts done and failed, rounds the percent and pluralizes the failures", () => {
    const pending = deriveTimelineNodes([node("a"), node("b"), node("c")], [], [], {});
    expect(deriveTimelineHeader(pending)).toEqual({
      total: 3,
      done: 0,
      failed: 0,
      percent: 0,
      text: "0 de 3 completados",
    });

    const oneFailed = deriveTimelineNodes(
      [node("a"), node("b"), node("c")],
      [ref("c")],
      [ok("a"), failed("b", "exit code 1")],
      {}
    );
    expect(deriveTimelineHeader(oneFailed)).toEqual({
      total: 3,
      done: 2,
      failed: 1,
      percent: 67,
      text: "2 de 3 completados · 1 falló",
    });

    const twoFailed = deriveTimelineNodes(
      [node("a"), node("b"), node("c")],
      [],
      [ok("a"), failed("b", "exit code 1"), failed("c", "exit code 1")],
      {}
    );
    expect(deriveTimelineHeader(twoFailed)).toEqual({
      total: 3,
      done: 3,
      failed: 2,
      percent: 100,
      text: "3 de 3 completados · 2 fallaron",
    });

    const oneOfThree = deriveTimelineNodes([node("a"), node("b"), node("c")], [], [ok("a")], {});
    expect(deriveTimelineHeader(oneOfThree).percent).toBe(33);

    expect(deriveTimelineHeader([])).toEqual({
      total: 0,
      done: 0,
      failed: 0,
      percent: 0,
      text: "0 de 0 completados",
    });
  });
});
