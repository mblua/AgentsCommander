import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declarations, declValue, scanRules, type ScannedRule } from "./css-test-helpers";

// #2236 phase 3 (#2283) - compact sidebar CSS, pinned as bytes on disk. jsdom applies no
// stylesheet and performs no layout, so flushness and row height are D20 runtime gates, not
// gates of this file. The sheet is CRLF, so comments are blanked with line breaks kept. Pins
// 2-4 merge every exact block carrying a selector: base and phase blocks are separate, so a
// first-match parser reads half the cascade and passes.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const RULES = scanRules(CSS.replace(/\/\*[\s\S]*?\*\//g, (match) => match.replace(/[^\r\n]/g, " ")));

const BANNER = ".root-agent-banner";
const TOGGLE = ".root-agent-banner-toggle";
const OPEN = ".root-agent-banner-open";
const LEFT_RAIL = '.sidebar-layout[data-rail-side="left"]';
const COMPACT = ".sidebar-layout.sidebar-compact";
const LEFT_RAIL_TOGGLE = `${LEFT_RAIL} .root-agent-banner-toggle`;
const COMPACT_BANNER = `${COMPACT} .root-agent-banner`;
const COMPACT_TOGGLE = `${COMPACT} .root-agent-banner-toggle`;
const COMPACT_TOGGLE_LEFT = `${COMPACT}[data-rail-side="left"] .root-agent-banner-toggle`;
const COMPACT_HIDDEN = `${COMPACT} .root-agent-banner > :not(.root-agent-banner-toggle)`;
const COMPACT_HALO = `${COMPACT} .root-agent-banner.active::before`;
const COMPACT_SCROLLABLE = ".sidebar-compact .sidebar-scrollable";
const INSET_LEFT = "--ac-banner-inset-left";
const INSET_RIGHT = "--ac-banner-inset-right";
const BANNER_STYLES = ["noir-minimal", "command-center", "card-sections"];
const PLAIN_STYLES = ["deep-space", "arctic-ops", "obsidian-mesh", "neon-circuit"];
const PLAIN_INLINE = ["border", "border-left", "border-right", "margin", "margin-left", "margin-right"];
const bannerFor = (style: string): string => `[data-sidebar-style="${style}"] .root-agent-banner`;

// The lift rule names exactly these five; the component census that catches a
// sixth control is phase 9's test 4d.
const LIFTED = [
  ".root-agent-banner > .session-item-mic-cancel",
  ".root-agent-banner > .session-item-bridge-icon",
  ".root-agent-banner .voice-cancel-execute",
  ".root-agent-banner .ctx-badge",
  ".root-agent-banner .profile-outdated-badge",
];

/** Rules carrying `selector` verbatim - exact equality, never a substring. */
const rulesFor = (selector: string): ScannedRule[] =>
  RULES.filter((rule) => rule.selectors.includes(selector));

/** The only rule carrying `selector`; throws on any other count. */
function one(selector: string): ScannedRule {
  const rules = rulesFor(selector);
  if (rules.length !== 1) throw new Error(`expected one rule: ${selector}`);
  return rules[0];
}

/** Effective declarations of every exact block carrying `selector`, last wins. */
function merged(selector: string): Map<string, string> {
  const rules = rulesFor(selector);
  if (rules.length === 0) throw new Error(`no rule carries ${selector}`);
  const out = new Map<string, string>();
  for (const rule of rules) for (const [prop, value] of declarations(rule.body)) out.set(prop, value);
  return out;
}

function pxValue(value: string): number {
  const match = value.match(/^(-?\d+(?:\.\d+)?)px$/);
  if (!match) throw new Error(`not a px value: ${value}`);
  return Number(match[1]);
}

/** Inline border width: the side longhand wins over the shorthand. */
function inlineBorder(decls: Map<string, string>, side: "left" | "right"): number {
  const value = (decls.get(`border-${side}`) ?? decls.get("border") ?? "0").trim().split(/\s+/)[0];
  return value === "none" || value === "0" ? 0 : pxValue(value);
}

/** Inline margin of a 1-to-4 value shorthand, side-aware. */
function inlineMargin(decls: Map<string, string>, side: "left" | "right"): number {
  const specific = decls.get(`margin-${side}`);
  if (specific !== undefined) return pxValue(specific);
  const parts = (decls.get("margin") ?? "0px").split(/\s+/).map(pxValue);
  if (parts.length === 4) return side === "left" ? parts[3] : parts[1];
  return parts.length >= 2 ? parts[1] : parts[0];
}

/** [ids, classes+attributes+pseudo-classes, elements] of a compound selector. */
const specificityOf = (selector: string): number[] => [
  (selector.match(/#[\w-]+/g) ?? []).length,
  (selector.match(/\.[\w-]+|\[[^\]]*\]|:[\w-]+/g) ?? []).length,
  (selector.match(/(?:^|[\s>+~])[a-zA-Z][\w-]*/g) ?? []).length,
];
const compareSpecificity = (a: number[], b: number[]): number =>
  a.reduce((diff, value, index) => diff || value - b[index], 0);

describe("#2236 phase 3 compact sidebar CSS bytes", () => {
  it("1+5. compact removes the scrollable and the banner rule touches no padding", () => {
    const removable = one(COMPACT_SCROLLABLE);
    expect(declValue(removable.body, "display")).toBe("none");
    // width: 0 would leave the contents focusable by Tab while invisible.
    expect(removable.body).not.toContain("width");

    const selectors = [
      COMPACT_BANNER,
      ...BANNER_STYLES.map((style) => `[data-sidebar-style="${style}"] ${COMPACT_BANNER}`),
    ].sort();
    const rule = RULES.find(
      (candidate) => [...candidate.selectors].sort().join("|") === selectors.join("|"),
    );
    if (rule === undefined) throw new Error(`no compact banner rule: ${selectors.join(", ")}`);
    const decls = new Map(declarations(rule.body));
    expect(decls.get("margin-inline")).toBe("0");
    expect(decls.get("border-radius")).toBe("0");
    expect(decls.get("border-inline")).toBe("0");
    expect(decls.get(INSET_LEFT)).toBe("0px");
    expect(decls.get(INSET_RIGHT)).toBe("0px");
    // Absence leg, load-bearing: a padding or margin shorthand would move the
    // measured row height, which no jsdom test can see.
    const props = declarations(rule.body).map(([prop]) => prop);
    for (const prop of props) expect(prop, prop).not.toMatch(/^padding/);
    expect(props).not.toContain("margin");

    // D19: class and rail side are read off the SAME node, never an ancestor
    // chain. Change 2 and phase 6's toolbar rule are legitimate descendants.
    const bannerCompact = RULES.filter((candidate) =>
      candidate.selectors.some((selector) => selector.includes(COMPACT)),
    );
    expect(bannerCompact.length).toBeGreaterThan(0);
    for (const candidate of bannerCompact) {
      for (const selector of candidate.selectors) {
        expect(selector, `not compound-keyed: ${selector}`).toContain(COMPACT);
      }
    }

    const hidden = one(COMPACT_HIDDEN);
    expect(declValue(hidden.body, "visibility")).toBe("hidden");
    expect(hidden.body).not.toContain("display");
  });

  it("2. the toggle is an out-of-flow, rail-wide, chrome-less overlay", () => {
    const decls = merged(TOGGLE);
    expect(decls.get("position")).toBe("absolute");
    expect(decls.get("border")).toBe("0");
    expect(decls.get("background")).toBe("transparent");
    expect(decls.get("inline-size")).toBe("var(--ac-rail-width)");
    expect(decls.get("inline-size")).not.toContain("68");
    // Exactly one inline inset is set; the other stays auto, so the box is
    // never over-constrained.
    expect([decls.get("left"), decls.get("right")].filter((side) => side === "auto")).toHaveLength(1);
    expect(decls.get("left")).toBe("auto");
    expect(decls.get("right")).toBe("var(--ac-banner-inset-right)");
  });

  it("3. every banner is a containing block with zero default insets", () => {
    const decls = merged(BANNER);
    expect(decls.get("position")).toBe("relative");
    // Unconditional, not only on .active: otherwise a non-active banner creates
    // no stacking context for the 0/1/2 ladder.
    expect(decls.get("isolation")).toBe("isolate");
    expect(decls.get(INSET_LEFT)).toBe("0px");
    expect(decls.get(INSET_RIGHT)).toBe("0px");
  });

  it("3b. the activation overlay, toggle and five lifted names form a strict ladder", () => {
    const open = merged(OPEN);
    expect(open.get("position")).toBe("absolute");
    expect(open.get("inset")).toBe("0");
    expect(open.get("border")).toBe("0");
    expect(open.get("background")).toBe("transparent");
    const openZ = Number(open.get("z-index"));
    const toggleZ = Number(merged(TOGGLE).get("z-index"));
    expect(Number.isFinite(openZ) && Number.isFinite(toggleZ)).toBe(true);
    expect(openZ).toBeLessThan(toggleZ);
    // Exactly one rule carries the lift block, every selector banner-scoped.
    const liftRules = RULES.filter(
      (rule) =>
        rule.selectors.every((selector) => selector.startsWith(BANNER)) &&
        rule.body.replace(/\s+/g, " ").trim() === "position: relative; z-index: 2;",
    );
    expect(liftRules).toHaveLength(1);
    const lift = liftRules[0];
    expect([...lift.selectors].sort()).toEqual([...LIFTED].sort());
    // Never bare: SessionItem rows share the .ctx-badge class.
    expect(lift.selectors).toContain(".root-agent-banner .ctx-badge");
    for (const name of LIFTED) {
      const decls = merged(name);
      expect(decls.get("position"), name).toBe("relative");
      const liftedZ = Number(decls.get("z-index"));
      expect(Number.isFinite(liftedZ), name).toBe(true);
      expect(toggleZ).toBeLessThan(liftedZ);
    }
    // A disabled <button> suppresses mouse events instead of retargeting them,
    // so right-click-while-busy needs pointer-events: none.
    expect(declValue(one(`${OPEN}:disabled`).body, "pointer-events")).toBe("none");
  });

  it("4. the two insets are computed from each style's own border and margin", () => {
    const base = merged(BANNER);
    const expected: Array<[string, number, number]> = [
      ["noir-minimal", -3, 0],
      ["command-center", -4, 0],
      ["card-sections", -9, 7],
    ];
    for (const [style, left, right] of expected) {
      const perStyle = merged(bannerFor(style));
      // Both sides are computed: mirroring one from the other is the defect
      // this pin exists to catch.
      const computedLeft = -(inlineBorder(perStyle, "left") + inlineMargin(perStyle, "left"));
      const computedRight = inlineMargin(perStyle, "left") - inlineBorder(perStyle, "right");
      expect([computedLeft, computedRight], style).toEqual([left, right]);
      const effective = new Map([...base, ...merged(bannerFor(style))]);
      expect(effective.get(INSET_LEFT), style).toBe(`${computedLeft}px`);
      expect(effective.get(INSET_RIGHT), style).toBe(`${computedRight}px`);
    }
    for (const style of PLAIN_STYLES) {
      const decls = merged(bannerFor(style));
      for (const prop of PLAIN_INLINE) {
        expect(decls.has(prop), `${style} declares ${prop}`).toBe(false);
      }
    }
    const toggle = merged(TOGGLE);
    expect(toggle.get("right")).toBe("var(--ac-banner-inset-right)");
    expect(toggle.get("right")).not.toContain("calc");
    const leftRail = one(LEFT_RAIL_TOGGLE);
    expect(declValue(leftRail.body, "right")).toBe("auto");
    expect(declValue(leftRail.body, "left")).toBe("var(--ac-banner-inset-left)");
  });

  it("5b. the compact toggle rule outranks the left-rail rule by specificity", () => {
    const compact = one(COMPACT_TOGGLE);
    expect(compact.selectors).toContain(COMPACT_TOGGLE_LEFT);
    expect(declValue(compact.body, "left")).toBe("0");
    expect(declValue(compact.body, "right")).toBe("0");
    expect(declValue(compact.body, "inline-size")).toBe("auto");
    // The first selector ties (0,3,0) and would depend on file order; the
    // second wins by (0,4,0), so it never does.
    expect(compareSpecificity(specificityOf(COMPACT_TOGGLE), specificityOf(LEFT_RAIL_TOGGLE))).toBe(0);
    expect(
      compareSpecificity(specificityOf(COMPACT_TOGGLE_LEFT), specificityOf(LEFT_RAIL_TOGGLE)),
    ).toBeGreaterThan(0);
  });

  it("6+7. the compact halo is the veil tail only and no literal 68px survives", () => {
    const halo = one(COMPACT_HALO);
    const image = declValue(halo.body, "background-image");
    expect(image.split("linear-gradient(")).toHaveLength(2);
    expect(halo.body).not.toContain("rgba(255, 255, 255");
    expect(halo.body).not.toContain("var(--ac-selected-rail-width)");
    expect(declValue(halo.body, "inset")).toBe("0");
    expect(CSS.match(/(^|[^0-9])68px/gm)).toBeNull();
  });
});
