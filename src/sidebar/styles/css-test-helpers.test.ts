import { describe, expect, it } from "vitest";
import { scanRules } from "./css-test-helpers";

describe("scanRules", () => {
  it.each(["", "no braces", "} lone { broken", " \t{}"])(
    "throws when no rule has a selector in %j",
    (css) => {
      expect(() => scanRules(css)).toThrow(/^scanRules found no rules at all$/);
    },
  );

  it.each([
    {
      label: "nested and adjacent rules",
      css: "@media x {\n  .a, .b { x: y; }\r\n .c{} }",
      expected: [
        { selectors: [".a", ".b"], body: " x: y; ", index: 13 },
        { selectors: [".c"], body: "", index: 32 },
      ],
    },
    {
      label: "stray and unmatched braces",
      css: "}.first{x} .second{ y } orphan{",
      expected: [
        { selectors: [".first"], body: "x", index: 1 },
        { selectors: [".second"], body: " y ", index: 11 },
      ],
    },
    {
      label: "blank selectors and empty comma entries",
      css: "\u00a0{}  , ,.one, .two,, {\r\n value }\r\n",
      expected: [{ selectors: [".one", ".two"], body: "\r\n value ", index: 5 }],
    },
    {
      label: "UTF-16 offsets and CRLF",
      css: "}😀\r\n  .x{ raw }\r\n.y{}",
      expected: [
        { selectors: ["😀\r\n  .x"], body: " raw ", index: 1 },
        { selectors: [".y"], body: "", index: 18 },
      ],
    },
    {
      label: "nested rules inside unmatched openers",
      css: "discard { nested {deep} unmatched { outer { inner {}",
      expected: [
        { selectors: ["nested"], body: "deep", index: 10 },
        { selectors: ["inner"], body: "", index: 44 },
      ],
    },
    {
      label: "literal opening braces inside a body",
      css: "a { content: '{{'; } .end{ok}",
      expected: [{ selectors: [".end"], body: "ok", index: 21 }],
    },
  ])("preserves complete rules for $label", ({ css, expected }) => {
    expect(scanRules(css)).toEqual(expected);
  });
});
