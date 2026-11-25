import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declarations, declProps, declValue, scanRules, soleRuleBody, type ScannedRule } from "../../sidebar/styles/css-test-helpers";

// Native <select> readable on WebKitGTK. select.css draws the arrow at
// specificity (0,2,2); any class rule at that level or above that sets the
// background shorthand would erase it. jsdom paints nothing: bytes are the contract.

const SRC = new URL("../../", import.meta.url);
const read = (rel: string): string => readFileSync(new URL(rel, SRC), "utf8");
const blank = (m: string): string => m.replace(/[^\r\n]/g, " ");
const mask = (css: string): string =>
  css.replace(/\/\*[\s\S]*?\*\//g, blank).replace(/@[a-z-]+[^;{}]*;/gi, blank);
const rulesOf = (rel: string): ScannedRule[] => scanRules(mask(read(rel)));

const ARROW = "html select:not([multiple]):not(.entity-select)";
const LIGHT_ARROW = "html.light-theme select:not([multiple]):not(.entity-select)";
// Background source of every select (Spec Board excluded: undefined tokens, covered live).
const SOURCES: Array<[string, string]> = [
  ["sidebar/styles/settings-modal.css", ".settings-input"],
  ["watchers/styles/watchers.css", ".watchers-select"],
  ["context-history/styles.css", ".ch-overlay select"],
  ["resource-monitor/styles/resource-monitor.css", ".rm-sort-field"],
  ["sidebar/styles/sidebar.css", ".workgroup-groups-nonstop-bot"],
];

type Rgba = [number, number, number, number];
const tokens = (body: string): Record<string, string> =>
  Object.fromEntries(declarations(body).filter(([p]) => p.startsWith("--")));
const hex = (h: string): Rgba => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16)).concat(1) as Rgba;
function color(raw: string, map: Record<string, string>, depth = 0): Rgba {
  const v = raw.trim().toLowerCase();
  const ref = /^var\(\s*(--[\w-]+)\s*\)$/.exec(v);
  if (ref && depth < 6) return color(map[ref[1] ?? ""] ?? "", map, depth + 1);
  if (/^#[0-9a-f]{6}$/.test(v)) return hex(v);
  const m = /^rgba?\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+))?\s*\)$/.exec(v);
  if (!m) throw new Error(`unresolved color: ${raw}`);
  return [+(m[1] ?? 0), +(m[2] ?? 0), +(m[3] ?? 0), m[4] === undefined ? 1 : +m[4]];
}
const over = (top: Rgba, base: Rgba): Rgba =>
  [0, 1, 2].map((i) => (top[i] ?? 0) * top[3] + (base[i] ?? 0) * (1 - top[3])).concat(1) as Rgba;
const lum = (c: Rgba): number =>
  [0.2126, 0.7152, 0.0722].reduce((sum, w, i) => {
    const s = (c[i] ?? 0) / 255;
    return sum + w * (s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4);
  }, 0);
const wcag = (a: Rgba, b: Rgba): number => {
  const [x, y] = [lum(a) + 0.05, lum(b) + 0.05];
  return Math.max(x, y) / Math.min(x, y);
};

const CLASSES = [
  "settings-input", "settings-rail-select", "settings-rail-count-select",
  "watchers-select", "rm-sort-field", "workgroup-groups-nonstop-bot",
];

type Spec = [number, number, number];
const add = (x: Spec, y: Spec): Spec => [x[0] + y[0], x[1] + y[1], x[2] + y[2]];
const gte = (x: Spec, y: Spec): boolean => {
  for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return (x[i] ?? 0) > (y[i] ?? 0);
  return true;
};

/** Split on top-level commas only. */
function splitTop(s: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < s.length; i++) {
    if (s[i] === "(") depth++;
    else if (s[i] === ")") depth--;
    else if (s[i] === "," && depth === 0) {
      out.push(s.slice(start, i));
      start = i + 1;
    }
  }
  out.push(s.slice(start));
  return out.map((p) => p.trim()).filter((p) => p !== "");
}

/** Index of the `)` that closes the `(` at `open`. */
function closeOf(s: string, open: number): number {
  let depth = 0;
  for (let i = open; i < s.length; i++) {
    if (s[i] === "(") depth++;
    else if (s[i] === ")" && --depth === 0) return i;
  }
  throw new Error(`unbalanced: ${s}`);
}

function specificity(sel: string): Spec {
  let total: Spec = [0, 0, 0];
  let s = sel.replace(/::[\w-]+(\([^)]*\))?/g, () => {
    total = add(total, [0, 0, 1]);
    return " ";
  });
  for (let m = /:(not|is|where)\(/.exec(s); m; m = /:(not|is|where)\(/.exec(s)) {
    const open = m.index + m[0].length - 1;
    const close = closeOf(s, open);
    if (m[1] !== "where") {
      const parts = splitTop(s.slice(open + 1, close)).map(specificity);
      total = add(total, parts.reduce((best, p) => (gte(p, best) ? p : best), [0, 0, 0] as Spec));
    }
    s = `${s.slice(0, m.index)}${s.slice(close + 1)}`;
  }
  const ids = s.match(/#[\w-]+/g)?.length ?? 0;
  const classes = (s.match(/\.[\w-]+|\[[^\]]*\]|:[\w-]+(\([^)]*\))?/g) ?? []).length;
  const elements = (s.match(/(^|[\s>+~])[a-zA-Z][\w-]*/g) ?? []).length;
  return add(total, [ids, classes, elements]);
}

const lastCompound = (sel: string): string => sel.trim().split(/[\s>+~]+/).pop() ?? "";
const isTarget = (sel: string): boolean => {
  const last = lastCompound(sel);
  if (/^select(?![\w-])/.test(last)) return true;
  return CLASSES.some((c) => new RegExp(`\\.${c}(?![\\w-])`).test(last));
};
const setsBackground = (body: string): boolean =>
  declProps(body).some((p) => p === "background" || p === "background-image");

/** Target selectors whose specificity ties or beats the arrow rule. */
function offenders(rules: ScannedRule[]): string[] {
  return rules
    .filter((r) => setsBackground(r.body))
    .flatMap((r) => r.selectors)
    .filter((s) => isTarget(s) && gte(specificity(s), [0, 2, 2]));
}

describe("native select contrast (#5140)", () => {
  it("variables.css sets color-scheme per theme", () => {
    const vars = rulesOf("sidebar/styles/variables.css");
    expect(declValue(soleRuleBody(vars, ":root"), "color-scheme")).toBe("dark");
    expect(declValue(soleRuleBody(vars, "html.light-theme"), "color-scheme")).toBe("light");
  });

  it("select.css draws the arrow and themes options", () => {
    const rules = rulesOf("shared/styles/select.css");
    const arrow = soleRuleBody(rules, ARROW);
    expect(declValue(arrow, "appearance")).toBe("none");
    expect(declValue(arrow, "background-image")).toContain("data:image/svg+xml");
    expect(declValue(arrow, "padding-right")).toBe("24px");
    const option = soleRuleBody(rules, "select option");
    expect(declValue(option, "background-color")).toBe("var(--sidebar-surface)");
    expect(declValue(option, "color")).toBe("var(--sidebar-fg)");
    expect(declValue(arrow, "background-image")).toContain("%2388888f");
    const light = soleRuleBody(rules, LIGHT_ARROW);
    expect(declValue(light, "background-image")).toContain("%2355555c");
  });

  it("arrow and text contrast on every select background (r3)", () => {
    const vars = rulesOf("sidebar/styles/variables.css");
    const dark = tokens(soleRuleBody(vars, ":root"));
    const themes = { dark, light: { ...dark, ...tokens(soleRuleBody(vars, "html.light-theme")) } };
    const arrows = { dark: "#88888f", light: "#55555c" };
    // Positive control: gate 3 measured #88888f on light --sidebar-hover = 2.88.
    const hover = color(themes.light["--sidebar-hover"] ?? "", themes.light);
    expect(wcag(hex(arrows.dark), hover)).toBeLessThan(3.0);
    const fails: string[] = [];
    for (const [file, sel] of SOURCES) {
      const body = soleRuleBody(rulesOf(file), sel);
      for (const t of ["dark", "light"] as const) {
        const map = themes[t];
        // rgba backgrounds are composited over --sidebar-bg; the real parent may differ.
        const base = color(map["--sidebar-bg"] ?? "", map);
        const bg = over(color(declValue(body, "background"), map), base);
        // r4: a sole light-theme rule with `color` replaces the class text color.
        const hits = t === "light" ? rulesOf(file).filter((r) => r.selectors.includes(`html.light-theme ${sel}`)) : [];
        const lightColor = hits.length === 1 && hits[0] ? declValue(hits[0].body, "color") : undefined;
        const fg = over(color(lightColor ?? declValue(body, "color"), map), bg);
        const a = wcag(hex(arrows[t]), bg);
        const x = wcag(fg, bg);
        if (a < 3.0 || x < 4.5) fails.push(`${sel} ${t}: arrow ${a.toFixed(2)} text ${x.toFixed(2)}`);
      }
    }
    expect(fails).toEqual([]);
  });

  it("Context History light select text override (r4)", () => {
    const rules = rulesOf("context-history/styles.css");
    const body = soleRuleBody(rules, "html.light-theme .ch-overlay select");
    expect(declValue(body, "color")).toBe("#0052a3");
    expect(declProps(body)).not.toContain("background");
    expect(declProps(body)).not.toContain("background-image");
    const vars = rulesOf("sidebar/styles/variables.css");
    const dark = tokens(soleRuleBody(vars, ":root"));
    const light = { ...dark, ...tokens(soleRuleBody(vars, "html.light-theme")) };
    const bg = over(color(light["--toolbar-btn-bg"] ?? "", light), color(light["--sidebar-bg"] ?? "", light));
    expect(wcag(hex("#0052a3"), bg)).toBeGreaterThanOrEqual(6.0);
    // Positive control: old light --toolbar-btn-fg (#0066cc) reproduces 4.44.
    expect(wcag(color(light["--toolbar-btn-fg"] ?? "", light), bg)).toBeLessThan(4.5);
  });

  it("main.tsx imports select.css", () => {
    expect(read("main.tsx")).toMatch(/^import "\.\/shared\/styles\/select\.css";$/m);
  });

  it("specificity positive controls", () => {
    expect(specificity(ARROW)).toEqual([0, 2, 2]);
    expect(specificity("::-webkit-scrollbar")).toEqual([0, 0, 1]);
    expect(specificity(":where(.x) select")).toEqual([0, 0, 1]);
    expect(specificity("html.light-theme .settings-input:focus")).toEqual([0, 3, 1]);
    expect(specificity("html .rm-sort-field:hover")).toEqual([0, 2, 1]);
  });

  it("guard flags only real offenders", () => {
    expect(offenders(scanRules("html.light-theme .settings-input:focus{background:red}"))).toHaveLength(1);
    expect(offenders(scanRules("html .rm-sort-field:hover{background:red}"))).toHaveLength(0);
    expect(offenders(scanRules(".settings-input-sm{background:red}"))).toHaveLength(0);
  });

  it("no select background rule reaches (0,2,2)", () => {
    // Inline style= backgrounds are invisible to this guard (none at 33755a587).
    const sheets = Object.keys(import.meta.glob("../../**/*.css"))
      .map((p) => new URL(p, import.meta.url).href.slice(SRC.href.length))
      .filter((p) => p !== "shared/styles/select.css");
    expect(sheets.length).toBeGreaterThan(10);
    const rules = sheets.flatMap((p) => {
      const css = mask(read(p));
      return css.includes("{") ? scanRules(css).map((r) => ({ ...r, path: p })) : [];
    });
    const targets = rules.filter((r) => setsBackground(r.body) && r.selectors.some(isTarget));
    expect(targets.length).toBeGreaterThan(0);
    expect(rules.flatMap((r) => offenders([r]).map((s) => `${r.path}: ${s}`))).toEqual([]);
  });
});
