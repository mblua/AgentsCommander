// #2746 (epic #2744) - generic guard: no CSS rule may be readable in dark mode
// and unreadable in light mode.
// Origin defect: #2744 (light-theme text resolved to dark-theme colors, e.g. a
// `rgba(255, 255, 255, 0.5)` fallback on the #f5f5f7 light surface).
//
// For every leaf rule of `src/**/*.css` that declares a resolvable `color`, the
// WCAG 2.1 contrast ratio is computed in both modes: tokens come from the two
// variables.css files (LIGHT = DARK overlaid with `html.light-theme`), the
// rule background is composited over --sidebar-bg, and the text over that.
// A `html.light-theme` override applies only when it names EVERY part of the
// base rule's selector list. Violation: light < 3.0 and dark >= 3.0.
// Violations need an allowlist entry (matched by file + normalised selector,
// never by line); the allowlist is two-way (an entry with no violation fails
// as stale).
//
// File listing uses import.meta.glob keys (never loaded) and reads bytes with
// the narrowed node:fs shim in src/vite-env.d.ts; @types/node is not a dependency.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

type Rgba = [number, number, number, number];
type Sheet = { path: string; text: string };
type Entry = { file: string; line: number; selector: string; reason: string };
type Allowlist = { version: number; entries: Entry[] };
type Violation = {
  file: string;
  line: number;
  selector: string;
  light: number;
  dark: number;
  color: string;
  background: string | null;
};
type Paint = { color: string | null; background: string | null };

const ROOT = new URL("../../../", import.meta.url);
const ALLOWLIST_PATH = "src/shared/styles/theme-contrast.allowlist.json";
const SIDEBAR_VARS = "sidebar/styles/variables.css";
const TERMINAL_VARS = "terminal/styles/variables.css";
const MIN_RATIO = 3.0;
const LIGHT_PREFIX = "html.light-theme";

// Keys are relative to this file; the loaders are never called.
const CSS_PATHS = Object.keys(import.meta.glob(["../../**/*.css"]))
  .map((key) => new URL(key, import.meta.url).href.slice(ROOT.href.length))
  .sort();

function readRoot(path: string): string {
  return readFileSync(new URL(path, ROOT), "utf8");
}

/** Replaces each comment with spaces of equal length (newlines kept). */
function stripComments(text: string): string {
  return text.replace(/\/\*[\s\S]*?\*\//g, (c) => c.replace(/[^\n]/g, " "));
}

function tokenBlock(text: string, block: RegExp): Record<string, string> {
  const out: Record<string, string> = {};
  const m = block.exec(stripComments(text));
  if (!m) return out;
  for (const decl of m[1].split(";")) {
    const i = decl.indexOf(":");
    if (i < 0) continue;
    const name = decl.slice(0, i).trim();
    if (name.startsWith("--")) out[name] = decl.slice(i + 1).trim();
  }
  return out;
}

const ROOT_BLOCK = /:root\s*\{([\s\S]*?)\}/;
const LIGHT_BLOCK = /html\.light-theme\s*\{([\s\S]*?)\}/;

function tokenMaps(sidebarVars: string, terminalVars: string) {
  const dark = { ...tokenBlock(sidebarVars, ROOT_BLOCK), ...tokenBlock(terminalVars, ROOT_BLOCK) };
  const light = {
    ...dark,
    ...tokenBlock(sidebarVars, LIGHT_BLOCK),
    ...tokenBlock(terminalVars, LIGHT_BLOCK),
  };
  return { dark, light };
}

function parseColor(raw: string | null): Rgba | null {
  if (raw === null) return null;
  const c = raw.trim().toLowerCase();
  let m: RegExpExecArray | null;
  if ((m = /^#([0-9a-f]{3})$/.exec(c))) {
    const [r, g, b] = [...m[1]].map((h) => parseInt(h + h, 16));
    return [r, g, b, 1];
  }
  if ((m = /^#([0-9a-f]{6})([0-9a-f]{2})?$/.exec(c))) {
    const hex = m[1];
    const alpha = m[2] === undefined ? 1 : parseInt(m[2], 16) / 255;
    return [0, 2, 4].map((i) => parseInt(hex.slice(i, i + 2), 16)).concat(alpha) as Rgba;
  }
  if ((m = /^rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)\s*(?:[,/]\s*([\d.]+%?))?\s*\)$/.exec(c))) {
    const a = m[4] === undefined ? 1 : m[4].endsWith("%") ? parseFloat(m[4]) / 100 : +m[4];
    return [+m[1], +m[2], +m[3], a];
  }
  if (c === "white") return [255, 255, 255, 1];
  if (c === "black") return [0, 0, 0, 1];
  if (c === "transparent") return [0, 0, 0, 0];
  return null;
}

/** Resolves a declaration value to rgba in `map`, or null (rule skipped). */
function resolve(value: string | null, map: Record<string, string>, depth = 0): Rgba | null {
  if (value === null || depth > 6) return null;
  const v = value.trim();
  const ref = /var\(\s*(--[\w-]+)\s*(?:,\s*([\s\S]+))?\)\s*$/.exec(v);
  if (ref) {
    const token = map[ref[1]];
    if (token !== undefined) return resolve(token, map, depth + 1);
    return ref[2] ? resolve(ref[2], map, depth + 1) : null;
  }
  // A shorthand such as `background: #fff url(x)` yields its color literal.
  const literal = /#[0-9a-fA-F]{3,8}|rgba?\([^)]*\)|\bwhite\b|\bblack\b|\btransparent\b/.exec(v);
  if (!literal) return null;
  if (/var\(/.test(v) && literal.index > v.indexOf("var(")) return null;
  return parseColor(literal[0]);
}

function over(top: Rgba | null, base: Rgba): Rgba {
  if (!top) return base;
  const a = top[3];
  return [0, 1, 2].map((i) => top[i] * a + base[i] * (1 - a)).concat(1) as Rgba;
}

function luminance(c: Rgba): number {
  const lin = (v: number) => {
    const s = v / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  return 0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2]);
}

function contrast(a: Rgba, b: Rgba): number {
  const [x, y] = [luminance(a), luminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

function ratio(paint: Paint, map: Record<string, string>, base: Rgba): number | null {
  const fg = resolve(paint.color, map);
  if (!fg) return null;
  const bg = over(paint.background ? resolve(paint.background, map) : null, base);
  return contrast(over(fg, bg), bg);
}

type Rule = { prelude: string; parts: string[]; body: string; line: number };

function normalise(part: string): string {
  let p = part.trim();
  if (p.startsWith(LIGHT_PREFIX)) p = p.slice(LIGHT_PREFIX.length);
  return p.trim().replace(/\s+/g, " ");
}

/** Leaf rules (no nested brace); `@` preludes skipped; 1-based selector line. */
function leafRules(text: string): Rule[] {
  const src = stripComments(text);
  const out: Rule[] = [];
  const re = /([^{}]+)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(src))) {
    const prelude = m[1].trim();
    if (prelude.startsWith("@")) continue;
    const start = m.index + (m[1].length - m[1].trimStart().length);
    out.push({
      prelude,
      parts: prelude.split(",").map(normalise),
      body: m[2],
      line: src.slice(0, start).split("\n").length,
    });
  }
  return out;
}

function declared(body: string, ...names: string[]): string | null {
  const decls = body
    .split(";")
    .map((d) => {
      const i = d.indexOf(":");
      return i < 0 ? null : [d.slice(0, i).trim().toLowerCase(), d.slice(i + 1).trim()];
    })
    .filter((d): d is string[] => d !== null);
  for (const name of names) {
    const hit = decls.filter((d) => d[0] === name);
    if (hit.length) return hit[hit.length - 1][1];
  }
  return null;
}

const paintOf = (body: string): Paint => ({
  color: declared(body, "color"),
  background: declared(body, "background", "background-color"),
});

function findViolations(sheets: Sheet[], sidebarVars: string, terminalVars: string): Violation[] {
  const { dark, light } = tokenMaps(sidebarVars, terminalVars);
  const baseDark = parseColor(dark["--sidebar-bg"] ?? null) ?? [10, 10, 15, 1];
  const baseLight = parseColor(light["--sidebar-bg"] ?? null) ?? [245, 245, 247, 1];
  const parsed = sheets.map((s) => ({ ...s, rules: leafRules(s.text) }));

  const overrides = new Map<string, Paint>();
  for (const { rules } of parsed) {
    for (const rule of rules) {
      if (!rule.prelude.includes("light-theme")) continue;
      const paint = paintOf(rule.body);
      for (const part of rule.parts) {
        const prev = overrides.get(part);
        overrides.set(part, {
          color: paint.color ?? prev?.color ?? null,
          background: paint.background ?? prev?.background ?? null,
        });
      }
    }
  }

  const out: Violation[] = [];
  for (const { path, rules } of parsed) {
    for (const rule of rules) {
      if (rule.prelude.includes("light-theme")) continue;
      const own = paintOf(rule.body);
      if (!own.color) continue;
      const darkRatio = ratio(own, dark, baseDark);
      if (darkRatio === null) continue;
      const candidates = rule.parts.every((p) => overrides.has(p))
        ? rule.parts.map((p) => {
            const ov = overrides.get(p)!;
            return { color: ov.color ?? own.color, background: ov.background ?? own.background };
          })
        : [own];
      let worst: { paint: Paint; light: number } | null = null;
      for (const paint of candidates) {
        const r = ratio(paint, light, baseLight);
        if (r !== null && (worst === null || r < worst.light)) worst = { paint, light: r };
      }
      if (worst === null || worst.light >= MIN_RATIO || darkRatio < MIN_RATIO) continue;
      out.push({
        file: path.replace(/^src\//, ""),
        line: rule.line,
        selector: rule.parts.join(", "),
        light: worst.light,
        dark: darkRatio,
        color: worst.paint.color!,
        background: worst.paint.background,
      });
    }
  }
  return out;
}

const key = (v: { file: string; selector: string }) => `${v.file}\u0000${v.selector}`;

function evaluate(violations: Violation[], allowlist: Allowlist) {
  const allowed = new Set(allowlist.entries.map(key));
  const found = new Set(violations.map(key));
  return {
    unallowed: violations.filter((v) => !allowed.has(key(v))),
    stale: allowlist.entries.filter((e) => !found.has(key(e))),
  };
}

function describeViolation(v: Violation): string {
  return (
    `  ${v.file}:${v.line}  ${v.selector}  light ${v.light.toFixed(2)} / dark ${v.dark.toFixed(2)}` +
    `  color=${v.color} background=${v.background ?? "(none, --sidebar-bg)"}`
  );
}

const FIXTURE_VARS = ":root { --sidebar-bg: #0a0a0f; }\nhtml.light-theme { --sidebar-bg: #f5f5f7; }\n";
const fixture = (text: string) =>
  findViolations([{ path: "src/fixture.css", text }], FIXTURE_VARS, "");
const EMPTY: Allowlist = { version: 1, entries: [] };

describe("theme contrast (fixtures)", () => {
  it("binds the empty case", () => {
    const v = fixture(".ok { color: #1a1a2e; background: #ffffff; }\n");
    expect(evaluate(v, EMPTY)).toEqual({ unallowed: [], stale: [] });
  });

  it("binds a synthetic violation", () => {
    const v = fixture("\n.bad { color: #f2d98a; }\n");
    expect(v).toHaveLength(1);
    expect(v[0]).toMatchObject({ file: "fixture.css", line: 2, selector: ".bad" });
    expect(v[0].light).toBeLessThan(MIN_RATIO);
    expect(evaluate(v, EMPTY).unallowed).toEqual(v);
  });

  it("binds a stale entry", () => {
    const v = fixture(".ok { color: #1a1a2e; }\n");
    const entry = { file: "fixture.css", line: 1, selector: ".ok", reason: "x" };
    expect(evaluate(v, { version: 1, entries: [entry] }).stale).toEqual([entry]);
  });

  it("a grouped light override matches every base rule it lists", () => {
    const v = fixture(
      ".a { color: #f2d98a; }\n.b { color: #f2d98a; }\n" +
        "html.light-theme .a,\nhtml.light-theme .b { color: #8a6400; }\n",
    );
    expect(v).toEqual([]);
  });

  it("a list base rule needs every part overridden", () => {
    const v = fixture(".a, .b { color: #f2d98a; }\nhtml.light-theme .a { color: #8a6400; }\n");
    expect(v.map((x) => x.selector)).toEqual([".a, .b"]);
  });
});

describe("theme contrast (real tree)", () => {
  const sheets = CSS_PATHS.map((path) => ({ path, text: readRoot(path) }));
  const allowlist = JSON.parse(readRoot(ALLOWLIST_PATH)) as Allowlist;
  const violations = findViolations(
    sheets,
    readRoot(`src/${SIDEBAR_VARS}`),
    readRoot(`src/${TERMINAL_VARS}`),
  );

  it("no prelude hides a comma inside parentheses", () => {
    expect(sheets.length).toBeGreaterThan(10);
    const hidden = sheets.flatMap((s) =>
      leafRules(s.text)
        .filter((r) => /\([^()]*,[^()]*\)/.test(r.prelude))
        .map((r) => `${s.path}:${r.line} ${r.prelude}`),
    );
    expect(hidden).toEqual([]);
  });

  it("no new light-mode contrast regressions", () => {
    const { unallowed } = evaluate(violations, allowlist);
    expect(
      unallowed,
      `Rules readable in dark mode but below ${MIN_RATIO} in light mode:\n` +
        `${unallowed.map(describeViolation).join("\n")}\n` +
        `add a html.light-theme override, or an entry with a reason in ${ALLOWLIST_PATH}`,
    ).toEqual([]);
  });

  it("the allowlist has no stale entries", () => {
    const { stale } = evaluate(violations, allowlist);
    expect(
      stale,
      `Stale entries (no violation any more), delete them from ${ALLOWLIST_PATH}:\n` +
        stale.map((e) => `  ${e.file}:${e.line}  ${e.selector}`).join("\n"),
    ).toEqual([]);
  });
});
