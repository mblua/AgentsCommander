import { describe, expect, it } from "vitest";

// #1167 - acceptance criterion 4: the sidebar coding-agent badge has ONE constant
// style, so no rule anywhere may colour .agent-badge by label. The four per-TOOL
// rules that survive are the Open-Agent modal's repo chips, anchored on
// .agent-modal-item-badges, and they are pinned here too so the anchor cannot be
// widened back into the sidebar by accident.
//
// The glob still defines the file set (all 12 CSS files under src/, the same depth
// idiom as sidebar/watchdog/no-presentation-import.test.ts), but the CONTENT is
// read from disk: Vitest replaces every CSS module with `export default ""` unless
// `test.css` is enabled, and `?raw` does not opt out because Vite's isCSSRequest()
// matches "sidebar.css?raw" too. Measured on this tree, the glob returned all 12
// keys with "" as the value, which made the second pin fail with 0 selectors and
// would have made the first pass vacuously forever. `@types/node` is not installed
// in this frontend tsconfig, so node:fs comes in through a non-literal specifier
// and is narrowed to the single function this guard needs.
const CSS_FILES = Object.keys(
  import.meta.glob<string>("../../**/*.css", {
    query: "?raw",
    import: "default",
    eager: true,
  }),
);

interface NodeFileReader {
  readonly readFileSync: (file: URL, encoding: "utf8") => string;
}

const FS_SPECIFIER = "node:fs";
const nodeFs = (await import(/* @vite-ignore */ FS_SPECIFIER)) as unknown as NodeFileReader;

const CSS_SOURCES: Record<string, string> = Object.fromEntries(
  CSS_FILES.map((file) => [file, nodeFs.readFileSync(new URL(file, import.meta.url), "utf8")]),
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
  // Guards the guard: an empty source set makes every selector assertion below
  // pass vacuously, which is exactly how the stylesheet-stubbing bug hid itself.
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
