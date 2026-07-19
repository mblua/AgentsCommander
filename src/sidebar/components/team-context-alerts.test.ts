import { describe, expect, it } from "vitest";
import {
  hydrateContextAlertThresholdDrafts,
  validateContextAlertThresholdDrafts,
} from "./team-context-alerts";
import type { ContextAlertThresholdDraft } from "./team-context-alerts";

function drafts(...rawValues: string[]): ContextAlertThresholdDraft[] {
  return rawValues.map((raw, index) => ({ id: index + 1, raw }));
}

describe("team context alert draft validation", () => {
  it("canonicalizes empty, boundary, leading-zero, and unordered drafts without mutation", () => {
    const cases: { input: ContextAlertThresholdDraft[]; expected: number[] }[] = [
      { input: [], expected: [] },
      { input: drafts("1"), expected: [1] },
      { input: drafts("100"), expected: [100] },
      { input: drafts("001"), expected: [1] },
      { input: drafts("90", "50", "75"), expected: [50, 75, 90] },
    ];

    for (const testCase of cases) {
      const before = testCase.input.map((draft) => ({ ...draft }));
      const result = validateContextAlertThresholdDrafts(testCase.input);
      expect(result.valid).toBe(true);
      if (!result.valid) throw new Error("Expected valid draft result");
      expect(result.canonicalPercentages).toEqual(testCase.expected);
      expect(result.canonicalPercentages).not.toBe(testCase.expected);
      expect(result.rows).toHaveLength(testCase.input.length);
      expect(result.summaryMessages).toEqual([]);
      expect(testCase.input).toEqual(before);
    }
  });

  it("accepts surrounding whitespace without rewriting the raw value", () => {
    const input = drafts(" \t50\n ");
    const result = validateContextAlertThresholdDrafts(input);
    expect(result.valid).toBe(true);
    if (!result.valid) throw new Error("Expected valid whitespace result");
    expect(result.canonicalPercentages).toEqual([50]);
    expect(input[0]?.raw).toBe(" \t50\n ");
  });

  it("returns fixed lexical and range annotations for invalid spellings", () => {
    const longDigits = "9".repeat(400);
    const cases: { raw: string; code: string; message: string }[] = [
      { raw: "", code: "blank", message: "Enter a threshold percentage." },
      { raw: "   ", code: "blank", message: "Enter a threshold percentage." },
      {
        raw: "text",
        code: "wholeNumber",
        message: "Enter a whole-number percentage using digits only.",
      },
      {
        raw: "+50",
        code: "wholeNumber",
        message: "Enter a whole-number percentage using digits only.",
      },
      {
        raw: "-50",
        code: "wholeNumber",
        message: "Enter a whole-number percentage using digits only.",
      },
      {
        raw: "5e1",
        code: "wholeNumber",
        message: "Enter a whole-number percentage using digits only.",
      },
      {
        raw: "50.0",
        code: "wholeNumber",
        message: "Enter a whole-number percentage using digits only.",
      },
      {
        raw: "50.5",
        code: "wholeNumber",
        message: "Enter a whole-number percentage using digits only.",
      },
      {
        raw: "0",
        code: "range",
        message: "Threshold must be between 1% and 100%.",
      },
      {
        raw: "101",
        code: "range",
        message: "Threshold must be between 1% and 100%.",
      },
      {
        raw: longDigits,
        code: "range",
        message: "Threshold must be between 1% and 100%.",
      },
    ];

    for (const testCase of cases) {
      const result = validateContextAlertThresholdDrafts(drafts(testCase.raw));
      expect(result.valid).toBe(false);
      expect(result.canonicalPercentages).toBeNull();
      expect(result.rows[0]?.errorCodes).toEqual([testCase.code]);
      expect(result.rows[0]?.messages).toEqual([testCase.message]);
      expect(result.summaryMessages).toEqual([`Threshold 1: ${testCase.message}`]);
    }
  });

  it("marks every numerically equivalent duplicate while leaving unrelated rows clean", () => {
    for (const input of [drafts("80", "080", "50"), drafts("80", "080", "00080")]) {
      const result = validateContextAlertThresholdDrafts(input);
      expect(result.valid).toBe(false);
      expect(result.canonicalPercentages).toBeNull();
      expect(result.rows[0]?.errorCodes).toContain("duplicate");
      expect(result.rows[1]?.errorCodes).toContain("duplicate");
      if (input.length === 3 && input[2]?.raw === "50") {
        expect(result.rows[2]?.errorCodes).toEqual([]);
      } else {
        expect(result.rows[2]?.errorCodes).toContain("duplicate");
      }
      expect(result.summaryMessages.every((message) => message.includes("already configured"))).toBe(true);
    }
  });

  it("preserves every over-cap row and emits the cap summary once before row errors", () => {
    for (const count of [4, 5]) {
      const input = drafts("10", "20", "30", "40", "50").slice(0, count);
      const result = validateContextAlertThresholdDrafts(input);
      expect(result.valid).toBe(false);
      expect(result.canonicalPercentages).toBeNull();
      expect(result.rows).toHaveLength(count);
      expect(result.rows.slice(0, 3).every((row) => !row.errorCodes.includes("cardinality"))).toBe(true);
      expect(result.rows.slice(3).every((row) => row.errorCodes[0] === "cardinality")).toBe(true);
      expect(result.rows.slice(3).every(
        (row) => row.messages[0] === "Remove a threshold so no more than 3 remain.",
      )).toBe(true);
      expect(result.summaryMessages[0]).toBe(
        "A team can have at most 3 context alert thresholds.",
      );
      expect(result.summaryMessages.filter(
        (message) => message === "A team can have at most 3 context alert thresholds.",
      )).toHaveLength(1);
    }
  });

  it("hydrates valid arrays canonically but preserves every invalid numeric row", () => {
    expect(hydrateContextAlertThresholdDrafts([])).toEqual([]);
    expect(hydrateContextAlertThresholdDrafts([90, 50, 75])).toEqual([
      { id: 1, raw: "50" },
      { id: 2, raw: "75" },
      { id: 3, raw: "90" },
    ]);

    for (const values of [
      [80, 80],
      [50.5, 75],
      [101, 25],
      [10, 20, 30, 40],
    ]) {
      expect(hydrateContextAlertThresholdDrafts(values)).toEqual(
        values.map((value, index) => ({ id: index + 1, raw: String(value) })),
      );
    }
  });
});
