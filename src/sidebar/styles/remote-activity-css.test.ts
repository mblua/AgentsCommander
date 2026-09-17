import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #2064 — the CI ring and the stale bar are two `box-shadow` layers, and the bytes
// on disk are the only place that can be pinned: the renderer never attaches
// `sidebar.css` (`ProjectPanel.menu-guard.test.tsx:455-458`), javascript-DOM does
// not substitute `var()`, so a computed `box-shadow` stays raw, and both computed
// backgrounds of these rules read rgba(0, 0, 0, 0) no matter what the file says.
// A round-2 version of these three tests used `getComputedStyle` and none of them
// could fail. Five precedents live in this directory; the shape copied is
// `coord-quick-access-css.test.ts` — rule bodies by anchored selector, declarations
// split on `;`, and every extraction helper THROWS when it matches nothing, so a
// zero match can never pass.
//
// CRLF: the working tree checks out CRLF (`.gitattributes` leaves `sidebar.css`
// unspecified and `core.autocrlf=true` on this host), which is the hazard
// `working-tint-css.test.ts:11-21` describes — a CRLF-sensitive regex silently
// matches nothing and the test passes vacuously. This file chooses normalization
// instead: the text is normalized ONCE, below, and every literal here is LF.
// Test 14 asserts `CSS` carries no `\r` after that, so both checkouts compare the
// same bytes.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8").replace(
  /\r\n/g,
  "\n"
);

/** Comment spans blanked, length and line breaks preserved: the #2064 comment above
 *  the rules names `ci-running` and `stale` in prose, and an unstripped scan would
 *  read a sentence as a selector. */
const NO_COMMENTS = CSS.replace(/\/\*[\s\S]*?\*\//g, (span) => span.replace(/[^\n]/g, " "));

interface CssRule {
  selector: string;
  body: string;
}

function matchingBrace(source: string, open: number): number {
  let depth = 0;
  for (let i = open; i < source.length; i += 1) {
    if (source[i] === "{") depth += 1;
    else if (source[i] === "}") {
      depth -= 1;
      if (depth === 0) return i;
    }
  }
  throw new Error(`no closing brace for the block opening at ${open}`);
}

/** Every declaration block in the SOURCE, at any nesting depth, so rules inside an
 *  `@media` wrapper are scanned instead of being skipped by a flat regex. */
function allRules(source: string): CssRule[] {
  const rules: CssRule[] = [];
  const walk = (from: number, to: number): void => {
    let i = from;
    while (i < to) {
      const open = source.indexOf("{", i);
      if (open < 0 || open >= to) break;
      const close = matchingBrace(source, open);
      const prelude = source.slice(i, open).trim();
      if (prelude.startsWith("@")) walk(open + 1, close);
      else rules.push({ selector: prelude, body: source.slice(open + 1, close) });
      i = close + 1;
    }
  };
  walk(0, source.length);
  return rules;
}

const RULES = allRules(NO_COMMENTS);

/** The rules that paint the repo chip's CI or staleness marker. `.ci-running` and
 *  `.stale` appear in exactly these selectors in this file, so a rule reintroduced
 *  under a different selector is still caught here. */
function chipSignalRules(): CssRule[] {
  return RULES.filter(
    (rule) =>
      rule.selector.includes(".ac-discovery-badge.branch") &&
      (rule.selector.includes(".ci-running") || rule.selector.includes(".stale"))
  );
}

function ruleFor(selector: string): CssRule {
  const hits = RULES.filter((rule) => rule.selector === selector);
  if (hits.length !== 1) {
    throw new Error(`expected exactly one rule for ${JSON.stringify(selector)}, found ${hits.length}`);
  }
  return hits[0];
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

/** A layout-occupying declaration would move the multi-repo row: `box-shadow` never
 *  does, and `inset` least of all, which is why the bar can be 3px wide for free. */
const LAYOUT_OCCUPYING = /^(width|min-width|max-width|border|border-left.*|padding.*|margin.*|outline.*)$/;

/** The exact bodies of the two rules this phase must NOT change, read from the file
 *  at implementation time (LF, after the normalization above). */
const BRANCH_BODY =
  "\n  background: rgba(139, 92, 246, 0.15);\n  color: #a78bfa;\n  text-transform: none;\n";
const BRANCH_DIRTY_BODY = "\n  color: var(--status-exited);\n";

const COMPOSED_SELECTOR =
  ".ac-discovery-badge.branch.ci-running,\n.ac-discovery-badge.branch.stale";

describe("#2064 remote-activity chip markers (bytes on disk)", () => {
  it("stale_marker_occupies_no_layout_box", () => {
    const rules = chipSignalRules();
    expect(rules.map((rule) => rule.selector)).toEqual([
      COMPOSED_SELECTOR,
      ".ac-discovery-badge.branch.ci-running",
      ".ac-discovery-badge.branch.stale",
    ]);

    const offenders = rules.flatMap((rule) =>
      declarations(rule.body)
        .map(([name]) => name)
        .filter((name) => LAYOUT_OCCUPYING.test(name))
        .map((name) => `${rule.selector} -> ${name}`)
    );
    expect(offenders).toEqual([]);
  });

  it("remote_activity_rules_declare_no_background", () => {
    // The regression check behind section 5: the rejected
    // `background: rgba(234, 179, 8, 0.20)` goes red here, and while this is green
    // no text contrast ratio can have moved, because neither marker composites.
    const offenders = chipSignalRules().flatMap((rule) =>
      declarations(rule.body)
        .map(([name]) => name)
        .filter((name) => name.startsWith("background"))
        .map((name) => `${rule.selector} -> ${name}`)
    );
    expect(offenders).toEqual([]);

    // The two neighbouring rules whose background must survive untouched: an
    // additive marker must not be achieved by editing the shared `.branch` rule,
    // which also paints AcDiscoveryPanel's chip.
    expect(ruleFor(".ac-discovery-badge.branch").body).toBe(BRANCH_BODY);
    expect(ruleFor(".ac-discovery-badge.branch.dirty").body).toBe(BRANCH_DIRTY_BODY);
  });

  it("both_markers_compose_through_one_declaration", () => {
    expect(CSS.includes("\r")).toBe(false);

    const boxShadows = chipSignalRules().flatMap((rule) =>
      declarations(rule.body)
        .filter(([name]) => name === "box-shadow")
        .map(([, value]) => ({ selector: rule.selector, value }))
    );
    // Exactly ONE declaration, in the composed rule: two competing `box-shadow`
    // declarations would silently drop the stale bar or the ring for a repo that is
    // both stale and running, and source order would decide which.
    expect(boxShadows).toEqual([
      {
        selector: COMPOSED_SELECTOR,
        value: "var(--ac-badge-ci-shadow, 0 0 #0000), var(--ac-badge-stale-shadow, 0 0 #0000)",
      },
    ]);

    // Each state rule sets only its own custom property and nothing else, so the
    // trimmed body IS the whole declaration list.
    expect(ruleFor(".ac-discovery-badge.branch.ci-running").body.trim()).toBe(
      "--ac-badge-ci-shadow: inset 0 0 0 1px rgba(234, 179, 8, 0.85);"
    );
    expect(ruleFor(".ac-discovery-badge.branch.stale").body.trim()).toBe(
      "--ac-badge-stale-shadow: inset 3px 0 0 var(--status-blocked);"
    );
  });
});
