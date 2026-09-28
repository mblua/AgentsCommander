import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import {
  AGENT_QUOTA_TOOLTIP_PREFIX,
  agentQuotaTitle,
  quotaChipAttrs,
  quotaFill,
  quotaRemainingLabel,
} from "./agent-quota";

const BAD_NUMBERS = [NaN, Infinity, -Infinity, -1, 101, 12.5];

describe("quotaFill", () => {
  it("a_reading_of_28_gives_72_remaining_and_28_used", () => {
    expect(quotaFill(28)).toEqual({ remaining: 72, used: 28 });
  });

  it("zero_and_one_hundred_are_real_readings_and_return_a_fill", () => {
    expect(quotaFill(0)).toEqual({ remaining: 100, used: 0 });
    expect(quotaFill(100)).toEqual({ remaining: 0, used: 100 });
  });

  it("null_and_undefined_return_null", () => {
    expect(quotaFill(null)).toBeNull();
    expect(quotaFill(undefined)).toBeNull();
  });

  it("nan_infinity_negative_over_one_hundred_and_fractional_return_null", () => {
    for (const v of BAD_NUMBERS) {
      expect(quotaFill(v)).toBeNull();
    }
  });
});

describe("agentQuotaTitle", () => {
  it("the_title_names_both_halves", () => {
    expect(agentQuotaTitle(28)).toBe(`${AGENT_QUOTA_TOOLTIP_PREFIX}: 72% remaining, 28% used`);
  });
});

describe("quotaChipAttrs", () => {
  it("attrs_without_a_reading_are_exactly_the_class_and_nothing_else", () => {
    for (const v of [null, undefined, ...BAD_NUMBERS]) {
      const attrs = quotaChipAttrs("Claude Code", v);
      expect(Object.keys(attrs)).toEqual(["class"]);
      expect(attrs).toEqual({ class: "ac-discovery-badge agent" });
    }
  });

  it("attrs_with_a_reading_keep_the_agent_label_in_the_accessible_name", () => {
    const attrs = quotaChipAttrs("Claude Code", 28);
    expect(attrs.class).toBe("ac-discovery-badge agent quota-fill");
    expect(attrs.style).toEqual({ "--ac-quota-remaining": "72%" });
    expect(attrs.title).toBe(agentQuotaTitle(28));
    expect(attrs["aria-label"]).toContain("Claude Code");
  });

  it("attrs_with_a_reading_carry_the_meter_range", () => {
    const attrs = quotaChipAttrs("Codex", 0);
    expect(attrs.role).toBe("meter");
    expect(attrs["aria-valuenow"]).toBe(0);
    expect(attrs["aria-valuemin"]).toBe(0);
    expect(attrs["aria-valuemax"]).toBe(100);
  });
});

// #2681 - the "N% left" text next to agent names in the picker and Settings.
describe("quotaRemainingLabel", () => {
  it("gives_remaining_for_a_valid_reading_and_null_otherwise", () => {
    const cases: [number | null | undefined, string | null][] = [
      [28, "72% left"],
      [0, "100% left"],
      [100, "0% left"],
      [null, null],
      [undefined, null],
      [101, null],
      [12.5, null],
    ];
    for (const [value, expected] of cases) expect(quotaRemainingLabel(value)).toBe(expected);
  });
});

// jsdom has no layout, so the one-line name + badge layout is guarded on the
// stylesheet text itself.
describe("#2681 quota name line CSS", () => {
  const css = readFileSync(new URL("../styles/sidebar.css", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const rule = (selector: string): { body: string; start: number } => {
    const start = css.indexOf(`${selector} {`);
    expect(start).toBeGreaterThanOrEqual(0);
    return { body: css.slice(start, css.indexOf("}", start)), start };
  };

  it("keeps_name_and_badge_on_one_line_with_the_name_ellipsised", () => {
    expect(rule(".agent-quota-name-line").body).toContain("display: flex;");
    const name = rule(".agent-quota-name-line > .agent-profile-provider-name,\n.agent-quota-name-line > .agent-quota-name").body;
    expect(name).toContain("min-width: 0;");
    expect(name).toContain("text-overflow: ellipsis;");
    expect(name).toContain("white-space: nowrap;");
    expect(rule(".agent-quota-remaining").body).toContain("flex: 0 0 auto;");
  });

  it("places_the_line_rule_after_the_provider_name_rule", () => {
    const providerName = css.indexOf(".agent-profile-provider-name,\n.agent-profile-card-title {");
    expect(providerName).toBeGreaterThanOrEqual(0);
    expect(rule(".agent-quota-name-line").start).toBeGreaterThan(providerName);
  });
});
