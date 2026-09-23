import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  declarations,
  declValue,
  escapeRe,
  scanRules,
  type ScannedRule,
} from "./css-test-helpers";

// #2271 / #2408 - the Co-managed ring. jsdom never applies sidebar.css (see
// working-tint-css.test.ts), so the bytes on disk are the only place the
// declarations, the token values and the contrast of the ring can be pinned.
//
// Every extraction below throws when it does not find what it looks for, so no
// assertion can pass vacuously on a missing rule or token.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const VARS = readFileSync(new URL("./variables.css", import.meta.url), "utf8");

// Blank out comment spans, preserving length and line breaks, so a comment can
// never satisfy a rule lookup.
function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " "));
}

const RULES = scanRules(stripComments(CSS));

/** The first rule whose selector list contains `selector` exactly. Throws on a miss. */
function ruleFor(selector: string): ScannedRule {
  const found = RULES.find((rule) => rule.selectors.includes(selector));
  if (!found) throw new Error(`no rule carries the selector: ${selector}`);
  return found;
}

function varsBlock(selector: string): string {
  const re = new RegExp(`^${escapeRe(selector)} \\{([^}]*)\\}`, "m");
  const match = stripComments(VARS).match(re);
  if (!match) throw new Error(`missing variables.css block: ${selector}`);
  return match[1];
}

const CONTAINERS = [".session-item", ".replica-item", ".root-agent-banner"];
const RING_SELECTORS = CONTAINERS.map((c) => `${c} .session-item-status.comanaged`);

// --- No-layout validator ----------------------------------------------------

const RING_DECLARATIONS: Array<[string, string]> = [
  ["outline", "2px solid var(--status-comanaged)"],
  ["outline-offset", "2px"],
];

/** The one validator the real rule and every mutant go through: the ring may
 *  declare exactly the outline pair and nothing else, so width, height, border,
 *  padding, margin, background or box-shadow can never ride along. */
function isNonLayoutRingBody(body: string): boolean {
  const decls = declarations(body);
  return (
    decls.length === RING_DECLARATIONS.length &&
    RING_DECLARATIONS.every(([prop, value], i) => decls[i][0] === prop && decls[i][1] === value)
  );
}

// --- Contrast arithmetic ------------------------------------------------------

type Rgb = [number, number, number];
type Rgba = [number, number, number, number];

function parseHex(hex: string): Rgb {
  const m = /^#([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) throw new Error(`not a #rrggbb colour: ${hex}`);
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 0xff, (n >> 8) & 0xff, n & 0xff];
}

function parseRgba(value: string): Rgba {
  const m = /^rgba\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*([\d.]+)\s*\)$/.exec(value.trim());
  if (!m) throw new Error(`not an rgba() literal: ${value}`);
  return [Number(m[1]), Number(m[2]), Number(m[3]), Number(m[4])];
}

/** Source-over of a translucent paint on an opaque backdrop, rounded to the
 *  nearest rendered 8-bit channel. */
function composite(top: Rgba, under: Rgb): Rgb {
  const a = top[3];
  return [0, 1, 2].map((i) => Math.round(top[i] * a + under[i] * (1 - a))) as Rgb;
}

/** WCAG 2.x relative luminance, no intermediate rounding. */
function luminance(rgb: Rgb): number {
  const [r, g, b] = rgb.map((c) => {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
}

/** WCAG 1.4.11 non-text contrast: the ring must reach 3:1 against its backdrop. */
const meetsNonTextContrast = (ratio: number): boolean => ratio >= 3;

// --- Source-derived paints ----------------------------------------------------

const THEMES = [
  { name: "dark", vars: ":root", prefix: "" },
  { name: "light", vars: "html.light-theme", prefix: "html.light-theme " },
] as const;

/** A `var(--token)` reference resolved against one theme block. Throws on a miss. */
function tokenHex(themeVars: string, reference: string): Rgb {
  const m = /^var\((--[\w-]+)\)$/.exec(reference.trim());
  if (!m) throw new Error(`not a var() reference: ${reference}`);
  return parseHex(declValue(themeVars, m[1]));
}

interface Measured {
  name: string;
  ratio: number;
}

function measuredMatrix(): Measured[] {
  const out: Measured[] = [];
  const wash = parseRgba(declValue(ruleFor(".replica-item.working::after").body, "background"));
  const hoverRef = declValue(ruleFor(".replica-item:hover").body, "background");
  const activeRef = declValue(ruleFor(".replica-item.active").body, "background");
  // The base row paints nothing of its own: it shows the sidebar background.
  if (declarations(ruleFor(".replica-item").body).some(([p]) => p.startsWith("background"))) {
    throw new Error(".replica-item gained its own background; the matrix no longer models it");
  }

  for (const theme of THEMES) {
    const vars = varsBlock(theme.vars);
    const ring = parseHex(declValue(vars, "--status-comanaged"));
    const sidebarBg = parseHex(declValue(vars, "--sidebar-bg"));
    const rows: Array<[string, Rgb]> = [
      ["normal", sidebarBg],
      ["hover", tokenHex(vars, hoverRef)],
      ["active", tokenHex(vars, activeRef)],
    ];
    for (const [state, row] of rows) {
      out.push({ name: `row ${theme.name} ${state}`, ratio: contrast(ring, row) });
    }
    // The working ::after wash sits above the whole row, ring included.
    for (const [state, row] of rows) {
      out.push({
        name: `working row ${theme.name} ${state}`,
        ratio: contrast(composite(wash, ring), composite(wash, row)),
      });
    }
    for (const state of ["normal", "hover"] as const) {
      const selector = `${theme.prefix}.root-agent-banner${state === "hover" ? ":hover" : ""}`;
      const tint = parseRgba(declValue(ruleFor(selector).body, "background"));
      out.push({
        name: `root banner ${theme.name} ${state}`,
        ratio: contrast(ring, composite(tint, sidebarBg)),
      });
    }
  }
  return out;
}

describe("Co-managed ring CSS (#2408)", () => {
  it("all three selectors share exactly the non-layout outline declarations", () => {
    for (const selector of RING_SELECTORS) {
      const rule = ruleFor(selector);
      expect(isNonLayoutRingBody(rule.body), selector).toBe(true);
    }
    // One shared rule, not three drifting copies.
    const rule = ruleFor(RING_SELECTORS[0]);
    expect([...rule.selectors].sort()).toEqual([...RING_SELECTORS].sort());
  });

  it("the no-layout validator rejects width, height, margin and box-shadow mutants", () => {
    const real = ruleFor(RING_SELECTORS[0]).body;
    expect(isNonLayoutRingBody(real)).toBe(true);
    const mutants = ["width: 12px", "height: 12px", "margin: 2px", "box-shadow: 0 0 6px red"];
    for (const extra of mutants) {
      const mutant = `${real.trimEnd().replace(/;?$/, ";")} ${extra};`;
      expect(isNonLayoutRingBody(mutant), `mutant with ${extra}`).toBe(false);
      console.log(`[#2408 mutation] ring rule + "${extra}" -> rejected`);
    }
  });

  it("keeps the dot 8x8px and the activity background rules unchanged", () => {
    const dot = ruleFor(".session-item-status").body;
    expect(declValue(dot, "width")).toBe("8px");
    expect(declValue(dot, "height")).toBe("8px");

    const activity: Array<[string, string]> = [
      [".session-item-status.active", "background: var(--status-active); box-shadow: 0 0 6px var(--status-active);"],
      [".session-item-status.running", "background: var(--status-running);"],
      [".session-item-status.idle", "background: var(--status-idle);"],
      [".session-item-status.exited", "background: var(--status-exited);"],
      [".session-item-status.offline", "background: var(--status-offline);"],
      [".replica-item .session-item-status.pending", "background: var(--status-pending); box-shadow: 0 0 6px var(--status-pending);"],
      [".replica-item .session-item-status.waiting", "background: var(--status-waiting); box-shadow: 0 0 6px var(--status-waiting);"],
    ];
    for (const [selector, body] of activity) {
      expect(ruleFor(selector).body.trim(), selector).toBe(body);
    }
  });

  // Split so a repo-wide grep for the removed class finds no survivor here.
  const REMOVED_CLASS = ["replica", "comanaged"].join("-");

  it("no removed in-row Co-managed selector survives", () => {
    expect(CSS).not.toContain(REMOVED_CLASS);
  });

  it("the ring token keeps its dark and light values", () => {
    expect(declValue(varsBlock(":root"), "--status-comanaged")).toBe("#ff3b5c");
    expect(declValue(varsBlock("html.light-theme"), "--status-comanaged")).toBe("#dc2626");
  });
});

describe("Co-managed ring contrast (#2408)", () => {
  it("arithmetic controls: 21:1, ~4.48:1, and a ~2.32:1 pair the >= 3 validator rejects", () => {
    const white = parseHex("#ffffff");
    const black = contrast(parseHex("#000000"), white);
    const grey77 = contrast(parseHex("#777777"), white);
    const greyAa = contrast(parseHex("#aaaaaa"), white);
    expect(Math.abs(black - 21)).toBeLessThan(1e-9);
    expect(grey77).toBeCloseTo(4.48, 2);
    expect(greyAa).toBeCloseTo(2.32, 2);
    expect(meetsNonTextContrast(black)).toBe(true);
    expect(meetsNonTextContrast(grey77)).toBe(true);
    expect(meetsNonTextContrast(greyAa)).toBe(false);
    console.log(
      `[#2408 control] #000/#fff=${black.toFixed(3)} #777/#fff=${grey77.toFixed(3)} #aaa/#fff=${greyAa.toFixed(3)} (rejected)`,
    );
  });

  it("measured default/noir-minimal/command-center surfaces: all 16 named ratios are >= 3:1", () => {
    const matrix = measuredMatrix();
    expect(matrix).toHaveLength(16);
    for (const { name, ratio } of matrix) {
      console.log(`[#2408 contrast] ${name}: ${ratio.toFixed(3)}:1`);
      expect(meetsNonTextContrast(ratio), `${name} = ${ratio}`).toBe(true);
    }
    // Known low cases from the plan, recomputed from the source bytes.
    const byName = new Map(matrix.map((m) => [m.name, m.ratio]));
    expect(byName.get("working row dark active")).toBeCloseTo(3.358, 2);
    expect(byName.get("working row light active")).toBeCloseTo(3.386, 2);
    expect(byName.get("working row light hover")).toBeCloseTo(3.756, 2);
    expect(byName.get("root banner light hover")).toBeCloseTo(3.973, 2);
  });

  it("noir-minimal and command-center row/banner rules reuse only the measured paints", () => {
    const allowed = new Set(["var(--sidebar-hover)", "var(--sidebar-active)"]);
    for (const style of ["noir-minimal", "command-center"]) {
      const scoped = RULES.filter((rule) =>
        rule.selectors.some(
          (s) =>
            s.includes(`[data-sidebar-style="${style}"]`) &&
            /\.(replica-item|root-agent-banner)(?![\w-])/.test(s),
        ),
      );
      expect(scoped.length, style).toBeGreaterThan(0);
      for (const rule of scoped) {
        for (const [prop, value] of declarations(rule.body)) {
          if (prop.startsWith("background")) {
            expect(allowed.has(value), `${style}: ${rule.selectors.join(", ")} ${prop}: ${value}`).toBe(true);
          }
        }
      }
    }
  });

  it("unmeasured by design: card-sections, deep-space, arctic-ops (translucent/gradient ancestors)", () => {
    // The source-only compositor cannot model these paints, so no ratio is
    // manufactured for them; reviewer visual inspection covers them.
    for (const style of ["card-sections", "deep-space", "arctic-ops"]) {
      expect(CSS).toContain(`[data-sidebar-style="${style}"]`);
      console.log(`[#2408 contrast] ${style}: unmeasured (explicitly out of the measured matrix)`);
    }
  });
});
