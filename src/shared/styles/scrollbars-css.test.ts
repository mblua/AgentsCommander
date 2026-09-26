import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declProps, declValue, scanRules, soleRuleBody as ruleFor, type ScannedRule } from "../../sidebar/styles/css-test-helpers";

// #2579 - one Noir thin scrollbar for every scroller in every window. The
// values live only in scrollbars.css; the two standard scrollbar properties are
// banned because on Chromium 121+ they win over the pseudo-element rules.
// jsdom never draws scrollbars, so the stylesheet bytes are the contract.

const SRC = new URL("../../", import.meta.url);
const read = (rel: string): string => readFileSync(new URL(rel, SRC), "utf8");

/** Blank (a) comments and (b) at-rule statements, keeping offsets and line breaks. */
const blank = (m: string): string => m.replace(/[^\r\n]/g, " ");
const mask = (css: string): string =>
  css.replace(/\/\*[\s\S]*?\*\//g, blank).replace(/@[a-z-]+[^;{}]*;/gi, blank);

// Keys only (never loaded); bytes come from disk as in agent-badge-css.test.ts.
const SHEETS = Object.keys(import.meta.glob("../../**/*.css"))
  .map((p) => `src/${new URL(p, import.meta.url).href.slice(SRC.href.length)}`)
  .sort();

const rulesOf = (path: string): ScannedRule[] => scanRules(mask(read(path.slice(4))));
const GLOBAL = rulesOf("src/shared/styles/scrollbars.css");

describe("#2579 global Noir scrollbars", () => {
  it("G1 values", () => {
    const bar = ruleFor(GLOBAL, "::-webkit-scrollbar");
    expect(declValue(bar, "width")).toBe("4px");
    expect(declValue(bar, "height")).toBe("4px");
    expect(declValue(ruleFor(GLOBAL, "::-webkit-scrollbar-button"), "display")).toBe("none");
    expect(declValue(ruleFor(GLOBAL, "::-webkit-scrollbar-track"), "background")).toBe("transparent");
    expect(declValue(ruleFor(GLOBAL, "::-webkit-scrollbar-corner"), "background")).toBe("transparent");
    const thumb = ruleFor(GLOBAL, "::-webkit-scrollbar-thumb");
    expect(declValue(thumb, "background")).toBe("var(--sidebar-border)");
    expect(declValue(thumb, "border-radius")).toBe("2px");
  });

  it("G2 exclusivity: no standard scrollbar property in any of the 13 sheets", () => {
    // The list is pinned so a new or renamed sheet cannot slip past G2/G3.
    expect(SHEETS).toEqual([
      "src/browser/styles/browser.css",
      "src/main/styles/main.css",
      "src/resource-monitor/styles/resource-monitor.css",
      "src/screenshot-overlay/styles/screenshot-overlay.css",
      "src/shared/styles/external-link-confirm.css",
      "src/shared/styles/scrollbars.css",
      "src/shared/styles/toast.css",
      "src/sidebar/styles/sidebar.css",
      "src/sidebar/styles/variables.css",
      "src/spec-board/styles/spec-board.css",
      "src/terminal/styles/terminal.css",
      "src/terminal/styles/variables.css",
      "src/watchers/styles/watchers.css",
    ]);
    const hits = SHEETS.flatMap((p) =>
      rulesOf(p).flatMap((r) =>
        declProps(r.body)
          .filter((d) => d === "scrollbar-width" || d === "scrollbar-color")
          .map((d) => `${p}: ${d}`),
      ),
    );
    expect(hits).toEqual([]);
  });

  it("G3 single source: only the five bare pseudo-element rules exist", () => {
    const hits = SHEETS.flatMap((p) =>
      rulesOf(p)
        .filter((r) => r.selectors.some((s) => s.includes("::-webkit-scrollbar")))
        .map((r) => `${p}: ${r.selectors.join(", ")}`),
    );
    expect(hits).toEqual([
      "src/shared/styles/scrollbars.css: ::-webkit-scrollbar",
      "src/shared/styles/scrollbars.css: ::-webkit-scrollbar-button",
      "src/shared/styles/scrollbars.css: ::-webkit-scrollbar-track",
      "src/shared/styles/scrollbars.css: ::-webkit-scrollbar-corner",
      "src/shared/styles/scrollbars.css: ::-webkit-scrollbar-thumb",
    ]);
  });

  it("G4 reach: main.tsx imports the sheet before everything else", () => {
    const main = read("main.tsx");
    const at = main.indexOf('import "./shared/styles/scrollbars.css";');
    const capture = main.indexOf('import "./shared/console-capture"');
    expect(at).toBeGreaterThanOrEqual(0);
    expect(capture).toBeGreaterThan(at);
  });

  it("G5 xterm slider: 4px geometry in CSS, dark colors in the theme", () => {
    const term = rulesOf("src/terminal/styles/terminal.css");
    const bar = ruleFor(term, ".xterm .xterm-scrollable-element > .scrollbar");
    expect(declValue(bar, "width")).toBe("4px !important");
    const slider = ruleFor(term, ".xterm .xterm-scrollable-element > .scrollbar > .slider");
    expect(declValue(slider, "width")).toBe("4px !important");
    expect(declValue(slider, "border-radius")).toBe("2px");
    const opts = read("terminal/components/terminal-options.ts");
    expect(opts).toContain('scrollbarSliderBackground: "rgba(255, 255, 255, 0.06)"');
    expect(opts).toContain('scrollbarSliderHoverBackground: "rgba(255, 255, 255, 0.12)"');
    expect(opts).toContain('scrollbarSliderActiveBackground: "rgba(255, 255, 255, 0.18)"');
  });
});
