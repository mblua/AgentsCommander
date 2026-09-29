// #2744 phase C - dark non-regression guard for the tokens defined in
// variables.css. Each of DEFINED used to be read as var(--x, <dark literal>)
// with no definition anywhere; its :root value is that same literal, which is
// only safe while every consumer passes it. A new consumer with a different or
// missing fallback would silently repaint dark mode, and fails here first.
// UNSAFE tokens have consumers that disagree or pass no fallback, so they must
// stay undefined.
//
// File listing uses import.meta.glob keys (never loaded) and reads bytes with
// the narrowed node:fs shim in src/vite-env.d.ts; @types/node is not a dependency.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

type Sheet = { path: string; text: string };

const ROOT = new URL("../../../", import.meta.url);
const VARIABLES_PATH = "src/sidebar/styles/variables.css";
const DEFINED = ["--btn-primary-bg", "--btn-primary-fg", "--sidebar-fg-muted", "--sidebar-warning"];
const UNSAFE = ["--text-muted", "--text-primary", "--fg-secondary", "--danger", "--status-error"];

// Keys are relative to this file; the loaders are never called.
const CSS_PATHS = Object.keys(import.meta.glob(["../../**/*.css"]))
  .map((key) => new URL(key, import.meta.url).href.slice(ROOT.href.length))
  .sort();

const norm = (s: string): string => s.replace(/\s+/g, " ").trim();
// Comments become spaces, keeping their newlines so reported lines stay true.
const stripComments = (s: string): string =>
  s.replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " "));

function rootValue(variablesCss: string, token: string): string | undefined {
  const root = /:root\s*\{([^}]*)\}/.exec(stripComments(variablesCss))?.[1] ?? "";
  const m = new RegExp(`(?:^|[;{\\s])${token}\\s*:\\s*([^;]+);`).exec(root);
  return m ? norm(m[1]) : undefined;
}

/** The fallback text of every var(--token ...) use, or null when it has none. */
function consumers(sheets: Sheet[], token: string): { site: string; fallback: string | null }[] {
  const out: { site: string; fallback: string | null }[] = [];
  // Whitespace, or a comment already blanked to spaces, may sit between var( and the name.
  const needle = new RegExp(`var\\(\\s*${token}(?![\\w-])`, "g");
  for (const { path, text } of sheets) {
    const body = stripComments(text);
    for (const m of body.matchAll(needle)) {
      const after = m.index + m[0].length;
      let depth = 1;
      let i = after;
      while (i < body.length && depth > 0) {
        if (body[i] === "(") depth++;
        else if (body[i] === ")") depth--;
        i++;
      }
      const args = body.slice(after, i - 1);
      const comma = args.indexOf(",");
      const line = body.slice(0, m.index).split("\n").length;
      out.push({ site: `${path}:${line}`, fallback: comma === -1 ? null : norm(args.slice(comma + 1)) });
    }
  }
  return out;
}

function mismatches(sheets: Sheet[], variablesCss: string, token: string): string[] {
  const expected = rootValue(variablesCss, token);
  return consumers(sheets, token)
    .filter((c) => c.fallback !== expected)
    .map((c) => `${c.site} ${token} fallback=${c.fallback ?? "<none>"} root=${expected ?? "<undefined>"}`);
}

function declares(sheets: Sheet[], token: string): string[] {
  const re = new RegExp(`(?:^|[;{\\s])${token}\\s*:`);
  return sheets.filter((s) => re.test(stripComments(s.text))).map((s) => s.path);
}

function loadSheets(): Sheet[] {
  return CSS_PATHS.map((path) => ({ path, text: readFileSync(new URL(path, ROOT), "utf8") }));
}

describe("token fallback parity (#2744 phase C)", () => {
  const sheets = loadSheets();
  const variablesCss = sheets.find((s) => s.path === VARIABLES_PATH)?.text ?? "";

  it("every defined token is passed the same fallback by every consumer", () => {
    expect(sheets.length).toBeGreaterThan(10);
    for (const token of DEFINED) {
      expect(rootValue(variablesCss, token), `${token} in :root`).toBeDefined();
      expect(consumers(sheets, token).length, `${token} consumers`).toBeGreaterThan(0);
      expect(mismatches(sheets, variablesCss, token)).toEqual([]);
    }
  });

  it("the five unsafe tokens stay undefined", () => {
    for (const token of UNSAFE) {
      expect(declares(sheets, token), `${token} declared`).toEqual([]);
    }
  });

  it("binds a synthetic mismatch", () => {
    const vars = ":root {\n  --zz-probe: #00d4ff;\n}\n";
    const fixture: Sheet[] = [
      { path: "a.css", text: ".ok { color: var(--zz-probe, #00d4ff); }" },
      { path: "b.css", text: ".bad {\n  color: var(--zz-probe, #ff0000);\n}" },
      { path: "c.css", text: ".bare { color: var(--zz-probe); }" },
      { path: "e.css", text: ".sp { color: var( --zz-probe, #ff0000); }" },
      { path: "f.css", text: "/* x\n */ .gap { color: var(/* gap */--zz-probe, #ff0000); }" },
      { path: "g.css", text: ".other { color: var(--zz-probe-2, #ff0000); }" },
    ];
    expect(mismatches(fixture, vars, "--zz-probe")).toEqual([
      "b.css:2 --zz-probe fallback=#ff0000 root=#00d4ff",
      "c.css:1 --zz-probe fallback=<none> root=#00d4ff",
      "e.css:1 --zz-probe fallback=#ff0000 root=#00d4ff",
      "f.css:2 --zz-probe fallback=#ff0000 root=#00d4ff",
    ]);
    expect(mismatches(fixture.slice(0, 1), vars, "--zz-probe")).toEqual([]);
    expect(declares([{ path: "d.css", text: ":root { --zz-probe: red; }" }], "--zz-probe")).toEqual(["d.css"]);
  });
});
