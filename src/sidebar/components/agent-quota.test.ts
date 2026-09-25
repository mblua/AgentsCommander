import { describe, expect, it } from "vitest";
import { AGENT_QUOTA_TOOLTIP_PREFIX, agentQuotaTitle, quotaChipAttrs, quotaFill } from "./agent-quota";

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
