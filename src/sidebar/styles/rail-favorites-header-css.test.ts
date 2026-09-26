import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declProps, declValue, scanRules } from "./css-test-helpers";

// #2584 - the Favorites header is a flex item of a capped flex column and also
// carries the label's `overflow: hidden`, so its automatic minimum size is 0.
// When favorites overflow the 40% cap it shrank to ~6px and clipped its text.
// jsdom does no layout, so the stylesheet bytes are the contract.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const CSS_SCAN = CSS.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " "));
const RULES = scanRules(CSS_SCAN);

/** Body of the single rule whose selector list contains `selector`. Throws unless exactly one. */
function ruleFor(selector: string): string {
  const hits = RULES.filter((r) => r.selectors.includes(selector));
  if (hits.length !== 1) throw new Error(`expected 1 rule for ${selector}, got ${hits.length}`);
  return hits[0].body;
}

const HEADER = ".workgroup-group-rail-favorites > .workgroup-group-rail-header";

describe("#2584 rail Favorites header", () => {
  it("T1 header never shrinks below its content height", () => {
    expect(declValue(ruleFor(HEADER), "flex-shrink")).toBe("0");
  });

  it("T2 the cause is still present, so T1 stays meaningful", () => {
    expect(declValue(ruleFor(".workgroup-group-rail-project-label"), "overflow")).toBe("hidden");
    const favorites = ruleFor(".workgroup-group-rail-favorites");
    expect(declValue(favorites, "display")).toBe("flex");
    expect(declValue(favorites, "flex-direction")).toBe("column");
  });

  it("T3 no flex shorthand in the header rule can reset flex-shrink", () => {
    expect(declProps(ruleFor(HEADER))).not.toContain("flex");
  });
});
