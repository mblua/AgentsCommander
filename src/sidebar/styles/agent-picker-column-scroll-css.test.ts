import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declarations, declProps, declValue, escapeRe } from "./css-test-helpers";

// #2038 — the two right-hand columns of the Coding Agent profile modal scroll
// independently. jsdom never applies this stylesheet, so the bytes on disk are
// the only place the fix's contract can be pinned: the wide block's three rules
// and their exact declarations, its cascade position after the
// .agent-projection-panel base rule it overrides and outside the narrow block,
// the untouched narrow block, and the provenance of the 220px comparison-table
// floor (the table's own clamp() lower bound, not a new number).
//
// The stylesheet is CRLF on disk. Every regex below is CRLF-safe: `[^}]*`
// spans \r\n freely and the `m` flag anchors ^ at line starts. A multi-line
// literal or a whitespace-spanning regex would match zero times here and pass
// vacuously on an LF checkout, which is why none is used.
//
// THE HELPER CONTRACT, which is load-bearing rather than stylistic: every byte
// extraction in this file throws when it does not find what it is looking for.
// There is no `?? ""` fallback and no optional-chained match anywhere. Without
// that, the absence and count assertions are vacuous — `expect(body ?? "")
// .not.toContain(...)` would pass on the empty string, and a census that
// returned an empty list instead of throwing would pass as zero hits.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");

// Blank out comment spans, preserving length and line breaks so byte indices
// stay comparable between the raw and the scanned copy. Required for
// correctness, not hygiene: the #2038 block's own comment names
// `@media (max-width: 900px)` and `clamp(220px, 42vh, 420px)` in prose, so
// unstripped scans would read the comment instead of the declarations.
function stripComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " "));
}

const CSS_SCAN = stripComments(CSS);

/** The body of the single top-level `selector { ... }` rule. Throws on a miss. */
function ruleBody(selector: string): string {
  const re = new RegExp(`^${escapeRe(selector)} \\{([^}]*)\\}`, "m");
  const match = CSS_SCAN.match(re);
  if (!match) throw new Error(`missing rule: ${selector}`);
  return match[1];
}

// ---------------------------------------------------------------------------
// Media-block and rule scanning.
// ---------------------------------------------------------------------------

const WIDE_HEAD = "@media (min-width: 901px)";
const NARROW_HEAD = "@media (max-width: 900px)";

interface Block {
  /** Byte index of the `@media` text, inside CSS_SCAN. */
  start: number;
  /** Byte index of the matching closing brace, inside CSS_SCAN. */
  end: number;
  /** The block's body, braces excluded. */
  body: string;
}

/** Brace-matches the media block headed by `head`. Throws when it is absent or unterminated. */
function block(head: string): Block {
  const start = CSS_SCAN.indexOf(head);
  if (start === -1) throw new Error(`missing media block: ${head}`);
  const open = CSS_SCAN.indexOf("{", start);
  if (open === -1) throw new Error(`missing opening brace for: ${head}`);
  let depth = 0;
  for (let i = open; i < CSS_SCAN.length; i += 1) {
    if (CSS_SCAN[i] === "{") depth += 1;
    else if (CSS_SCAN[i] === "}") {
      depth -= 1;
      if (depth === 0) return { start, end: i, body: CSS_SCAN.slice(open + 1, i) };
    }
  }
  throw new Error(`unterminated media block: ${head}`);
}

interface ScannedRule {
  selectorList: string;
  body: string;
}

/**
 * Every innermost `selector-list { ... }` in `text`. Rules nested in an @media
 * block are found too; the wrapper itself never yields a match because its body
 * contains braces. Comment spans are assumed already stripped by the caller.
 */
function scanRules(text: string): ScannedRule[] {
  const out: ScannedRule[] = [];
  const re = /([^{}]*)\{([^{}]*)\}/g;
  let m: RegExpExecArray | null = re.exec(text);
  while (m !== null) {
    if (m[1].trim() !== "") {
      out.push({ selectorList: m[1].trim().replace(/\s+/g, " "), body: m[2] });
    }
    m = re.exec(text);
  }
  return out;
}

/** The rules of `current`, with an empty block treated as a miss. */
function blockRules(current: Block): ScannedRule[] {
  const rules = scanRules(current.body);
  if (rules.length === 0) throw new Error("media block contains no rules");
  return rules;
}

/** The one rule in `current` whose selector list is exactly `selector`. Throws on a miss or an ambiguity. */
function blockRule(current: Block, selector: string): ScannedRule {
  const hits = blockRules(current).filter((rule) => rule.selectorList === selector);
  if (hits.length !== 1) {
    throw new Error(`expected exactly one ${selector} rule in the block, found ${hits.length}`);
  }
  return hits[0];
}

const wideBlock = (): Block => block(WIDE_HEAD);
const narrowBlock = (): Block => block(NARROW_HEAD);

const PX = /^(\d+(?:\.\d+)?)px$/;

function px(value: string): number {
  const m = value.trim().match(PX);
  if (!m) throw new Error(`not a px length: ${value}`);
  return Number(m[1]);
}

// ---------------------------------------------------------------------------
// The contract.
// ---------------------------------------------------------------------------

describe("#2038 independent column scroll CSS bytes", () => {
  it("C1. the wide block is exactly the three planned rules with exactly their declarations", () => {
    const rules = blockRules(wideBlock());
    expect(rules.map((r) => r.selectorList)).toEqual([
      ".agent-profile-assignment-scroll",
      ".agent-profile-assignment-scroll > .agent-profile-panel",
      ".agent-projection-panel",
    ]);
    expect(rules.map((r) => declarations(r.body))).toEqual([
      [["grid-template-rows", "minmax(0, 1fr)"]],
      [["overflow-y", "auto"]],
      [["grid-template-rows", "auto auto minmax(220px, 1fr) auto"]],
    ]);
  });

  it("C2. the wide block cascades after the base rule it overrides and after the narrow block", () => {
    const wide = wideBlock();
    const baseRe = new RegExp(`^${escapeRe(".agent-projection-panel")}\\s*\\{`, "m");
    const base = CSS_SCAN.match(baseRe);
    if (!base || base.index === undefined) {
      throw new Error("missing the base .agent-projection-panel rule");
    }
    expect(wide.start).toBeGreaterThan(base.index);
    // Against the narrow block's END, not its head: a wide block nested inside
    // the narrow one would start after the head and would apply only below 901px.
    expect(wide.start).toBeGreaterThan(narrowBlock().end);
  });

  it("C3. the narrow block is untouched", () => {
    const narrow = narrowBlock();
    expect(blockRules(narrow).map((r) => r.selectorList)).toEqual([
      ".agent-profile-assignment-body",
      ".agent-profile-provider-panel",
      ".agent-profile-assignment-scroll",
      ".agent-picker-actions .modal-btn",
    ]);
    const scroll = blockRule(narrow, ".agent-profile-assignment-scroll");
    expect(declValue(scroll.body, "grid-template-columns")).toBe("1fr");
    expect(declValue(scroll.body, "overflow")).toBe("visible");
    expect(declProps(scroll.body)).not.toContain("grid-template-rows");
  });

  it("C4. the wide block carries no !important, no nested @media and no column-1 selector", () => {
    const wide = wideBlock();
    expect(wide.body).not.toContain("!important");
    expect(wide.body).not.toContain("@media");
    for (const rule of blockRules(wide)) {
      expect(rule.selectorList).not.toContain(".agent-profile-provider-panel");
      expect(rule.selectorList).not.toContain(".agent-profile-provider-list");
    }
  });

  it("C5. the base rules the wide block builds on are still present", () => {
    const scroll = ruleBody(".agent-profile-assignment-scroll");
    expect(declValue(scroll, "overflow-y")).toBe("auto");
    expect(declValue(scroll, "grid-template-columns")).toBe(
      "minmax(300px, 0.92fr) minmax(320px, 0.9fr)"
    );
    const panel = ruleBody(".agent-projection-panel");
    expect(declValue(panel, "grid-template-rows")).toBe("auto auto minmax(0, 1fr) auto");
    expect(declValue(panel, "min-height")).toBe("0");
  });

  it("C6. the 220px floor is the comparison table's own clamp() lower bound", () => {
    const maxHeight = declValue(ruleBody(".agent-comparison-table"), "max-height");
    const clamp = maxHeight.match(/^clamp\(([^,]+),/);
    if (!clamp) throw new Error(`not a clamp() value: ${maxHeight}`);
    const track = declValue(blockRule(wideBlock(), ".agent-projection-panel").body, "grid-template-rows");
    const floor = track.match(/minmax\(([^,]+),/);
    if (!floor) throw new Error(`no minmax floor in: ${track}`);
    expect(px(floor[1])).toBe(px(clamp[1]));
  });

  it("C7. exactly one rule in the sheet declares minmax(220px, 1fr)", () => {
    const FLOOR = /minmax\(\s*220px\s*,\s*1fr\s*\)/;
    const hits = scanRules(CSS_SCAN).filter((rule) => FLOOR.test(rule.body));
    expect(hits.map((r) => r.selectorList)).toEqual([".agent-projection-panel"]);
  });
});
