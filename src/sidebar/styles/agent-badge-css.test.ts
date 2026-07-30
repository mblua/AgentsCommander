import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #1167 - acceptance criterion 4: the sidebar coding-agent badge has ONE constant
// style, so no rule anywhere may colour .agent-badge by label. The four per-TOOL
// rules that survive are the Open-Agent modal's repo chips, anchored on
// .agent-modal-item-badges, and they are pinned here too so the anchor cannot be
// widened back into the sidebar by accident.
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

const SELECTOR_LINE_RE = /^[^{}]*\{/gm;

function selectorsMentioning(needle: string): string[] {
  const hits: string[] = [];
  for (const [file, source] of Object.entries(CSS_SOURCES)) {
    const withoutComments = source.replace(/\/\*[\s\S]*?\*\//g, "");
    for (const match of withoutComments.matchAll(SELECTOR_LINE_RE)) {
      const selector = match[0].slice(0, -1).trim();
      if (selector.includes(needle)) hits.push(`${file.replace(/\\/g, "/")}: ${selector}`);
    }
  }
  return hits.sort();
}

describe("coding-agent badge CSS (#1167)", () => {
  // Guards the guard, and runs first on purpose: an empty source set makes every
  // selector assertion below pass vacuously, which is exactly how the CSS-stubbing
  // trap hid itself the first time this file was written.
  it("reads real stylesheet text for every file it globs", () => {
    expect(CSS_FILES.length).toBeGreaterThan(0);
    expect(Object.values(CSS_SOURCES).every((source) => source.length > 0)).toBe(true);
    expect(CSS_SOURCES["./sidebar.css"]).toContain(".agent-badge {");
  });

  it("has no per-agent colour rule on .agent-badge", () => {
    const offenders = selectorsMentioning(".agent-badge").filter((s) => s.includes("[data-agent"));
    expect(offenders).toEqual([]);
  });

  it("keeps every surviving data-agent rule scoped to the Open-Agent modal", () => {
    const dataAgentSelectors = selectorsMentioning("[data-agent");
    expect(dataAgentSelectors.every((s) => s.includes(".agent-modal-item-badges "))).toBe(true);
    expect(dataAgentSelectors).toHaveLength(4);
  });
});
