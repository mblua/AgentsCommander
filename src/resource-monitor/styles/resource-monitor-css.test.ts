import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #2245 - the CSS byte contract for the integral view.
//
// jsdom lays nothing out, implements no container queries and does not know
// `max-content`, so the bytes on disk are the only place the layout switch, the
// focus offsets and the anti-hiding sweep can be pinned at all. Every claim
// here is about bytes; none is about geometry.
//
// THE HELPER CONTRACT, load-bearing rather than stylistic: every extraction
// below throws when it matches nothing. There is no `?? ""` and no optional
// chaining anywhere, because `expect(body ?? "").not.toContain(...)` passes
// vacuously on the empty string and the absence criteria are most of this file.
//
// The stylesheet is LF on disk. Every regex is CRLF-safe: `[^}]*` spans a
// newline freely and no assertion depends on a line ending.
const CSS = readFileSync(new URL("./resource-monitor.css", import.meta.url), "utf8");

/** Blank out comment spans, preserving length and line breaks so byte offsets
 *  stay comparable between the raw and the scanned copy. Required for
 *  correctness: the new comments name `display: none`, `float`, `@media` and
 *  `min-width: max-content` in prose, and every count below would be wrong. */
function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " "));
}

const SCAN = stripComments(CSS);

interface StyleRule {
  selector: string;
  selectors: string[];
  body: string;
  start: number;
  end: number;
  /** The at-rule name containing this rule, or null when unconditional. */
  atRule: string | null;
}

interface AtRuleBlock {
  name: string;
  prelude: string;
  start: number;
}

const normalize = (text: string): string => text.trim().replace(/\s+/g, " ");

const splitSelectors = (selector: string): string[] =>
  selector
    .split(",")
    .map(normalize)
    .filter((s) => s.length > 0);

/**
 * A deliberately small CSS block parser: it walks braces and records every
 * style rule together with the at-rule (if any) that encloses it. `@import`
 * carries no block, so it is never seen here and enters neither family — which
 * is exactly how criterion 26 counts it separately.
 */
function parse(css: string): { rules: StyleRule[]; atRules: AtRuleBlock[] } {
  const rules: StyleRule[] = [];
  const atRules: AtRuleBlock[] = [];
  const stack: (AtRuleBlock | null)[] = [];
  let preludeStart = 0;
  let index = 0;

  while (index < css.length) {
    const char = css[index];
    if (char === "{") {
      const prelude = normalize(css.slice(preludeStart, index));
      if (prelude.startsWith("@")) {
        const name = /^@([a-zA-Z-]+)/.exec(prelude);
        if (!name) throw new Error(`unparsable at-rule prelude: ${prelude}`);
        const block: AtRuleBlock = {
          name: name[1],
          prelude,
          start: preludeStart,
        };
        atRules.push(block);
        stack.push(block);
        index += 1;
        preludeStart = index;
        continue;
      }

      // A style rule: find its matching close brace. Style rules do not nest in
      // this stylesheet, so the next `}` closes it.
      const close = css.indexOf("}", index);
      if (close === -1) throw new Error(`unterminated rule: ${prelude}`);
      const enclosing = stack.length > 0 ? stack[stack.length - 1] : null;
      rules.push({
        selector: prelude,
        selectors: splitSelectors(prelude),
        body: css.slice(index + 1, close),
        start: preludeStart,
        end: close,
        atRule: enclosing ? enclosing.name : null,
      });
      index = close + 1;
      preludeStart = index;
      continue;
    }

    if (char === "}") {
      stack.pop();
      index += 1;
      preludeStart = index;
      continue;
    }

    if (char === ";") {
      index += 1;
      preludeStart = index;
      continue;
    }

    index += 1;
  }

  if (stack.length !== 0) throw new Error("unbalanced at-rule braces");
  return { rules, atRules };
}

const PARSED = parse(SCAN);
const ALL_RULES = PARSED.rules;
const BASE_RULES = ALL_RULES.filter((r) => r.atRule === null);
const CONDITIONAL_RULES = ALL_RULES.filter((r) => r.atRule !== null);

/** A WHOLE declaration, anchored both sides, so `width: 0` cannot fire on
 *  `min-width: 0` and `opacity: 0` cannot fire on `opacity: 0.01`. */
const declRe = (prop: string, value: string): RegExp =>
  new RegExp(`(^|;)\\s*${prop}\\s*:\\s*${value}\\s*(!important)?\\s*(;|$)`, "i");

const declares = (body: string, prop: string, value: string): boolean =>
  declRe(prop, value).test(body);

/** Reads back the declared value of a property, throwing when absent. */
function declaredValue(rule: StyleRule, prop: string): string {
  const match = new RegExp(`(?:^|;)\\s*${prop}\\s*:\\s*([^;]+)`, "i").exec(rule.body);
  if (!match) throw new Error(`${rule.selector} does not declare ${prop}`);
  return normalize(match[1]);
}

function ruleWithSelector(selector: string, atRule: string | null): StyleRule {
  const wanted = normalize(selector);
  const found = ALL_RULES.find(
    (r) => r.atRule === atRule && r.selectors.includes(wanted)
  );
  if (!found) {
    throw new Error(
      `missing rule for ${selector} ${atRule ? `inside @${atRule}` : "outside every at-rule"}`
    );
  }
  return found;
}

const baseRule = (selector: string): StyleRule => ruleWithSelector(selector, null);
const containerRule = (selector: string): StyleRule =>
  ruleWithSelector(selector, "container");

/** Counts top-level terms, treating `repeat(N, ...)` as the N tracks it emits.
 *  Splitting on whitespace would be wrong either way: minmax() and repeat()
 *  both hold commas and spaces of their own. */
const trackCount = (rule: StyleRule): number => {
  const value = declaredValue(rule, "grid-template-columns");
  const repeat = /^repeat\(\s*(\d+)\s*,(.*)\)$/.exec(value);
  if (repeat) {
    return Number(repeat[1]) * countTerms(repeat[2]);
  }
  return countTerms(value);
};

function countTerms(value: string): number {
  let depth = 0;
  let terms = 0;
  let inTerm = false;
  for (const ch of value) {
    if (ch === "(") depth += 1;
    else if (ch === ")") depth -= 1;
    if (depth === 0 && /\s/.test(ch)) {
      inTerm = false;
      continue;
    }
    if (!inTerm) {
      terms += 1;
      inTerm = true;
    }
  }
  return terms;
}

// Normalized zero: 0, 0px, 0%, 0em, 0rem, 0.0, 0.00 — matched as a family and
// not as one literal, so `height: 0rem` cannot slip past.
const ZERO = String.raw`0(?:\.0{1,2})?(?:px|%|em|rem)?`;

const FORBIDDEN: ReadonlyArray<[string, string]> = [
  ["display", "none"],
  ["visibility", "hidden"],
  ["visibility", "collapse"],
  ["opacity", ZERO],
  ["width", ZERO],
  ["height", ZERO],
  ["max-width", ZERO],
  ["max-height", ZERO],
  ["font-size", ZERO],
];

// Both exception lists are empty, and both are written as empty literals so
// that adding one is a visible edit in the diff rather than a silent relaxation.
const BASE_HIDING_EXCEPTIONS: ReadonlyArray<string> = [];
const CONDITIONAL_HIDING_EXCEPTIONS: ReadonlyArray<string> = [];

const METRIC_LEG_SELECTORS = [
  ".rm-group-main > span:not(.rm-group-identity):not(.rm-network-pill)",
  ".rm-process-header > span:not(:first-child)",
  ".rm-process-row > span:not(:first-child)",
];

// Every selector this change introduces. Criterion 33 is scoped to these,
// because the stylesheet already carries rgba() literals in untouched bytes.
const NEW_SELECTORS = [
  ".rm-group-identity-line",
  ".rm-partial-pill",
  ".rm-filter-pid-input",
  ".rm-filter-search-input",
  ".rm-filter-input-clear",
  ".rm-filter-help",
  ".rm-filter-pid-error",
  ".rm-filter-coverage",
  ".rm-pid-chip",
  ".rm-pid-chip-name",
  ".rm-filter-trailing",
  ".rm-sort",
  ".rm-sort-field",
  ".rm-sort-direction",
  ".rm-process-depth",
  ".rm-process-row.is-pid-match",
  ".rm-process-pid-match",
];

describe("#2245 resource-monitor.css byte contract", () => {
  // 26
  it("declares no hiding or zero-sizing anywhere, in either rule family", () => {
    // Non-vacuity first: a parser that silently returned an empty or partial
    // base set would satisfy every absence below.
    expect(BASE_RULES.length).toBeGreaterThanOrEqual(60);
    expect(CONDITIONAL_RULES.length).toBeGreaterThan(0);
    expect(BASE_RULES.length + CONDITIONAL_RULES.length).toBe(ALL_RULES.length);

    expect(BASE_HIDING_EXCEPTIONS).toEqual([]);
    expect(CONDITIONAL_HIDING_EXCEPTIONS).toEqual([]);

    // At-rule inventory. Counting the families separately is what stops the
    // layout switch being quietly reinstated as a viewport query, and banning
    // every other name stops a third family hosting an escape.
    const withBlocks = PARSED.atRules.map((a) => a.name);
    expect(new Set(withBlocks)).toEqual(new Set(["media", "container"]));
    expect(withBlocks.filter((n) => n === "media")).toHaveLength(1);
    expect(withBlocks.filter((n) => n === "container")).toHaveLength(1);
    // @import carries no rule body, so it enters neither family; it is counted
    // here on its own and must survive.
    expect(SCAN.match(/@import\b/g)).toHaveLength(1);

    for (const family of [BASE_RULES, CONDITIONAL_RULES]) {
      for (const rule of family) {
        for (const [prop, value] of FORBIDDEN) {
          expect(
            declares(rule.body, prop, value),
            `${rule.selector} declares ${prop}: ${value}`
          ).toBe(false);
        }
      }
    }

    // The one legitimate hide is asserted POSITIVELY, so the sweep above can
    // never be satisfied by deleting it. None of its values is a zero.
    const automation = baseRule(".rm-automation-metric");
    expect(declares(automation.body, "width", "1px")).toBe(true);
    expect(declares(automation.body, "height", "1px")).toBe(true);
    expect(declares(automation.body, "opacity", "0\\.01")).toBe(true);
  });

  // 27
  it("puts the layout switch on the container, at the end of the cascade", () => {
    const body = baseRule(".rm-body");
    expect(declares(body.body, "container-type", "inline-size")).toBe(true);
    expect(declares(body.body, "container-name", "rm-body")).toBe(true);

    // Exactly once, and only on .rm-body. A container-type on .rm-root would
    // make it the containing block of the position: fixed .rm-modal-backdrop
    // and shrink the kill dialog's backdrop from the window to the pane.
    expect(SCAN.match(/container-type\s*:/g)).toHaveLength(1);
    expect(SCAN.match(/container-name\s*:/g)).toHaveLength(1);
    // The shorthand sets both while leaving both longhand counts at one, so it
    // would slip past the guard above. It must not appear at all.
    expect(SCAN.match(/(^|[;{])\s*container\s*:/g)).toBeNull();

    // Matched as bytes: a max-width form, a return to 760px or 899px, or any
    // @media form fails here.
    const container = PARSED.atRules.find((a) => a.name === "container");
    if (!container) throw new Error("missing @container block");
    expect(container.prelude).toBe("@container rm-body (min-width: 860px)");

    // Five rule groups, asserted present rather than merely not-absent.
    expect(declares(containerRule(".rm-group-main").body, "display", "grid")).toBe(true);
    expect(trackCount(containerRule(".rm-group-main"))).toBe(8);
    const processes = containerRule(".rm-process-header");
    expect(processes.selectors).toEqual([".rm-process-header", ".rm-process-row"]);
    expect(declares(processes.body, "display", "grid")).toBe(true);
    expect(trackCount(processes)).toBe(6);
    for (const selector of METRIC_LEG_SELECTORS) {
      expect(declares(containerRule(selector).body, "min-width", "0")).toBe(true);
    }
    expect(declaredValue(containerRule(".rm-status-strip"), "grid-template-columns"))
      .toBe("repeat(5, minmax(110px, 1fr))");
    expect(declares(containerRule(".rm-header").body, "flex-direction", "row")).toBe(true);

    // Order, in bytes. @container adds no specificity, so this comparison — and
    // nothing else in criteria 26-29 — is what proves the block's min-width: 0
    // actually beats the base min-width: max-content.
    const blockSelectors = new Set([
      ".rm-group-main",
      ".rm-process-header",
      ".rm-process-row",
      ".rm-status-strip",
      ".rm-header",
      ...METRIC_LEG_SELECTORS,
    ]);
    for (const rule of BASE_RULES) {
      if (rule.selectors.some((s) => blockSelectors.has(s))) {
        expect(
          container.start,
          `${rule.selector} is declared after the @container block`
        ).toBeGreaterThan(rule.end);
      }
    }
  });

  // 27b
  it("keeps the block free of the old narrow-layout bytes", () => {
    const inBlock = CONDITIONAL_RULES.filter((r) => r.atRule === "container");
    for (const rule of inBlock) {
      expect(rule.body).not.toContain("calc(100% - 58px)");
      expect(rule.selector).not.toContain("nth-child");
      if (rule.selectors.includes(".rm-kill-btn")) {
        expect(declaredValue(rule, "width")).toBe("");
      }
    }
    expect(inBlock.some((r) => r.selectors.includes(".rm-kill-btn"))).toBe(false);

    const templated = new Set(
      inBlock
        .filter((r) => /(^|;)\s*grid-template-columns\s*:/i.test(r.body))
        .flatMap((r) => r.selectors)
    );
    expect(templated).toEqual(
      new Set([
        ".rm-group-main",
        ".rm-process-header",
        ".rm-process-row",
        ".rm-status-strip",
      ])
    );
  });

  // 27c
  it("keeps the identity and metric legs disjoint, in closed form", () => {
    const withMaxContent = ALL_RULES.filter((r) =>
      declares(r.body, "min-width", "max-content")
    );
    expect(withMaxContent).toHaveLength(1);
    // A set equality, not three literal checks: the round-4 form fixed three
    // strings and let a descendant or unspaced spelling through alongside.
    expect(new Set(withMaxContent[0].selectors)).toEqual(new Set(METRIC_LEG_SELECTORS));
    expect(withMaxContent[0].atRule).toBeNull();

    // The pill keeps the shared ellipsis rule it already had, outside every
    // at-rule, which is why excluding it from the leg costs nothing.
    const shared = ALL_RULES.find(
      (r) => r.atRule === null && r.selectors.includes(".rm-network-pill")
    );
    if (!shared) throw new Error("missing .rm-network-pill rule");
    expect(declares(shared.body, "min-width", "0")).toBe(true);
    expect(declares(shared.body, "text-overflow", "ellipsis")).toBe(true);
  });

  // 28
  it("removes the floats and grids the group row", () => {
    expect(SCAN.match(/float/g)).toBeNull();

    const row = baseRule(".rm-group-row");
    expect(declares(row.body, "display", "grid")).toBe(true);
    // A whole declaration, so a return to `1fr` fails: 1fr means
    // minmax(auto, 1fr), whose automatic minimum would push the Kill track out.
    expect(declaredValue(row, "grid-template-columns")).toBe("minmax(0, 1fr) 64px");
    // Any gap would come out of the minmax(0, 1fr) track. `gap` alone is not
    // enough to ask for: an anchored regex on `gap` does not match `column-gap`.
    for (const prop of ["gap", "row-gap", "column-gap", "grid-gap"]) {
      expect(
        new RegExp(`(^|;)\\s*${prop}\\s*:`, "i").test(row.body),
        `.rm-group-row declares ${prop}`
      ).toBe(false);
    }
  });

  // 28b
  it("carries the safe wrap layout in the unconditional base", () => {
    for (const selector of [".rm-group-main", ".rm-process-header", ".rm-process-row"]) {
      const rule = baseRule(selector);
      expect(declares(rule.body, "display", "flex")).toBe(true);
      expect(declares(rule.body, "flex-wrap", "wrap")).toBe(true);
      expect(/(^|;)\s*grid-template-columns\s*:/i.test(rule.body)).toBe(false);
    }

    const main = baseRule(".rm-group-main");
    // Leaving the base width alive would cost the first track 64px.
    expect(/(^|;)\s*width\s*:/i.test(main.body)).toBe(false);
    // The 8px token the 56px gap term of the 818px arithmetic rests on.
    expect(declares(main.body, "gap", "var\\(--spacing-sm\\)")).toBe(true);

    const identity = ALL_RULES.find(
      (r) =>
        r.atRule === null &&
        r.selectors.includes(".rm-group-main > .rm-group-identity")
    );
    if (!identity) throw new Error("missing identity leg");
    expect(declares(identity.body, "flex", "1 1 auto")).toBe(true);
    expect(declares(identity.body, "min-width", "0")).toBe(true);

    const metric = baseRule(METRIC_LEG_SELECTORS[0]);
    expect(declares(metric.body, "flex", "0 0 auto")).toBe(true);

    const list = baseRule(".rm-process-list");
    expect(declares(list.body, "grid-column", "1 / -1")).toBe(true);
    expect(/(^|;)\s*clear\s*:/i.test(list.body)).toBe(false);

    expect(trackCount(baseRule(".rm-status-strip"))).toBe(2);
    expect(declares(baseRule(".rm-header").body, "flex-direction", "column")).toBe(true);
  });

  // 29
  it("holds the eight- and six-track templates exactly once, inside the block", () => {
    const eight = ALL_RULES.filter(
      (r) => /(^|;)\s*grid-template-columns\s*:/i.test(r.body) && trackCount(r) === 8
    );
    const six = ALL_RULES.filter(
      (r) => /(^|;)\s*grid-template-columns\s*:/i.test(r.body) && trackCount(r) === 6
    );
    expect(eight).toHaveLength(1);
    expect(six).toHaveLength(1);
    expect(eight[0].atRule).toBe("container");
    expect(six[0].atRule).toBe("container");
  });

  // 30
  it("rings all thirteen focusable selectors, with the two negative offsets", () => {
    const positive = [
      ".rm-action-btn",
      ".rm-filter-seg-btn",
      ".rm-filter-chip",
      ".rm-filter-clear",
      ".rm-titlebar-btn",
      ".rm-filter-pid-input",
      ".rm-filter-search-input",
      ".rm-filter-input-clear",
      ".rm-pid-chip",
      ".rm-sort-field",
      ".rm-sort-direction",
    ];
    // Negative on these two because .rm-group-row carries overflow: hidden and
    // both fill the row, so a positive offset would be clipped to invisibility
    // while a byte test still passed.
    const negative = [".rm-group-main", ".rm-kill-btn"];
    expect(positive).toHaveLength(11);
    expect(positive.length + negative.length).toBe(13);

    for (const selector of positive) {
      const rule = baseRule(`${selector}:focus-visible`);
      expect(declares(rule.body, "outline-offset", "2px")).toBe(true);
    }
    for (const selector of negative) {
      const rule = baseRule(`${selector}:focus-visible`);
      expect(declares(rule.body, "outline-offset", "-2px")).toBe(true);
    }
  });

  // 31
  it("cancels the transitions under reduced motion", () => {
    const media = PARSED.atRules.find((a) => a.name === "media");
    if (!media) throw new Error("missing @media block");
    expect(media.prelude).toBe("@media (prefers-reduced-motion: reduce)");
    const inside = CONDITIONAL_RULES.filter((r) => r.atRule === "media");
    expect(inside.length).toBeGreaterThan(0);
    expect(inside.some((r) => declares(r.body, "transition", "none"))).toBe(true);
  });

  // 32
  it("uses tabular numerals on the tiles, the group row and the process table", () => {
    for (const selector of [".rm-status-strip", ".rm-group-row", ".rm-process-row"]) {
      const rule = baseRule(selector);
      expect(
        declares(rule.body, "font-variant-numeric", "tabular-nums"),
        `${selector} lacks tabular-nums`
      ).toBe(true);
    }
  });

  // 33
  it("uses only var(--...) tokens for colour in every rule it adds", () => {
    for (const selector of NEW_SELECTORS) {
      const rule = baseRule(selector);
      for (const literal of ["#", "rgb(", "rgba(", "hsl("]) {
        expect(
          rule.body.includes(literal),
          `${selector} carries the colour literal ${literal}`
        ).toBe(false);
      }
    }
  });
});
