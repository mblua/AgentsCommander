import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #1167 - acceptance criterion 4: the sidebar coding-agent badge has ONE constant
// style, so no rule anywhere may colour .agent-badge by label. The four per-TOOL
// rules that survive are the Open-Agent modal's repo chips, anchored on
// .agent-modal-item-badges, and they are pinned here too so the anchor cannot be
// widened back into the sidebar by accident. The emerald rule that all three sidebar
// sites now resolve through is pinned as well: jsdom never applies the stylesheet, so
// deleting that rule breaks every row's appearance without a single test noticing.
//
// The glob supplies the FILE SET and nothing else. It is deliberately NOT eager and
// carries no ?raw query: under Vitest every CSS module evaluates to
// `export default ""` unless test.css is enabled, and ?raw does not opt out, because
// Vite's isCSSRequest() also matches "sidebar.css?raw". Measured on this tree, the
// eager ?raw form returned all 12 keys with "" as every single value, which made the
// selector pins below pass vacuously on an empty stylesheet. The bytes therefore come
// from disk; node:fs is typed in src/vite-env.d.ts because @types/node is not a
// dependency of this frontend.
const CSS_FILES = Object.keys(import.meta.glob("../../**/*.css"));

const CSS_SOURCES: Record<string, string> = Object.fromEntries(
  CSS_FILES.map((file) => [file, readFileSync(new URL(file, import.meta.url), "utf8")]),
);

type RuleOpening = { selectorList: string; bodyStart: number };
type Compound = { file: string; selector: string };

const COMMENT_RE = /\/\*[\s\S]*?\*\//g;
const BRACE_RE = /[{}]/g;
const MODAL_ANCHOR = ".agent-modal-item-badges";

// Every rule opening in the file, not just the first one on each line. A line-anchored
// scan is what used to make this guard blind to `.a { } .evil[data-agent="X"] { }` and
// to any minified stylesheet, so there is no line boundary anywhere in this scanner: a
// selector list is the text between the previous brace of EITHER kind and the `{` that
// opens the rule. Nesting and at-rules fall out of that for free.
function ruleOpenings(source: string): RuleOpening[] {
  const openings: RuleOpening[] = [];
  let segmentStart = 0;
  for (const match of source.matchAll(BRACE_RE)) {
    const at = match.index ?? 0;
    if (match[0] === "{") {
      openings.push({ selectorList: source.slice(segmentStart, at), bodyStart: at + 1 });
    }
    segmentStart = at + 1;
  }
  return openings;
}

function collapseWhitespace(text: string): string {
  return text.replace(/\s+/g, " ");
}

// CSS allows whitespace inside an attribute selector, so `[ data-agent = "Claude" ]`
// is the same selector as `[data-agent="Claude"]` and browsers honour both. Fold that
// away before any needle is applied. Descendant spaces are preserved: they carry
// meaning, and the modal anchor below depends on them.
function normaliseSelector(selector: string): string {
  return collapseWhitespace(selector)
    .replace(/\[\s+/g, "[")
    .replace(/\s+\]/g, "]")
    .replace(/\s*([~^$*|]?=)\s*/g, "$1")
    .trim();
}

// Criterion 5 is about individual compound selectors, not selector lists: a rule is
// widened back into the sidebar by adding one comma-separated compound next to the
// modal-anchored one, and a check that only asks whether the whole list mentions the
// anchor cannot see that. Depth-aware because the commas in :is(a, b) / :has(a, b) are
// not list separators. No functional pseudo-class in the tree contains a comma today,
// so this costs nothing now and cannot silently mangle a compound later.
function splitSelectorList(selectorList: string): string[] {
  const compounds: string[] = [];
  let depth = 0;
  let current = "";
  for (const ch of selectorList) {
    if (ch === "(") depth += 1;
    else if (ch === ")") depth = Math.max(0, depth - 1);
    if (ch === "," && depth === 0) {
      compounds.push(current);
      current = "";
      continue;
    }
    current += ch;
  }
  compounds.push(current);
  return compounds.map(normaliseSelector).filter((compound) => compound.length > 0);
}

const ALL_COMPOUNDS: Compound[] = Object.entries(CSS_SOURCES).flatMap(([file, source]) => {
  const withoutComments = source.replace(COMMENT_RE, "");
  return ruleOpenings(withoutComments).flatMap((opening) =>
    splitSelectorList(opening.selectorList).map((selector) => ({
      file: file.replace(/\\/g, "/"),
      selector,
    })),
  );
});

function report(compounds: Compound[]): string[] {
  return compounds.map(({ file, selector }) => `${file}: ${selector}`).sort();
}

// Attribute NAMES are ASCII case-insensitive against HTML elements, so a selector
// written [DATA-AGENT="Claude"] really does match a chip and has to count as a hit.
const DATA_AGENT_COMPOUNDS = ALL_COMPOUNDS.filter((compound) =>
  compound.selector.toLowerCase().includes("[data-agent"),
);

// Scoped means the attribute selector is a DESCENDANT of the modal container: the
// anchor must be present, be followed by a combinator, and come before the
// [data-agent] part. `.session-item-meta [data-agent="X"]` fails, and so does
// `[data-agent="X"] .agent-modal-item-badges`.
function isModalScoped(selector: string): boolean {
  const at = selector.indexOf(MODAL_ANCHOR);
  if (at === -1) return false;
  const afterAnchor = selector.slice(at + MODAL_ANCHOR.length);
  return /^[\s>+~]/.test(afterAnchor) && afterAnchor.toLowerCase().includes("[data-agent");
}

// The declarations of the first rule whose selector list contains `wanted` as a whole
// compound selector, whitespace-collapsed. Exact-compound on purpose: a themed
// override like `html.light-theme .ac-discovery-badge.agent` is a different rule.
function declarationsOf(source: string, wanted: string): string | null {
  const withoutComments = source.replace(COMMENT_RE, "");
  for (const opening of ruleOpenings(withoutComments)) {
    if (!splitSelectorList(opening.selectorList).includes(wanted)) continue;
    const end = withoutComments.indexOf("}", opening.bodyStart);
    if (end === -1) return null;
    return collapseWhitespace(withoutComments.slice(opening.bodyStart, end)).trim();
  }
  return null;
}

describe("coding-agent badge CSS (#1167)", () => {
  // Guards the guard, and runs first on purpose: an empty source set makes every
  // assertion below pass vacuously, which is exactly how the CSS-stubbing trap hid
  // itself the first time this file was written. The compound count covers the second
  // way to go vacuous: bytes read, but a scanner that parses nothing out of them.
  it("reads real stylesheet text for every file it globs", () => {
    expect(CSS_FILES.length).toBeGreaterThan(0);
    expect(Object.values(CSS_SOURCES).every((source) => source.length > 0)).toBe(true);
    expect(CSS_SOURCES["./sidebar.css"]).toContain(".agent-badge {");
    expect(ALL_COMPOUNDS.length).toBeGreaterThan(100);
  });

  it("has no per-agent colour rule on .agent-badge", () => {
    const offenders = DATA_AGENT_COMPOUNDS.filter((compound) =>
      compound.selector.includes(".agent-badge"),
    );
    expect(report(offenders)).toEqual([]);
  });

  it("keeps every surviving data-agent rule scoped to the Open-Agent modal", () => {
    const escaped = DATA_AGENT_COMPOUNDS.filter((compound) => !isModalScoped(compound.selector));
    expect(report(escaped)).toEqual([]);
    expect(report(DATA_AGENT_COMPOUNDS)).toHaveLength(4);
  });

  it("keeps the emerald rule every sidebar row now resolves through", () => {
    const declarations = declarationsOf(CSS_SOURCES["./sidebar.css"], ".ac-discovery-badge.agent");
    expect(declarations).not.toBeNull();
    expect(declarations).toContain("background: rgba(16, 185, 129, 0.14);");
    expect(declarations).toContain("color: #34d399;");
    expect(declarations).toContain("text-transform: none;");
  });
});
