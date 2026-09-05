import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #1755 — the passive "someone is working here" wash. jsdom never applies this
// stylesheet, and it resolves the cascade by document order rather than by
// specificity (measured with a control, see ProjectPanel.working-tint.test.tsx),
// so the bytes on disk are the only place the declarations, the token values and
// the cascade position can be pinned. Leg 1 is the declarations and the cascade;
// leg 3 is the composite arithmetic and the signal ladder. Both live here.
//
// Both stylesheets are CRLF on disk. Every regex below is CRLF-safe: `[^}]*`
// spans \r\n freely and the `m` flag anchors ^ at line starts. A multi-line
// literal or a whitespace-spanning regex would match zero times here and pass
// vacuously on an LF checkout, which is why none is used.
//
// THE HELPER CONTRACT, which is load-bearing rather than stylistic: every byte
// extraction in this file throws when it does not find what it is looking for.
// There is no `?? ""` fallback and no optional-chained match anywhere. Without
// that, the absence and count assertions are vacuous — `expect(body ?? "")
// .not.toContain("z-index")` passes on the empty string — and leg 3a's loop
// would run zero times and still report a pass.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const VARS = readFileSync(new URL("./variables.css", import.meta.url), "utf8");

// Blank out comment spans, preserving length and line breaks so byte indices
// stay comparable between the raw and the scanned copy. Required for
// correctness, not hygiene: the #1755 block's own comment names
// `.ac-wg-subgroup.working` in prose, so an unstripped count for assertion 8
// returns 4 instead of 3, and sidebar.css already carries a comment mentioning
// `var(--sidebar-active)` that would pollute a token scan.
function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " "));
}

const CSS_SCAN = stripComments(CSS);
const VARS_SCAN = stripComments(VARS);

const escapeRe = (s: string): string => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

// Variadic on purpose, not as a flourish: the #1755 group rule is a two-selector
// list, so a single-selector helper cannot capture it at all, and under the
// throw-on-miss rule that failure would land on correct CSS rather than on a
// defect. Joining with `,\s*` rather than a literal newline is what keeps this
// CRLF-safe here and LF-safe on a fresh checkout.
function ruleBody(...selectors: string[]): string {
  if (selectors.length === 0) throw new Error("ruleBody needs at least one selector");
  const re = new RegExp(`^${selectors.map(escapeRe).join(",\\s*")} \\{([^}]*)\\}`, "m");
  const match = CSS.match(re);
  if (!match) throw new Error(`missing rule: ${selectors.join(", ")}`);
  return match[1];
}

function declarations(body: string): Array<[string, string]> {
  return body
    .split(";")
    .map((d) => d.trim())
    .filter((d) => d.includes(":"))
    .map((d) => [
      d.slice(0, d.indexOf(":")).trim(),
      d.slice(d.indexOf(":") + 1).trim().replace(/\s+/g, " "),
    ]);
}

const declProps = (body: string): string[] => declarations(body).map(([prop]) => prop);

/** Last declaration of `prop` in `body`, mirroring the within-rule cascade. Throws on a miss. */
function declValue(body: string, prop: string): string {
  let found: string | undefined;
  for (const [p, v] of declarations(body)) if (p === prop) found = v;
  if (found === undefined) throw new Error(`missing declaration: ${prop}`);
  return found;
}

// ---------------------------------------------------------------------------
// Colour model for leg 3.
// ---------------------------------------------------------------------------

interface Rgba {
  r: number;
  g: number;
  b: number;
  a: number;
}

function parseRgba(value: string): Rgba {
  const m = value.match(/^rgba\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*,\s*([0-9.]+)\s*\)$/);
  if (!m) throw new Error(`not an rgba literal: ${value}`);
  return { r: Number(m[1]), g: Number(m[2]), b: Number(m[3]), a: Number(m[4]) };
}

function parseHex(value: string): Rgba {
  const m = value.match(/^#([0-9a-fA-F]{6})$/);
  if (!m) throw new Error(`not a 6-digit hex colour: ${value}`);
  const h = m[1];
  return {
    r: Number.parseInt(h.slice(0, 2), 16),
    g: Number.parseInt(h.slice(2, 4), 16),
    b: Number.parseInt(h.slice(4, 6), 16),
    a: 1,
  };
}

/** source-over, in floating point. The ground is never rounded on the way through. */
function over(fg: Rgba, bg: Rgba): Rgba {
  return {
    r: fg.r * fg.a + bg.r * (1 - fg.a),
    g: fg.g * fg.a + bg.g * (1 - fg.a),
    b: fg.b * fg.a + bg.b * (1 - fg.a),
    a: 1,
  };
}

const to8 = (c: Rgba): [number, number, number] => [
  Math.round(c.r),
  Math.round(c.g),
  Math.round(c.b),
];

/** Rec.709 relative luminance on the 0..255 scale, from the UNROUNDED composite. */
const luma = (c: Rgba): number => 0.2126 * c.r + 0.7152 * c.g + 0.0722 * c.b;

// ---------------------------------------------------------------------------
// Tokens, read out of variables.css. Every read throws on a miss.
// ---------------------------------------------------------------------------

function varsBlock(selector: string): string {
  const re = new RegExp(`^${escapeRe(selector)} \\{([^}]*)\\}`, "m");
  const match = VARS_SCAN.match(re);
  if (!match) throw new Error(`missing variables.css block: ${selector}`);
  return match[1];
}

const DARK_VARS = varsBlock(":root");
const LIGHT_VARS = varsBlock("html.light-theme");

// ---------------------------------------------------------------------------
// The #1755 block, read out of the bytes.
// ---------------------------------------------------------------------------

const OVERLAY_BASE = ".replica-item::after";
const OVERLAY_WORKING = ".replica-item.working::after";
const GROUP_SELECTORS = [
  ".ac-wg-subgroup.working",
  'html.light-theme[data-sidebar-style="card-sections"] .ac-wg-subgroup.working',
];

// ---------------------------------------------------------------------------
// The `.ac-wg-subgroup` census used by assertion 7.
// ---------------------------------------------------------------------------

interface ScannedRule {
  selectors: string[];
  body: string;
  index: number;
}

/**
 * Every innermost `selector-list { ... }` in the sheet, with the byte index of
 * the first character of the selector list. Rules nested in an @media block are
 * found too; the wrapper itself never yields a match because its body contains
 * braces.
 */
function scanRules(css: string): ScannedRule[] {
  const out: ScannedRule[] = [];
  const re = /([^{}]*)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null = re.exec(css);
  while (m !== null) {
    const lead = m[1];
    const raw = lead.trim();
    if (raw !== "") {
      out.push({
        selectors: raw.split(",").map((s) => s.trim()).filter((s) => s !== ""),
        body: m[2],
        index: m.index + (lead.length - lead.trimStart().length),
      });
    }
    m = re.exec(css);
  }
  if (out.length === 0) throw new Error("scanRules found no rules at all");
  return out;
}

const ALL_RULES = scanRules(CSS_SCAN);

const SUBGROUP = ".ac-wg-subgroup";

/** The compound after the last combinator, which is the selector's subject. */
function lastCompound(selector: string): string {
  const parts = selector.split(/[\s>+~]+/).filter((p) => p !== "");
  if (parts.length === 0) throw new Error(`empty selector: ${selector}`);
  return parts[parts.length - 1];
}

/**
 * The scan criterion assertion 7 states: the subject is `.ac-wg-subgroup`,
 * optionally carrying `.working`. A selector where `.ac-wg-subgroup` is followed
 * by a combinator has a descendant as its subject and is excluded, which is what
 * correctly drops :6977, :7021 and :7480.
 */
function isSubgroupSubject(selector: string): boolean {
  const compound = lastCompound(selector);
  return compound === SUBGROUP || compound === `${SUBGROUP}.working`;
}

type Spec = [number, number, number];

function specificity(selector: string): Spec {
  let a = 0;
  let b = 0;
  let c = 0;
  let s = selector;
  s = s.replace(/\[[^\]]*\]/g, () => {
    b += 1;
    return " ";
  });
  s = s.replace(/::[a-zA-Z-]+/g, () => {
    c += 1;
    return " ";
  });
  s = s.replace(/:[a-zA-Z-]+(\([^)]*\))?/g, () => {
    b += 1;
    return " ";
  });
  s = s.replace(/#[a-zA-Z0-9_-]+/g, () => {
    a += 1;
    return " ";
  });
  s = s.replace(/\.[a-zA-Z0-9_-]+/g, () => {
    b += 1;
    return " ";
  });
  const types = s.match(/[a-zA-Z][a-zA-Z0-9_-]*/g);
  if (types) c += types.length;
  return [a, b, c];
}

const cmpSpec = (x: Spec, y: Spec): number => x[0] - y[0] || x[1] - y[1] || x[2] - y[2];

const SUBGROUP_RULES = ALL_RULES.filter((rule) => rule.selectors.some(isSubgroupSubject));

// The partition is on the presence of `.working` in the selector list, NOT on
// byte index. It has to be: the block's third rule is the bare selector
// `.ac-wg-subgroup`, textually identical to the pre-existing rule at :6280, so
// no selector-text test can separate them, and a byte-index test would
// reintroduce exactly the byte-order reasoning assertion 7 exists to replace.
const WORKING_SUBGROUP_RULES = SUBGROUP_RULES.filter((rule) =>
  rule.selectors.some((s) => s.includes(".working"))
);
const OTHER_SUBGROUP_RULES = SUBGROUP_RULES.filter(
  (rule) => !rule.selectors.some((s) => s.includes(".working"))
);

const declaresGround = (rule: ScannedRule): boolean => {
  const props = declProps(rule.body);
  return props.includes("background") || props.includes("border-left");
};

const COMPETITOR_RULES = OTHER_SUBGROUP_RULES.filter(declaresGround);

describe("#1755 working-tint CSS bytes (leg 1)", () => {
  it("1. declares the base .replica-item::after overlay in full", () => {
    const body = ruleBody(OVERLAY_BASE);
    expect(declValue(body, "content")).toBe('""');
    expect(declValue(body, "position")).toBe("absolute");
    expect(declValue(body, "inset")).toBe("0");
    expect(declValue(body, "border-radius")).toBe("inherit");
    expect(declValue(body, "pointer-events")).toBe("none");
    expect(declValue(body, "background")).toBe("transparent");
    expect(declValue(body, "transition")).toBe("background var(--transition-fast)");
  });

  it("2. .replica-item.working::after carries the row wash", () => {
    expect(declValue(ruleBody(OVERLAY_WORKING), "background")).toBe("rgba(58, 123, 255, 0.15)");
  });

  it("3. neither ::after rule declares z-index", () => {
    // The failure this kills: pushed behind the content the overlay sits UNDER
    // the opaque --sidebar-hover and --sidebar-active backgrounds and vanishes
    // in exactly the hovered and selected cases, which are required behaviours
    // 2 and 3. Both bodies come from a throwing extractor, so "not found" is a
    // failure here and can never be a pass.
    expect(declProps(ruleBody(OVERLAY_BASE))).not.toContain("z-index");
    expect(declProps(ruleBody(OVERLAY_WORKING))).not.toContain("z-index");
  });

  it("4. the working state never sets a background on the row element itself", () => {
    const onElement = CSS_SCAN.match(/\.replica-item\.working(?!::after)/g);
    expect(onElement).toBeNull();
  });

  it("5. the states the overlay has to compose with are all still present", () => {
    expect(declValue(ruleBody(".replica-item:hover"), "background")).toBe("var(--sidebar-hover)");
    const active = ruleBody(".replica-item.active");
    expect(declValue(active, "background")).toBe("var(--sidebar-active)");
    expect(declValue(active, "border-left-color")).toBe("var(--sidebar-accent)");
    const quickActive = ruleBody(
      ".coord-quick-access .replica-item.active:has(.ac-discovery-badge.coord)"
    );
    expect(declValue(quickActive, "background")).toBe("var(--sidebar-active)");
  });

  it("6. the group rule is one rule with both selectors and one set of declarations", () => {
    // Captured by the FULL two-selector list, in file order. If the second
    // selector is ever dropped this call throws rather than silently capturing
    // a narrower rule.
    const body = ruleBody(...GROUP_SELECTORS);
    expect(declValue(body, "background")).toBe("rgba(58, 123, 255, 0.066)");
    expect(declValue(body, "border-left")).toBe("3px solid rgba(58, 123, 255, 0.28)");
    // Declared once, so the two selectors cannot drift apart into two literals.
    const props = declProps(body);
    expect(props.filter((p) => p === "background")).toHaveLength(1);
    expect(props.filter((p) => p === "border-left")).toHaveLength(1);
  });

  describe("7. cascade dominance on .ac-wg-subgroup, by computed specificity", () => {
    it("7a. the scan finds exactly 13 rules whose subject is .ac-wg-subgroup", () => {
      // 11 pre-existing plus BOTH rules the #1755 block contributes: the bare
      // `.ac-wg-subgroup { transition: ... }` and the two-selector `.working`
      // list. Both meet the criterion the scan itself states. The count is
      // pinned so that a drift fails rather than silently narrowing the
      // assertion; narrowing the scan until 12 comes out is the failure mode
      // this assertion exists to prevent.
      expect(SUBGROUP_RULES).toHaveLength(13);
      expect(WORKING_SUBGROUP_RULES).toHaveLength(1);
      expect(OTHER_SUBGROUP_RULES).toHaveLength(12);
    });

    it("7b. exactly 7 of the 12 non-.working rules declare background or border-left", () => {
      // The block's own bare rule is inside this denominator and declares only
      // `transition`, so it is not a competitor. That is itself a detector: give
      // it a background and this count goes to 8.
      expect(COMPETITOR_RULES).toHaveLength(7);
    });

    it("7c. the .working rule out-ranks or out-places every competitor", () => {
      expect(WORKING_SUBGROUP_RULES).toHaveLength(1);
      const working = WORKING_SUBGROUP_RULES[0];
      const workingSpecs = working.selectors.map(specificity);
      for (const competitor of COMPETITOR_RULES) {
        for (const selector of competitor.selectors) {
          if (!isSubgroupSubject(selector)) continue;
          const spec = specificity(selector);
          const dominates = workingSpecs.some(
            (ws) =>
              cmpSpec(ws, spec) > 0 ||
              (cmpSpec(ws, spec) === 0 && working.index > competitor.index)
          );
          expect(
            dominates,
            `no selector of the #1755 group rule dominates ${selector} (${spec.join(",")})`
          ).toBe(true);
        }
      }
    });

    it("7d. every competitor above (0,2,0) is matched verbatim with .working appended", () => {
      // This is what makes 7c sufficient rather than merely necessary: it
      // guarantees the dominating selector matches everywhere the competitor
      // matches, instead of only out-ranking it on paper. Exactly one competitor
      // triggers it today, :6686 html.light-theme[...card-sections] .ac-wg-subgroup.
      expect(WORKING_SUBGROUP_RULES).toHaveLength(1);
      const working = WORKING_SUBGROUP_RULES[0];
      const base: Spec = [0, 2, 0];
      let triggered = 0;
      for (const competitor of COMPETITOR_RULES) {
        for (const selector of competitor.selectors) {
          if (!isSubgroupSubject(selector)) continue;
          if (cmpSpec(specificity(selector), base) <= 0) continue;
          triggered += 1;
          const expected = `${selector}.working`;
          expect(
            working.selectors,
            `the #1755 group rule must carry ${expected} verbatim`
          ).toContain(expected);
        }
      }
      expect(triggered).toBe(1);
    });
  });

  it("8. the token .working occurs exactly three times in the stylesheet", () => {
    // Once in .replica-item.working::after and twice in the group rule's
    // two-selector list. `.working` occurs 0 times before this change, so the
    // count is exact rather than a delta. The comment stripping is load-bearing:
    // the block's own comment names .ac-wg-subgroup.working, so the raw bytes
    // give 4.
    const hits = CSS_SCAN.match(/\.working(?![a-zA-Z0-9_-])/g);
    if (!hits) throw new Error("no .working occurrences found at all");
    expect(hits).toHaveLength(3);
  });

  it("9. variables.css carries the token values the wash is measured against", () => {
    expect(declValue(DARK_VARS, "--sidebar-active")).toBe("#222227");
    expect(declValue(LIGHT_VARS, "--sidebar-active")).toBe("#dcdce4");
    // Provenance of the literal rgba: if --status-running is ever retoned, the
    // duplication becomes visible here instead of drifting silently.
    expect(declValue(DARK_VARS, "--status-running")).toBe("#3a7bff");
  });

  it("10. exactly four styles hardcode .replica-item.active instead of using the token", () => {
    // The scan allows an optional pseudo-class suffix on the .active compound
    // and asserts on the SET OF STYLE NAMES rather than on a rule count, because
    // obsidian-mesh has no bare rule at all, only the :has() / :not(:has()) pair
    // at :7384 and :7392. A strict `\.replica-item\.active \{` returns three
    // styles and fails on correct bytes. Anchoring at ^ keeps out the
    // html.light-theme twins and the .coord-quick-access rules, which do route
    // through the token.
    const re = /^\[data-sidebar-style="([a-z-]+)"\] \.replica-item\.active(:\S+)? \{([^}]*)\}/gm;
    const literal = new Set<string>();
    const token = new Set<string>();
    let m: RegExpExecArray | null = re.exec(CSS_SCAN);
    if (m === null) throw new Error("no per-style .replica-item.active rules found");
    while (m !== null) {
      const props = declProps(m[3]);
      if (props.includes("background")) {
        const value = declValue(m[3], "background");
        (value.includes("var(--sidebar-active)") ? token : literal).add(m[1]);
      }
      m = re.exec(CSS_SCAN);
    }
    expect([...literal].sort()).toEqual([
      "arctic-ops",
      "deep-space",
      "neon-circuit",
      "obsidian-mesh",
    ]);
    expect([...token].sort()).toEqual(["noir-minimal"]);
  });
});

// ---------------------------------------------------------------------------
// Leg 3. Every value below is read out of the bytes through a throwing
// extractor, so there is no path on which a missing value yields an empty list
// and a silently empty loop.
//
// The reads are lazy, inside the tests, rather than at module scope. That is
// deliberate: a throwing extractor evaluated at module scope takes the whole
// FILE down on a mutation, so every assertion reports as a collection error and
// none of them can be shown failing by name. Evaluated inside the test, the same
// throw fails exactly the leg it belongs to. The throw-on-miss property is
// unchanged; only where it lands moves.
// ---------------------------------------------------------------------------

const rowWash = (): Rgba => parseRgba(declValue(ruleBody(OVERLAY_WORKING), "background"));
const roomWash = (): Rgba => parseRgba(declValue(ruleBody(...GROUP_SELECTORS), "background"));

interface Theme {
  name: string;
  bg: Rgba;
  hover: Rgba;
  active: Rgba;
  arcticQuickAccess: Rgba;
  obsidianQuickAccess: Rgba;
}

function darkTheme(): Theme {
  return {
    name: "dark",
    bg: parseHex(declValue(DARK_VARS, "--sidebar-bg")),
    hover: parseHex(declValue(DARK_VARS, "--sidebar-hover")),
    active: parseHex(declValue(DARK_VARS, "--sidebar-active")),
    arcticQuickAccess: parseRgba(
      declValue(
        ruleBody('[data-sidebar-style="arctic-ops"] .coord-quick-access-group'),
        "background"
      )
    ),
    obsidianQuickAccess: parseRgba(
      declValue(
        ruleBody('[data-sidebar-style="obsidian-mesh"] .coord-quick-access-group'),
        "background"
      )
    ),
  };
}

function lightTheme(): Theme {
  return {
    name: "light",
    bg: parseHex(declValue(LIGHT_VARS, "--sidebar-bg")),
    hover: parseHex(declValue(LIGHT_VARS, "--sidebar-hover")),
    active: parseHex(declValue(LIGHT_VARS, "--sidebar-active")),
    arcticQuickAccess: parseRgba(
      declValue(
        ruleBody('html.light-theme[data-sidebar-style="arctic-ops"] .coord-quick-access-group'),
        "background"
      )
    ),
    obsidianQuickAccess: parseRgba(
      declValue(
        ruleBody('html.light-theme[data-sidebar-style="obsidian-mesh"] .coord-quick-access-group'),
        "background"
      )
    ),
  };
}

const themes = (): Theme[] => [darkTheme(), lightTheme()];

/**
 * The six grounds a working row can actually land on, from the inventory under
 * "Where the row wash actually composites". Ground 2 here is the room wash over
 * --sidebar-bg, which covers the three styles whose .ac-wg-group wrapper
 * declares no background. Ground 2b, the room wash over that wrapper in the
 * other four styles, is deliberately absent: the user accepted the ladder
 * inversion it causes rather than fixing it, so there is no bound left to
 * assert there, and it is gated by the card-sections hover exhibit instead. The
 * deep-space quick-access ground is absent for a different reason: it is a
 * gradient, and a scalar composite cannot model it.
 */
function groundsFor(theme: Theme, room: Rgba): Array<[string, Rgba]> {
  return [
    ["--sidebar-bg", theme.bg],
    ["room wash over --sidebar-bg", over(room, theme.bg)],
    ["--sidebar-hover", theme.hover],
    ["--sidebar-active", theme.active],
    ["arctic-ops quick access", over(theme.arcticQuickAccess, theme.bg)],
    ["obsidian-mesh quick access", over(theme.obsidianQuickAccess, theme.bg)],
  ];
}

describe("#1755 composite arithmetic and the signal ladder (leg 3)", () => {
  it("3a. the row wash is visible on all six grounds in both themes", () => {
    // The loop must prove that it ran. Without the pinned count and
    // expect.hasAssertions(), a reader that returned an empty list instead of
    // throwing would give a loop body that never executes, zero expect calls,
    // and a PASSING test. That is what makes mutation 1's claim about this leg
    // guaranteed rather than hoped for.
    expect.hasAssertions();
    const wash = rowWash();
    const room = roomWash();
    let comparisons = 0;
    for (const theme of themes()) {
      for (const [label, ground] of groundsFor(theme, room)) {
        // The rounding convention is pinned: composite in floating point
        // through the ground, round only the two values being compared. Round
        // the intermediate ground too and the light arctic-ops blue delta moves
        // from 1 to 2, which changes no verdict but breaks reproducibility of
        // the plan's table.
        const before = to8(ground);
        const after = to8(over(wash, ground));
        const deltas = after.map((v, i) => v - before[i]);
        const strongest = Math.max(...deltas.map((d) => Math.abs(d)));
        expect(
          strongest,
          `${theme.name} / ${label}: wash is invisible, deltas (${deltas.join(", ")})`
        ).toBeGreaterThanOrEqual(4);
        comparisons += 1;
      }
    }
    expect(comparisons).toBe(12);
  });

  it("3b. the signal ladder holds in both themes", () => {
    // room wash < hover < working row < selection, as the MAGNITUDE of the
    // Rec.709 luminance distance from --sidebar-bg, so light theme (which
    // darkens) and dark theme (which lightens) are one assertion. Computed from
    // the UNROUNDED composite, rounded at no point.
    //
    // Scope, stated so the assertion is not read as more than it is: this ranks
    // the four states over --sidebar-bg. It does not model the .ac-wg-group
    // wrapper ground, and in the four styles that declare one the room wash
    // rises above hover. That inversion is measured, accepted by the user, and
    // recorded in plans/1755-sidebar-working-tint.md; it is not this assertion's
    // to catch.
    expect.hasAssertions();
    const wash = rowWash();
    const room = roomWash();
    for (const theme of themes()) {
      const base = luma(theme.bg);
      const dist = (c: Rgba): number => Math.abs(luma(c) - base);
      const roomStep = dist(over(room, theme.bg));
      const hover = dist(theme.hover);
      const workingRow = dist(over(wash, theme.bg));
      const selected = dist(theme.active);

      expect(hover - roomStep, `${theme.name}: room wash vs hover`).toBeGreaterThanOrEqual(1);
      expect(
        workingRow - hover,
        `${theme.name}: hover vs working row`
      ).toBeGreaterThanOrEqual(5);
      expect(
        selected - workingRow,
        `${theme.name}: working row vs selection`
      ).toBeGreaterThanOrEqual(5);
    }
  });
});
