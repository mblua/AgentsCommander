// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import {
  RAIL_COLOR_DEFAULT,
  RAIL_COLOR_PROPERTY,
  RAIL_WIDTH_DEFAULT,
  RAIL_WIDTH_MAX,
  RAIL_WIDTH_PROPERTY,
  applySelectedRowRail,
  isRailColorDefault,
  isValidRailColor,
  isValidRailWidth,
  railWidthToCss,
} from "./selected-row-rail";

// #1796 P3. One predicate, two consumers. The load-bearing invariant has two
// halves (#2751): a value warns exactly when its validator is false, and a value
// is published exactly when it is valid and, for the colour, is not the factory
// default. The validators and the writer are measured against the same literals.

const VALID_WIDTHS = ["1", "9", "9px", "14", "14px", " 12px "];

// "0", "15", "99" and "999px" were accepted before round 4: they are the
// regression pins for the cap. "09" pins the leading zero, which a range check
// written as Number(v) <= 14 would let through.
const INVALID_WIDTHS = [
  "",
  "   ",
  "0",
  "15",
  "15px",
  "16",
  "16px",
  "99",
  "999px",
  "09",
  "9 px",
  "9pt",
  "-3px",
  "abc",
  "9px;color:red",
];

const VALID_COLORS = ["#00ff5f", "#ABCDEF", " #000000 "];

const INVALID_COLORS = ["", "#fff", "#00ff5f80", "00ff5f", "red", "#00ff5g"];

/** The shape-valid colours of the agreement list, stated as data rather than
 *  derived from the validator under test. */
const COLOR_CASES_VALID = new Set([...VALID_COLORS, "#FFFFFF"]);

/** A fresh element for every case. The writer's production root is
 *  document.documentElement, which is shared across cases in this file, and a
 *  leftover property on it would make an absence assertion pass. S1 and S2
 *  below deliberately override this rule; nothing else does. */
function freshElement(): HTMLElement {
  return document.createElement("div");
}

describe("isValidRailWidth", () => {
  it.each(VALID_WIDTHS)("accepts %j", (value) => {
    expect(isValidRailWidth(value)).toBe(true);
  });

  it.each(INVALID_WIDTHS)("rejects %j", (value) => {
    expect(isValidRailWidth(value)).toBe(false);
  });

  // The cap is pinned to the exported constant, not restated. Changing WIDTH_RE
  // without RAIL_WIDTH_MAX, or the reverse, fails here. P4b imports the same
  // constant, so the accepted range and the CSS arithmetic cannot drift apart.
  it("accepts RAIL_WIDTH_MAX and rejects one above it", () => {
    expect(isValidRailWidth(String(RAIL_WIDTH_MAX))).toBe(true);
    expect(isValidRailWidth(String(RAIL_WIDTH_MAX + 1))).toBe(false);
  });
});

describe("isValidRailColor", () => {
  it.each(VALID_COLORS)("accepts %j", (value) => {
    expect(isValidRailColor(value)).toBe(true);
  });

  it.each(INVALID_COLORS)("rejects %j", (value) => {
    expect(isValidRailColor(value)).toBe(false);
  });
});

describe("railWidthToCss", () => {
  it("adds px to a bare number and leaves an explicit length alone", () => {
    expect(railWidthToCss("12")).toBe("12px");
    expect(railWidthToCss("12px")).toBe("12px");
    expect(railWidthToCss(" 12 ")).toBe("12px");
  });
});

describe("applySelectedRowRail", () => {
  it("publishes both properties when both values are valid", () => {
    const el = freshElement();
    applySelectedRowRail(el, "9px", "#00ff5f");
    expect(el.style.getPropertyValue(RAIL_WIDTH_PROPERTY)).toBe("9px");
    expect(el.style.getPropertyValue(RAIL_COLOR_PROPERTY)).toBe("#00ff5f");
  });

  it("publishes a bare number as a length", () => {
    const el = freshElement();
    applySelectedRowRail(el, "12", "#00ff5f");
    expect(el.style.getPropertyValue(RAIL_WIDTH_PROPERTY)).toBe("12px");
    expect(el.style.getPropertyValue(RAIL_COLOR_PROPERTY)).toBe("#00ff5f");
  });

  it("leaves the width absent and the colour present when only the width is invalid", () => {
    const el = freshElement();
    applySelectedRowRail(el, "abc", "#00ff5f");
    expect(el.style.getPropertyValue(RAIL_WIDTH_PROPERTY)).toBe("");
    expect(el.style.getPropertyValue(RAIL_COLOR_PROPERTY)).toBe("#00ff5f");
  });

  it("leaves the colour absent and the width present when only the colour is invalid", () => {
    const el = freshElement();
    applySelectedRowRail(el, "9px", "nope");
    expect(el.style.getPropertyValue(RAIL_WIDTH_PROPERTY)).toBe("9px");
    expect(el.style.getPropertyValue(RAIL_COLOR_PROPERTY)).toBe("");
  });

  it("leaves both absent when both values are invalid", () => {
    const el = freshElement();
    applySelectedRowRail(el, "abc", "nope");
    expect(el.style.getPropertyValue(RAIL_WIDTH_PROPERTY)).toBe("");
    expect(el.style.getPropertyValue(RAIL_COLOR_PROPERTY)).toBe("");
  });
});

// The stale-value controls. Each uses ONE element across two consecutive calls,
// and that is the entire control: an element fresh for the second call cannot
// hold a stale value, so a fresh-element version of these two cases passes under
// a writer whose invalid branch skips instead of removing. They deliberately
// override the fresh-element rule above.
describe("stale-value controls", () => {
  it("S1 - an invalid width removes a previously published width", () => {
    const el = freshElement();
    applySelectedRowRail(el, "9px", "#00ff5f");
    applySelectedRowRail(el, "abc", "#00ff5f");
    expect(el.style.getPropertyValue(RAIL_WIDTH_PROPERTY)).toBe("");
  });

  // S2 is new in round 4 and is not a duplicate of S1. Round 3 had the width
  // case only and every invalid-colour case built a fresh element, so a writer
  // whose colour branch skipped instead of removing left a stale colour on
  // screen with the suite green.
  it("S2 - an invalid colour removes a previously published colour", () => {
    const el = freshElement();
    applySelectedRowRail(el, "9px", "#00ff5f");
    applySelectedRowRail(el, "9px", "nope");
    expect(el.style.getPropertyValue(RAIL_COLOR_PROPERTY)).toBe("");
  });
});

// The agreement control - the reason this module exists. A future edit that
// loosens the hint without loosening the writer, or the reverse, fails here.
describe("agreement between the hint predicate and the writer", () => {
  it.each([...VALID_WIDTHS, ...INVALID_WIDTHS])(
    "width %j warns exactly when it is not published",
    (value) => {
      const el = freshElement();
      applySelectedRowRail(el, value, RAIL_COLOR_DEFAULT);
      const published = el.style.getPropertyValue(RAIL_WIDTH_PROPERTY) !== "";
      expect(published).toBe(isValidRailWidth(value));
    }
  );

  const COLOR_CASES = [...VALID_COLORS, ...INVALID_COLORS, "#FFFFFF"];

  it.each(COLOR_CASES)("colour %j warns exactly when isValidRailColor is false", (value) => {
    const warns = !COLOR_CASES_VALID.has(value);
    expect(warns).toBe(!isValidRailColor(value));
  });

  it.each(COLOR_CASES)(
    "colour %j is published exactly when it is valid and not the default",
    (value) => {
      const el = freshElement();
      applySelectedRowRail(el, RAIL_WIDTH_DEFAULT, value);
      const published = el.style.getPropertyValue(RAIL_COLOR_PROPERTY) !== "";
      expect(published).toBe(isValidRailColor(value) && !isRailColorDefault(value));
    }
  );
});

// #2751 - the factory default means "follow the theme", so it is never published
// inline: the :root and html.light-theme declarations in variables.css decide.
describe("the factory default colour follows the theme", () => {
  it.each(["#FFFFFF", "#ffffff", " #FFFFFF "])("does not publish %j", (value) => {
    const el = freshElement();
    applySelectedRowRail(el, "9px", value);
    expect(el.style.getPropertyValue(RAIL_COLOR_PROPERTY)).toBe("");
    expect(el.style.getPropertyValue(RAIL_WIDTH_PROPERTY)).toBe("9px");
  });

  // Stale control S3: ONE element, so a default branch that skips instead of
  // removing leaves the green on screen and fails here.
  it("S3 - the default removes a previously published colour", () => {
    const el = freshElement();
    applySelectedRowRail(el, "9px", "#00ff5f");
    applySelectedRowRail(el, "9px", "#FFFFFF");
    expect(el.style.getPropertyValue(RAIL_COLOR_PROPERTY)).toBe("");
  });

  it.each(["#FFFFFE", "#FEFFFF"])("still publishes the near-default %j", (value) => {
    const el = freshElement();
    applySelectedRowRail(el, "9px", value);
    expect(el.style.getPropertyValue(RAIL_COLOR_PROPERTY)).toBe(value);
  });

  it("the default does not warn", () => {
    expect(isValidRailColor("#FFFFFF")).toBe(true);
  });
});

// Cross-file pins. jsdom never loads these files, so the bytes on disk are read.
// Vite rewrites the literal `new URL(..., import.meta.url)` form into a served
// asset; a binding keeps the file: URL that node:fs accepts.
const moduleUrl = import.meta.url;

describe("cross-file pins", () => {
  it("the TS default still matches the Rust default", () => {
    const rust = readFileSync(
      new URL("../../src-tauri/src/config/settings.rs", moduleUrl),
      "utf8"
    );
    const body = rust.match(/fn default_selected_row_rail_color\(\)[^{]*\{([^}]*)\}/);
    expect(body, "default_selected_row_rail_color not found").not.toBeNull();
    expect(body![1]).toContain(`"${RAIL_COLOR_DEFAULT}"`);
  });

  it("variables.css declares the default in :root and #1A1A2E in html.light-theme", () => {
    const css = readFileSync(new URL("./styles/variables.css", moduleUrl), "utf8").replace(
      /\/\*[\s\S]*?\*\//g,
      ""
    );
    const decl = (selector: string): string[] => {
      const blocks = [...css.matchAll(/([^{}]+)\{([^{}]*)\}/g)].filter(
        (m) => m[1].trim() === selector
      );
      return blocks.flatMap((m) =>
        [...m[2].matchAll(/--ac-selected-rail-color\s*:\s*([^;]+);/g)].map((d) => d[1].trim())
      );
    };
    expect(decl(":root")).toEqual([RAIL_COLOR_DEFAULT]);
    expect(decl("html.light-theme")).toEqual(["#1A1A2E"]);
  });
});
