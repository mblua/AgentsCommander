import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declarations, declProps, declValue, scanRules, type ScannedRule } from "./css-test-helpers";

// #2577 Amendment A - the Assign profile picker rows keep their own height and
// the reorder grip sits inside the card border. jsdom never applies
// sidebar.css, so the file bytes are the only place to pin that contract.
// Comments are stripped first: the scanner would fold a leading comment into
// the first selector. Every lookup throws on a miss.

const css = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const rules = scanRules(css);

const sameSelectors = (rule: ScannedRule, selectors: string[]): boolean =>
  rule.selectors.length === selectors.length && rule.selectors.every((s, i) => s === selectors[i]);

function rulesWith(selectors: string[]): ScannedRule[] {
  return rules.filter((rule) => sameSelectors(rule, selectors));
}

function soleRule(selectors: string[]): ScannedRule {
  const found = rulesWith(selectors);
  if (found.length !== 1) throw new Error(`expected one rule for ${selectors.join(", ")}, found ${found.length}`);
  return found[0];
}

const px = (value: string): number => {
  const m = /^(-?\d+(?:\.\d+)?)px\b/.exec(value);
  if (m === null) throw new Error(`not a leading px value: ${value}`);
  return Number(m[1]);
};

describe("#2577 picker reorder CSS", () => {
  it("picker list packs rows at the top without editing the shared list rule", () => {
    const picker = soleRule([".agent-picker-modal .agent-profile-provider-list"]);
    expect(declarations(picker.body)).toEqual([
      ["position", "relative"],
      ["align-content", "start"],
    ]);

    const shared = soleRule([".agent-profile-provider-list", ".agent-profile-card-list"]);
    expect(declProps(shared.body)).toEqual(["display", "gap", "min-width", "min-height", "overflow-y"]);
  });

  it("card wrap is a positioned block, never a flex centerer", () => {
    const centerers = rules.filter(
      (rule) =>
        rule.selectors.some((s) => s.endsWith(".agent-profile-provider-card-wrap")) &&
        declProps(rule.body).some((p) => p === "display" || p === "align-items"),
    );
    expect(centerers).toEqual([]);

    const positioned = rulesWith([".agent-profile-provider-list > .agent-profile-provider-card-wrap"]).filter(
      (rule) => declProps(rule.body).includes("position"),
    );
    expect(positioned).toHaveLength(1);
    expect(declValue(positioned[0].body, "position")).toBe("relative");

    const bare = rulesWith([".agent-profile-provider-card-wrap"]).filter((rule) =>
      declProps(rule.body).includes("position"),
    );
    expect(bare).toEqual([]);
  });

  it("grip sits inside the card border", () => {
    const grip = soleRule([".agent-profile-provider-card-wrap > .agent-profile-provider-drag-handle"]);
    expect(declValue(grip.body, "position")).toBe("absolute");
    expect(declValue(grip.body, "top")).toBe("50%");
    expect(declValue(grip.body, "left")).toBe("3px");
    expect(declValue(grip.body, "z-index")).toBe("1");
    expect(declValue(grip.body, "transform")).toBe("translateY(-50%)");

    const card = soleRule([".agent-profile-provider-card-wrap > .agent-profile-provider-card"]);
    expect(declValue(card.body, "padding-left")).toBe("22px");

    const base = soleRule([".settings-agent-drag-handle", ".agent-profile-provider-drag-handle"]);
    const width = px(declValue(base.body, "width"));
    expect(width).toBe(16);

    const cardBase = soleRule([".agent-profile-provider-card", ".agent-profile-card"]);
    const border = px(declValue(cardBase.body, "border"));
    expect(border).toBe(1);

    const left = px(declValue(grip.body, "left"));
    const paddingLeft = px(declValue(card.body, "padding-left"));
    expect(left).toBeGreaterThanOrEqual(border);
    expect(left + width).toBeLessThan(border + paddingLeft);
  });
});
