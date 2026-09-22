import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #2236 phase 6 — the compact toolbar rule. jsdom attaches no stylesheet and
// performs no layout, so the bytes on disk are the only place the declarations
// and the arithmetic that lets two 30px buttons fit the 68px rail can be
// pinned. Both sheets are CRLF on disk: every scan splits on /\r?\n/, never a
// bare \n, or a line-equality pin reads a trailing \r and goes vacuous.
const CSS_SOURCE = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");
const VARS_SOURCE = readFileSync(new URL("./variables.css", import.meta.url), "utf8");

/** Blanks block-comment spans line by line, keeping the line count stable. */
function stripComments(lines: string[]): string[] {
  let inComment = false;
  return lines.map((line) => {
    let out = "";
    let index = 0;
    while (index < line.length) {
      if (inComment) {
        const end = line.indexOf("*/", index);
        if (end === -1) break;
        index = end + 2;
        inComment = false;
      } else {
        const start = line.indexOf("/*", index);
        if (start === -1) {
          out += line.slice(index);
          break;
        }
        out += line.slice(index, start);
        index = start + 2;
        inComment = true;
      }
    }
    return out;
  });
}

const CSS_LINES = stripComments(CSS_SOURCE.split(/\r?\n/));
const VARS_LINES = stripComments(VARS_SOURCE.split(/\r?\n/));

/** Body lines of the first rule whose header line is exactly `<selector> {`. */
function ruleBodyLines(lines: string[], selector: string): string[] {
  const start = lines.findIndex((line) => line.trim() === `${selector} {`);
  if (start === -1) throw new Error(`missing rule: ${selector}`);
  const body: string[] = [];
  for (let index = start + 1; index < lines.length; index += 1) {
    const close = lines[index].indexOf("}");
    if (close !== -1) {
      body.push(lines[index].slice(0, close));
      return body;
    }
    body.push(lines[index]);
  }
  throw new Error(`unterminated rule: ${selector}`);
}

/** The declarations of one rule body, last declaration winning. */
function declarations(lines: string[]): Map<string, string> {
  const result = new Map<string, string>();
  for (const line of lines) {
    const match = line.match(/^\s*([a-zA-Z-]+)\s*:\s*(.+);\s*$/);
    if (match) result.set(match[1], match[2].trim().replace(/\s+/g, " "));
  }
  return result;
}

/** One declaration, throwing on a miss so no pin can pass on an empty read. */
function declaration(lines: string[], prop: string, context: string): string {
  const value = declarations(lines).get(prop);
  if (value === undefined) throw new Error(`missing ${prop} in ${context}`);
  return value;
}

function px(value: string, context: string): number {
  const match = value.match(/^(\d+(?:\.\d+)?)px$/);
  if (!match) throw new Error(`not a px value for ${context}: ${value}`);
  return Number(match[1]);
}

const COMPACT_ACTION_BAR = ".sidebar-compact .action-bar";

describe("#2236 phase 6 compact toolbar CSS bytes", () => {
  it("declares the compact inline padding and gap and no block padding or shorthand", () => {
    const declared = declarations(ruleBodyLines(CSS_LINES, COMPACT_ACTION_BAR));

    expect(declared.get("padding-inline")).toBe("2px");
    expect(declared.get("gap")).toBe("2px");
    // The absence leg is load-bearing: a shorthand would cut 8px off the
    // toolbar height in compact, which no jsdom test can see.
    expect(declared.has("padding")).toBe(false);
    expect(declared.has("padding-block")).toBe(false);
    expect(declared.has("padding-top")).toBe(false);
    expect(declared.has("padding-bottom")).toBe(false);
  });

  it("fits two 30px buttons in the rail width, computed from the declared values", () => {
    const compactActionBar = ruleBodyLines(CSS_LINES, COMPACT_ACTION_BAR);
    const actionBarGap = px(
      declaration(compactActionBar, "gap", COMPACT_ACTION_BAR),
      "compact .action-bar gap",
    );
    const inlinePadding = px(
      declaration(compactActionBar, "padding-inline", COMPACT_ACTION_BAR),
      "compact .action-bar padding-inline",
    );
    const iconsGap = px(
      declaration(ruleBodyLines(CSS_LINES, ".action-bar-icons"), "gap", ".action-bar-icons"),
      ".action-bar-icons gap",
    );
    const buttonWidth = px(
      declaration(ruleBodyLines(CSS_LINES, ".toolbar-gear-btn"), "width", ".toolbar-gear-btn"),
      ".toolbar-gear-btn width",
    );
    const railWidth = px(
      declaration(ruleBodyLines(VARS_LINES, ":root"), "--ac-rail-width", ":root"),
      "--ac-rail-width",
    );

    // The live gap is .action-bar-icons's; the rule's own gap separates a
    // single child in compact and is inert, but pinned so the two cannot drift
    // apart unnoticed. A literal total would not fail when a button widens.
    expect(actionBarGap).toBe(2);
    expect(iconsGap).toBe(2);
    expect(2 * inlinePadding + 2 * buttonWidth + iconsGap).toBeLessThanOrEqual(railWidth);
  });
});
