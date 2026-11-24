import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declProps, declValue, scanRules, soleRuleBody, type ScannedRule } from "../../sidebar/styles/css-test-helpers";

// #5140 - native <select> readable on WebKitGTK. select.css draws the arrow at
// specificity (0,2,2); any class rule at that level or above that sets the
// background shorthand would erase it. jsdom paints nothing: bytes are the contract.

const SRC = new URL("../../", import.meta.url);
const read = (rel: string): string => readFileSync(new URL(rel, SRC), "utf8");
const blank = (m: string): string => m.replace(/[^\r\n]/g, " ");
const mask = (css: string): string =>
  css.replace(/\/\*[\s\S]*?\*\//g, blank).replace(/@[a-z-]+[^;{}]*;/gi, blank);
const rulesOf = (rel: string): ScannedRule[] => scanRules(mask(read(rel)));

const ARROW = "html select:not([multiple]):not(.entity-select)";
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
