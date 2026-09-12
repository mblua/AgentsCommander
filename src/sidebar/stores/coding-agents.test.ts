import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FakeTransport } from "../../shared/testing/fake-transport";
import { __setTransportForTests } from "../../shared/ipc";
import type {
  CatalogDiagnostic,
  CatalogReport,
  CodingAgentDefinition,
} from "../../shared/types";
import { codingAgentsStore } from "./coding-agents";

const REPORT_CMD = "get_coding_agent_catalog_report";
const LIST_CMD = "list_reseedable_agent_commands";

function def(key: string, command = key): CodingAgentDefinition {
  return {
    key,
    label: key,
    description: `by ${key}`,
    color: "#123456",
    command,
    envs: [],
    isolatedHome: false,
    removable: true,
    updateCommands: [],
    autoUpdate: false,
  };
}

function warning(code: string, path: string, reason: string): CatalogDiagnostic {
  return { code, path, reason };
}

/** A success report carries all five fields; overrides exercise the rest. */
function report(overrides: Partial<CatalogReport> = {}): CatalogReport {
  return {
    primaryProjectRoot: null,
    sourcePath: null,
    catalog: [],
    warnings: [],
    unavailable: null,
    ...overrides,
  };
}

interface Deferred<T> {
  promise: Promise<T>;
  resolve: (value: T) => void;
  reject: (reason?: unknown) => void;
}

function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function tick(): Promise<void> {
  return new Promise<void>((resolve) => setTimeout(resolve, 0));
}

/** Per-call deferreds: request N resolves through handles[N]. */
function deferredHandler<T>(handles: Deferred<T>[]) {
  let index = 0;
  return () => {
    const handle = handles[index];
    index += 1;
    if (!handle) throw new Error("unexpected extra request");
    return handle.promise;
  };
}

describe("codingAgentsStore (#1965 catalog report)", () => {
  let fake: FakeTransport;

  beforeEach(() => {
    codingAgentsStore.resetForTests();
    fake = new FakeTransport();
    __setTransportForTests(fake);
    // Default the master-list fetch so catalog-focused cases do not hit an
    // unhandled invoke; individual tests override it.
    fake.resolve(LIST_CMD, []);
  });

  afterEach(() => {
    codingAgentsStore.resetForTests();
    vi.restoreAllMocks();
  });

  it("starts empty and unloaded — no selectable bundled fallback", () => {
    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.loaded()).toBe(false);
    expect(codingAgentsStore.loading()).toBe(false);
    expect(codingAgentsStore.error()).toBeNull();
    expect(codingAgentsStore.warnings()).toEqual([]);
    expect(codingAgentsStore.sourcePath()).toBeNull();
    expect(codingAgentsStore.reseedableCommands()).toEqual([]);
  });

  it("publishes catalog, source path, warnings and the reseedable set on success", async () => {
    fake.resolve(
      REPORT_CMD,
      report({
        primaryProjectRoot: "C:/repo/app",
        sourcePath: "C:/repo/app/.ac/coding-agents/agents.json",
        catalog: [def("claude"), def("codex")],
        warnings: [
          warning(
            "local-overlay-invalid",
            "C:/repo/app/.ac/coding-agents/agents.local.json",
            "unknown field",
          ),
        ],
      }),
    );
    fake.resolve(LIST_CMD, ["claude", "codex"]);

    await codingAgentsStore.ensureLoaded();

    expect(codingAgentsStore.catalog()).toEqual([def("claude"), def("codex")]);
    expect(codingAgentsStore.loaded()).toBe(true);
    expect(codingAgentsStore.loading()).toBe(false);
    expect(codingAgentsStore.error()).toBeNull();
    expect(codingAgentsStore.warnings()).toEqual([
      warning(
        "local-overlay-invalid",
        "C:/repo/app/.ac/coding-agents/agents.local.json",
        "unknown field",
      ),
    ]);
    expect(codingAgentsStore.sourcePath()).toBe("C:/repo/app/.ac/coding-agents/agents.json");
    expect(codingAgentsStore.reseedableCommands()).toEqual(["claude", "codex"]);
  });

  it("treats a valid empty catalog as a successful load, never a fallback", async () => {
    fake.resolve(
      REPORT_CMD,
      report({ primaryProjectRoot: null, sourcePath: null, catalog: [], warnings: [] }),
    );

    await codingAgentsStore.ensureLoaded();

    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.loaded()).toBe(true);
    expect(codingAgentsStore.error()).toBeNull();
  });

  it("publishes an unavailable report as a diagnostic error and keeps loaded=false", async () => {
    fake.resolve(
      REPORT_CMD,
      report({
        primaryProjectRoot: "C:/repo/app",
        sourcePath: "C:/repo/app/.ac/coding-agents/agents.json",
        catalog: [],
        unavailable: warning(
          "baseUnavailable",
          "C:/repo/app/.ac/coding-agents/agents.json",
          "corrupt bytes",
        ),
      }),
    );

    await codingAgentsStore.ensureLoaded();

    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.loaded()).toBe(false);
    expect(codingAgentsStore.error()).toEqual(
      warning("baseUnavailable", "C:/repo/app/.ac/coding-agents/agents.json", "corrupt bytes"),
    );
    expect(codingAgentsStore.sourcePath()).toBe("C:/repo/app/.ac/coding-agents/agents.json");
  });

  it("keeps the catalog when only the master list fails, and publishes that diagnostic", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: null, catalog: [def("claude")] }));
    fake.reject(LIST_CMD, "reseedable dead");

    await codingAgentsStore.ensureLoaded();

    expect(codingAgentsStore.catalog()).toEqual([def("claude")]);
    expect(codingAgentsStore.loaded()).toBe(true);
    expect(codingAgentsStore.error()).toBeNull();
    expect(codingAgentsStore.reseedableCommands()).toEqual([]); // controls disabled
    const diagnostic = codingAgentsStore.warnings()[0];
    expect(diagnostic?.code).toBe("reseedable-unavailable");
    expect(diagnostic?.reason).toContain("reseedable dead");
  });

  it("captures a transport failure as a diagnostic and retries on the next load", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const reports = [deferred<CatalogReport>()];
    fake.onInvoke(REPORT_CMD, deferredHandler(reports));

    reports[0].reject("config-dir failure");
    await expect(codingAgentsStore.ensureLoaded()).resolves.toBeUndefined();
    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.loaded()).toBe(false);
    expect(codingAgentsStore.error()?.code).toBe("transport-error");
    expect(codingAgentsStore.error()?.reason).toContain("config-dir failure");

    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: null, catalog: [def("claude")] }));
    await codingAgentsStore.ensureLoaded();
    expect(codingAgentsStore.catalog()).toEqual([def("claude")]);
    expect(codingAgentsStore.loaded()).toBe(true);
    expect(codingAgentsStore.error()).toBeNull();
  });

  it("absorbs a synchronous throw from the report wrapper (never rejects)", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    fake.onInvoke(REPORT_CMD, () => {
      throw "sync-boom";
    });

    await expect(codingAgentsStore.ensureLoaded()).resolves.toBeUndefined();

    expect(codingAgentsStore.loaded()).toBe(false);
    expect(codingAgentsStore.error()?.reason).toContain("sync-boom");
  });

  it("both endpoint failures leave an unusable catalog with diagnostics", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    fake.reject(REPORT_CMD, "report dead");
    fake.reject(LIST_CMD, "list dead");

    await codingAgentsStore.ensureLoaded();

    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.reseedableCommands()).toEqual([]);
    expect(codingAgentsStore.loaded()).toBe(false);
    expect(codingAgentsStore.error()?.code).toBe("transport-error");
    expect(codingAgentsStore.error()?.reason).toContain("report dead");
  });

  it("dedups concurrent loads into one request per generation", async () => {
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: null, catalog: [def("claude")] }));

    await Promise.all([
      codingAgentsStore.ensureLoaded(),
      codingAgentsStore.ensureLoaded(),
      codingAgentsStore.ensureLoaded(),
    ]);

    expect(fake.callsFor(REPORT_CMD).length).toBe(1);
    expect(codingAgentsStore.catalog()).toEqual([def("claude")]);
  });

  it("does not refetch once loaded", async () => {
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: null, catalog: [def("claude")] }));
    await codingAgentsStore.ensureLoaded();
    await codingAgentsStore.ensureLoaded();
    expect(fake.callsFor(REPORT_CMD).length).toBe(1);
  });

  it("switching primary: a slow A report cannot overwrite the newer B report", async () => {
    const reports = [deferred<CatalogReport>(), deferred<CatalogReport>()];
    const lists = [deferred<string[]>(), deferred<string[]>()];
    fake.onInvoke(REPORT_CMD, deferredHandler(reports));
    fake.onInvoke(LIST_CMD, deferredHandler(lists));

    const first = codingAgentsStore.setPrimaryProject("C:/repo/A");
    await tick();
    const second = codingAgentsStore.setPrimaryProject("C:/repo/B");
    await tick();

    reports[1].resolve(report({ primaryProjectRoot: "C:/repo/B", catalog: [def("bravo")] }));
    lists[1].resolve(["bravo"]);
    await second;
    reports[0].resolve(report({ primaryProjectRoot: "C:/repo/A", catalog: [def("alpha")] }));
    lists[0].resolve(["alpha"]);
    await first;

    expect(codingAgentsStore.catalog()).toEqual([def("bravo")]);
    expect(codingAgentsStore.reseedableCommands()).toEqual(["bravo"]);
    expect(codingAgentsStore.error()).toBeNull();
    expect(codingAgentsStore.loaded()).toBe(true);
  });

  it("a superseded rejection cannot overwrite the current generation's state", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const reports = [deferred<CatalogReport>(), deferred<CatalogReport>()];
    fake.onInvoke(REPORT_CMD, deferredHandler(reports));

    const first = codingAgentsStore.setPrimaryProject("C:/repo/A");
    await tick();
    const second = codingAgentsStore.setPrimaryProject("C:/repo/B");
    await tick();

    reports[1].resolve(report({ primaryProjectRoot: "C:/repo/B", catalog: [def("bravo")] }));
    await second;
    reports[0].reject("A transport dead");
    await first;

    expect(codingAgentsStore.catalog()).toEqual([def("bravo")]);
    expect(codingAgentsStore.error()).toBeNull();
    expect(codingAgentsStore.loaded()).toBe(true);
  });

  it("reload during a request discards the stale result and adopts the reloaded identity", async () => {
    const reports = [deferred<CatalogReport>(), deferred<CatalogReport>()];
    fake.onInvoke(REPORT_CMD, deferredHandler(reports));

    const mountLoad = codingAgentsStore.ensureLoaded();
    await tick();
    const reload = codingAgentsStore.refresh();
    await tick();

    reports[1].resolve(report({ primaryProjectRoot: "C:/repo/B", catalog: [def("bravo")] }));
    await reload;
    reports[0].resolve(report({ primaryProjectRoot: "C:/repo/A", catalog: [def("alpha")] }));
    await mountLoad;

    expect(codingAgentsStore.catalog()).toEqual([def("bravo")]);
    expect(codingAgentsStore.error()).toBeNull();
    expect(codingAgentsStore.loaded()).toBe(true);
    expect(codingAgentsStore.loading()).toBe(false);
  });

  it("a superseded completion cannot finalize the owning generation's loading or in-flight slot", async () => {
    const reports = [deferred<CatalogReport>(), deferred<CatalogReport>()];
    fake.onInvoke(REPORT_CMD, deferredHandler(reports));

    const superseded = codingAgentsStore.ensureLoaded(); // generation N
    await tick();
    const owning = codingAgentsStore.refresh(); // generation N+1
    await tick();
    expect(codingAgentsStore.loading()).toBe(true);

    // The abandoned first-generation request settles while its successor is
    // still in flight: it must not touch loading or the in-flight slot.
    reports[0].reject("superseded transport failure");
    await superseded;
    expect(codingAgentsStore.loading()).toBe(true);

    const callsBefore = fake.callsFor(REPORT_CMD).length;
    const follower = codingAgentsStore.ensureLoaded();
    await tick();
    // It joined the owning generation's single request instead of starting a
    // second one (the in-flight slot survived the superseded finalizer).
    expect(fake.callsFor(REPORT_CMD).length).toBe(callsBefore);

    reports[1].resolve(report({ primaryProjectRoot: null, catalog: [def("bravo")] }));
    await Promise.all([owning, follower]);

    expect(codingAgentsStore.loading()).toBe(false);
    expect(codingAgentsStore.catalog()).toEqual([def("bravo")]);
    expect(codingAgentsStore.error()).toBeNull();
  });

  it("repeated refresh starts a new generation each time and only the last publishes", async () => {
    const reports = [
      deferred<CatalogReport>(),
      deferred<CatalogReport>(),
      deferred<CatalogReport>(),
    ];
    fake.onInvoke(REPORT_CMD, deferredHandler(reports));

    const startGeneration = codingAgentsStore.generation();
    const first = codingAgentsStore.refresh();
    await tick();
    const second = codingAgentsStore.refresh();
    await tick();
    const third = codingAgentsStore.refresh();
    await tick();

    expect(codingAgentsStore.generation()).toBe(startGeneration + 3);
    expect(fake.callsFor(REPORT_CMD).length).toBe(3);

    reports[2].resolve(report({ primaryProjectRoot: null, catalog: [def("charlie")] }));
    await third;
    reports[1].resolve(report({ primaryProjectRoot: null, catalog: [def("bravo")] }));
    await second;
    reports[0].resolve(report({ primaryProjectRoot: null, catalog: [def("alpha")] }));
    await first;

    expect(codingAgentsStore.catalog()).toEqual([def("charlie")]);
    expect(codingAgentsStore.loaded()).toBe(true);
    expect(codingAgentsStore.loading()).toBe(false);
  });

  it("refresh clears selectable state immediately and publishes only its own result", async () => {
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: "C:/repo/A", catalog: [def("alpha")] }));
    await codingAgentsStore.ensureLoaded();
    expect(codingAgentsStore.catalog()).toEqual([def("alpha")]);
    expect(codingAgentsStore.loaded()).toBe(true);

    const reports = [deferred<CatalogReport>()];
    fake.onInvoke(REPORT_CMD, deferredHandler(reports));
    const reload = codingAgentsStore.refresh();

    // The moment the reload starts, nothing stale stays selectable.
    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.reseedableCommands()).toEqual([]);
    expect(codingAgentsStore.loaded()).toBe(false);
    expect(codingAgentsStore.loading()).toBe(true);

    reports[0].resolve(report({ primaryProjectRoot: "C:/repo/B", catalog: [def("bravo")] }));
    await reload;
    expect(codingAgentsStore.loading()).toBe(false);
    expect(codingAgentsStore.catalog()).toEqual([def("bravo")]);
    expect(codingAgentsStore.error()).toBeNull();
  });

  it("resetForTests during a request invalidates the outstanding completion", async () => {
    const reports = [deferred<CatalogReport>()];
    fake.onInvoke(REPORT_CMD, deferredHandler(reports));

    const load = codingAgentsStore.ensureLoaded();
    await tick();
    codingAgentsStore.resetForTests();
    reports[0].resolve(report({ primaryProjectRoot: "C:/repo/A", catalog: [def("alpha")] }));
    await load;

    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.loaded()).toBe(false);
    expect(codingAgentsStore.loading()).toBe(false);
  });

  it("discards a report for another project and shows a source-changed retry state", async () => {
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: "C:/repo/B", catalog: [def("bravo")] }));

    await codingAgentsStore.setPrimaryProject("C:/repo/A");

    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.reseedableCommands()).toEqual([]);
    expect(codingAgentsStore.loaded()).toBe(false);
    expect(codingAgentsStore.error()?.code).toBe("primary-project-changed");
    expect(codingAgentsStore.error()?.path).toBe("C:/repo/B");
    expect(codingAgentsStore.error()?.reason).toContain("C:/repo/A");
  });

  it("adopts the first completed report's identity and enforces it on later loads", async () => {
    const reports = [deferred<CatalogReport>(), deferred<CatalogReport>()];
    fake.onInvoke(REPORT_CMD, deferredHandler(reports));

    reports[0].resolve(
      report({
        primaryProjectRoot: "C:/repo/A",
        unavailable: warning("baseUnavailable", "C:/repo/A/agents.json", "no usable base"),
      }),
    );
    await codingAgentsStore.ensureLoaded();
    expect(codingAgentsStore.loaded()).toBe(false);

    // The identity adopted from the first report is now required for background loads.
    const second = codingAgentsStore.ensureLoaded();
    await tick();
    reports[1].resolve(report({ primaryProjectRoot: "C:/repo/B", catalog: [def("bravo")] }));
    await second;

    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.error()?.code).toBe("primary-project-changed");
  });

  it("null is a real no-project identity, distinct from a path", async () => {
    // A path-shaped report never satisfies the null identity.
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: "C:/repo/A", catalog: [def("alpha")] }));
    await codingAgentsStore.setPrimaryProject(null);
    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.error()?.code).toBe("primary-project-changed");

    // After another primary, an explicit switch to null must fetch and match.
    await codingAgentsStore.setPrimaryProject("C:/repo/B");
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: null, catalog: [def("alpha")] }));
    await codingAgentsStore.setPrimaryProject(null);
    expect(codingAgentsStore.catalog()).toEqual([def("alpha")]);
    expect(codingAgentsStore.error()).toBeNull();
    expect(codingAgentsStore.loaded()).toBe(true);
  });

  it("setPrimaryProject is a no-op when the initialized identity is unchanged", async () => {
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: "C:/repo/A", catalog: [def("alpha")] }));
    await codingAgentsStore.setPrimaryProject("C:/repo/A");
    const calls = fake.callsFor(REPORT_CMD).length;
    const gen = codingAgentsStore.generation();

    await codingAgentsStore.setPrimaryProject("c:/REPO/a/");

    expect(fake.callsFor(REPORT_CMD).length).toBe(calls);
    expect(codingAgentsStore.generation()).toBe(gen);
  });

  it("matches equivalent Windows aliases: slash, case, trailing and verbatim forms", async () => {
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: "c:/repo/app/", catalog: [def("alpha")] }));
    await codingAgentsStore.setPrimaryProject("C:\\Repo\\App");
    expect(codingAgentsStore.catalog()).toEqual([def("alpha")]);
    expect(codingAgentsStore.error()).toBeNull();

    codingAgentsStore.resetForTests();
    fake.resolve(
      REPORT_CMD,
      report({ primaryProjectRoot: "\\\\?\\C:\\Repo\\App", catalog: [def("bravo")] }),
    );
    await codingAgentsStore.setPrimaryProject("C:/Repo/App");
    expect(codingAgentsStore.catalog()).toEqual([def("bravo")]);
    expect(codingAgentsStore.error()).toBeNull();

    codingAgentsStore.resetForTests();
    fake.resolve(
      REPORT_CMD,
      report({ primaryProjectRoot: "\\\\server\\share\\Proj", catalog: [def("charlie")] }),
    );
    await codingAgentsStore.setPrimaryProject("\\\\?\\UNC\\server\\share\\Proj\\");
    expect(codingAgentsStore.catalog()).toEqual([def("charlie")]);
    expect(codingAgentsStore.error()).toBeNull();
  });

  it("keeps device markers distinct from the plain drive shape", async () => {
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: "C:/Repo/App", catalog: [def("alpha")] }));

    await codingAgentsStore.setPrimaryProject("\\\\.\\C:\\Repo\\App");

    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.error()?.code).toBe("primary-project-changed");
  });

  it("POSIX roots preserve case and literal backslashes but trim trailing slashes", async () => {
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: "/repo/app/", catalog: [def("alpha")] }));
    await codingAgentsStore.setPrimaryProject("/repo/app");
    expect(codingAgentsStore.catalog()).toEqual([def("alpha")]);

    // Case differs → a different project on a case-sensitive filesystem.
    codingAgentsStore.resetForTests();
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: "/repo/APP", catalog: [def("bravo")] }));
    await codingAgentsStore.setPrimaryProject("/repo/app/");
    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.error()?.code).toBe("primary-project-changed");

    // Two slash-separated POSIX paths that differ only by a literal backslash.
    codingAgentsStore.resetForTests();
    fake.resolve(REPORT_CMD, report({ primaryProjectRoot: "/repo/a/b", catalog: [def("charlie")] }));
    await codingAgentsStore.setPrimaryProject("/repo/a\\b/");
    expect(codingAgentsStore.catalog()).toEqual([]);
    expect(codingAgentsStore.error()?.code).toBe("primary-project-changed");
  });
});
