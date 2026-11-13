import { describe, expect, it } from "vitest";
import { readdirSync, readFileSync } from "node:fs";

// Same narrow URL overload as src/shared/raw-html-sinks.test.ts (no @types/node, #1167).
declare module "node:fs" {
  export function readdirSync(path: URL, o: { recursive: true; encoding: "utf8" }): string[];
}

/** #4330 - least-privilege Tauri capabilities: one file per window type. The expected map
 *  mirrors the call-site evidence in the #4330 plan. Change both together. */
type Capability = { identifier: string; windows: string[]; permissions: string[] };

const DIR = new URL("../../src-tauri/capabilities/", import.meta.url);
const w = (name: string) => `core:window:allow-${name}`;
const ZOOM = "core:webview:allow-set-webview-zoom";
const MAX = [w("maximize"), w("unmaximize")];

const EXPECTED: Record<string, { windows: string[]; permissions: string[] }> = {
  main: {
    windows: ["main"],
    permissions: [
      "core:default", w("close"), w("destroy"), w("set-focus"),
      w("request-user-attention"), w("minimize"), ...MAX, w("set-always-on-top"),
      w("start-dragging"), w("set-position"), w("set-size"), ZOOM, "dialog:allow-open",
    ],
  },
  "terminal-detached": {
    windows: ["terminal-*"],
    permissions: [
      "core:default", w("close"), w("destroy"), w("minimize"), ...MAX,
      w("start-dragging"), ZOOM,
    ],
  },
  "spec-board": {
    windows: ["spec-board"],
    permissions: ["core:default", w("close"), w("destroy"), w("minimize"), w("start-dragging")],
  },
  "resource-monitor": {
    windows: ["resource-monitor"],
    permissions: ["core:default", w("close"), w("minimize"), ...MAX, w("start-dragging")],
  },
  watchers: {
    windows: ["watchers"],
    permissions: ["core:default", w("close"), w("minimize"), w("start-dragging")],
  },
  "screenshot-overlay": {
    windows: ["screenshot-overlay-*"],
    permissions: ["core:event:default"],
  },
};

/** One sample label per window Rust builds (window.rs, geometry.rs, screenshot.rs). */
const CREATED_LABELS = [
  "main",
  "terminal-0123456789abcdef0123456789abcdef",
  "spec-board",
  "resource-monitor",
  "watchers",
  "screenshot-overlay-1-0",
];
/** Labels that must get no capability: web-browser loads an External URL (#4289); the
 *  unsuffixed sidebar/terminal windows are never created. */
const UNGRANTED_LABELS = ["web-browser", "sidebar", "terminal"];
const NEVER_GRANTED = [w("create"), w("set-title"), w("show"), w("hide"), w("unminimize")];

/** Every production `WebviewWindowBuilder::new(` per file. A new window builder fails this
 *  test until its label is added to CREATED_LABELS or UNGRANTED_LABELS and here. */
const BUILDERS: Record<string, number> = {
  "app_run/setup/geometry.rs": 1,
  "commands/screenshot.rs": 1,
  "commands/web_browser.rs": 1,
  "commands/window.rs": 5,
};
/** `?window=` routes in src/main.tsx; a new route needs a decision here. */
const ROUTES = ["terminal", "detached", "resource-monitor", "watchers", "screenshot-overlay",
  "spec-board"];
const RUST_SRC = new URL("../../src-tauri/src/", import.meta.url);

/** Relative file and folder paths under dir, recursive, with "/" (Windows gives "\\"). */
const walk = (dir: URL) =>
  readdirSync(dir, { recursive: true, encoding: "utf8" }).map((p) => p.replaceAll("\\", "/"));

function builderCounts(dir: URL): Record<string, number> {
  const out: Record<string, number> = {};
  for (const path of walk(dir)) {
    const parts = path.split("/");
    const name = parts[parts.length - 1];
    const isTest = name === "tests.rs" || name.endsWith("_tests.rs") || parts.includes("tests");
    if (!name.endsWith(".rs") || isTest) continue;
    const text = readFileSync(new URL(path, dir), "utf8");
    const n = text.split("WebviewWindowBuilder::new(").length - 1;
    if (n > 0) out[path] = n;
  }
  return out;
}

function matches(pattern: string, label: string): boolean {
  if (!pattern.endsWith("*")) return pattern === label;
  return label.startsWith(pattern.slice(0, -1));
}

const sorted = (items: string[]) => [...items].sort().join(",");

function problems(caps: Capability[]): string[] {
  const out: string[] = [];
  const grants = (label: string) =>
    caps.filter((c) => c.windows.some((p) => matches(p, label))).length;
  for (const label of CREATED_LABELS) {
    const n = grants(label);
    if (n !== 1) out.push(`${label}: matched by ${n} capabilities`);
  }
  for (const label of UNGRANTED_LABELS) {
    if (grants(label) > 0) out.push(`${label}: must not be granted`);
  }
  const ids = sorted(caps.map((c) => c.identifier));
  if (ids !== sorted(Object.keys(EXPECTED))) out.push(`identifiers: ${ids}`);
  for (const c of caps) {
    const want = EXPECTED[c.identifier];
    if (!want) continue;
    if (sorted(c.windows) !== sorted(want.windows)) out.push(`${c.identifier}: windows`);
    const got = sorted(c.permissions);
    if (got !== sorted(want.permissions)) out.push(`${c.identifier}: permissions ${got}`);
    for (const p of c.permissions) {
      if (NEVER_GRANTED.includes(p)) out.push(`${c.identifier}: forbidden ${p}`);
    }
  }
  return out;
}

function load(): Capability[] {
  return walk(DIR)
    .filter((name) => !name.includes("/") && name.endsWith(".json"))
    .map((name) => JSON.parse(readFileSync(new URL(name, DIR), "utf8")) as Capability);
}

function mutated(edit: (caps: Capability[]) => void): string[] {
  const caps = structuredClone(load());
  edit(caps);
  return problems(caps);
}

const byId = (caps: Capability[], id: string) => {
  const found = caps.find((c) => c.identifier === id);
  if (!found) throw new Error(`missing capability ${id}`);
  return found;
};

describe("#4330 Tauri capabilities per window", () => {
  it("positive control: the checked-in layout has no problems", () => {
    expect(problems(load())).toEqual([]);
  });

  it("every Rust window builder is accounted for", () => {
    expect(builderCounts(RUST_SRC)).toEqual(BUILDERS);
  });

  it("every ?window= route is accounted for", () => {
    const main = readFileSync(new URL("../main.tsx", import.meta.url), "utf8");
    const found = [...main.matchAll(/windowType === "([a-z-]+)"/g)].map((m) => m[1]);
    expect(sorted(found)).toBe(sorted(ROUTES));
  });

  it("overlay gets event IPC only", () => {
    expect(byId(load(), "screenshot-overlay").permissions).toEqual(["core:event:default"]);
  });

  it("negative: removing a needed permission fails", () => {
    const out = mutated((caps) => {
      const main = byId(caps, "main");
      main.permissions = main.permissions.filter((p) => p !== "dialog:allow-open");
    });
    expect(out).toContainEqual(expect.stringContaining("main: permissions"));
  });

  it("negative: an uncovered window pattern fails", () => {
    const out = mutated((caps) => {
      byId(caps, "watchers").windows = [];
    });
    expect(out).toContain("watchers: matched by 0 capabilities");
  });

  it("negative: granting the overlay an unneeded permission fails", () => {
    const out = mutated((caps) => {
      byId(caps, "screenshot-overlay").permissions.push(w("create"));
    });
    expect(out).toContain(`screenshot-overlay: forbidden ${w("create")}`);
  });

  it("negative: granting web-browser fails", () => {
    const out = mutated((caps) => {
      byId(caps, "watchers").windows.push("web-browser");
    });
    expect(out).toContain("web-browser: must not be granted");
  });

  it("negative: a window matched by two capabilities fails", () => {
    const out = mutated((caps) => {
      byId(caps, "main").windows.push("terminal-*");
    });
    expect(out).toContainEqual(expect.stringContaining("terminal-0123"));
  });
});
