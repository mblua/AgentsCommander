import { describe, expect, it } from "vitest";
import { declProps, declValue, scanSheet, soleRuleBody } from "./css-test-helpers";

// #2584 - the Favorites header is a flex item of a capped flex column and also
// carries the label's `overflow: hidden`, so its automatic minimum size is 0.
// When favorites overflow the 40% cap it shrank to ~6px and clipped its text.
// jsdom does no layout, so the stylesheet bytes are the contract.
const RULES = scanSheet(new URL("./sidebar.css", import.meta.url));
const ruleFor = (selector: string): string => soleRuleBody(RULES, selector);

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
