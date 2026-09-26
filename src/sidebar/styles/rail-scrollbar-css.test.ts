import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declProps, declValue, scanRules } from "./css-test-helpers";

// #2578 — the compact rail's Favorites and project scrollers draw their bars on
// one x line at the rail's inner edge, in the thin Noir style of
// .sidebar-scrollable. jsdom never renders scrollbars, so the stylesheet bytes
// are the contract. scanRules() is brace-based and so CRLF-safe; every helper
// throws on a miss, so no assertion below can pass on an empty match.
// #2579 moved the scrollbar values to src/shared/styles/scrollbars.css; they are
// now owned by src/shared/styles/scrollbars-css.test.ts (G1/G3), and T4 here
// asserts the rails inherit that global rule.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const CSS_SCAN = CSS.replace(/\/\*[\s\S]*?\*\//g, (m) => m.replace(/[^\r\n]/g, " "));
const RULES = scanRules(CSS_SCAN);

/** Body of the single rule whose selector list contains `selector`. Throws unless exactly one. */
function ruleFor(selector: string): string {
  const hits = RULES.filter((r) => r.selectors.includes(selector));
  if (hits.length !== 1) throw new Error(`expected 1 rule for ${selector}, got ${hits.length}`);
  return hits[0].body;
}

const SCROLLERS = [".workgroup-group-rail-scroll", ".workgroup-group-rail-favorites-scroll"];

describe("#2578 rail scrollbars", () => {
  it("T1 Favorites container has no horizontal padding", () => {
    expect(declValue(ruleFor(".workgroup-group-rail-favorites"), "padding")).toBe("6px 0 4px");
  });

  it("T2 both scrollers carry the 4px horizontal padding themselves", () => {
    expect(declValue(ruleFor(".workgroup-group-rail-favorites-scroll"), "padding")).toBe("0 4px");
    expect(declValue(ruleFor(".workgroup-group-rail-scroll"), "padding")).toBe("6px 4px");
  });

  it("T3 Favorites header keeps a 4px side indent without touching its 5px bottom gap", () => {
    const body = ruleFor(".workgroup-group-rail-favorites > .workgroup-group-rail-header");
    expect(declValue(body, "width")).toBe("auto");
    expect(declValue(body, "margin-left")).toBe("4px");
    expect(declValue(body, "margin-right")).toBe("4px");
    const props = declProps(body);
    for (const p of ["margin", "margin-top", "margin-bottom"]) expect(props).not.toContain(p);
    expect(declValue(ruleFor(".workgroup-group-rail-project-label"), "margin-bottom")).toBe("5px");
  });

  it("T4 both scrollers use the thin Noir webkit scrollbar", () => {
    // #2579: no rail-level pseudo-element rule may survive; one would outrank the
    // global Noir rule in scrollbars.css.
    for (const s of SCROLLERS) {
      expect(
        RULES.filter((r) => r.selectors.some((x) => x.startsWith(s) && x.includes("::-webkit-scrollbar"))),
      ).toEqual([]);
    }
  });

  it("T5 no standard scrollbar property can override the webkit rules", () => {
    const railRules = RULES.filter((r) =>
      r.selectors.some((s) => SCROLLERS.some((sc) => s.includes(sc.slice(1)))),
    );
    expect(railRules.length).toBeGreaterThanOrEqual(2);
    for (const r of railRules) {
      expect(declProps(r.body)).not.toContain("scrollbar-width");
      expect(declProps(r.body)).not.toContain("scrollbar-color");
    }
    const all = RULES.flatMap((r) => declProps(r.body));
    expect(all.filter((p) => p === "scrollbar-width" || p === "scrollbar-color")).toEqual([]);
  });
});
