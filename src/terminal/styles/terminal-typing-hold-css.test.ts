import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #2379 - jsdom never applies terminal.css, so the bytes on disk are the only
// place the padlock state colors can be pinned. The padlock is a monochrome
// SVG, so `color` paints it: dimmed status-bar foreground while inactive,
// accent while the hold is closed.
const CSS = readFileSync(new URL("./terminal.css", import.meta.url), "utf8");

function ruleBody(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = CSS.match(new RegExp(`^${escaped} \\{([^}]*)\\}`, "m"));
  if (!match) throw new Error(`missing rule: ${selector}`);
  return match[1];
}

function declValue(body: string, prop: string): string {
  let found: string | undefined;
  for (const declaration of body.split(";")) {
    const trimmed = declaration.trim();
    const separator = trimmed.indexOf(":");
    if (separator === -1) continue;
    if (trimmed.slice(0, separator).trim() === prop) {
      found = trimmed.slice(separator + 1).trim().replace(/\s+/g, " ");
    }
  }
  if (found === undefined) throw new Error(`missing declaration: ${prop}`);
  return found;
}

describe("typing-hold padlock CSS (#2379)", () => {
  it("dims the open padlock with the status-bar foreground", () => {
    const body = ruleBody(".status-bar-btn-typing-hold .status-bar-typing-hold-icon");
    expect(declValue(body, "color")).toBe("var(--statusbar-fg)");
    expect(declValue(body, "opacity")).toBe("0.55");
    expect(declValue(body, "width")).toBe("12px");
    expect(declValue(body, "height")).toBe("12px");
  });

  it("paints the closed padlock with the status-bar accent", () => {
    const body = ruleBody(".status-bar-btn-typing-hold.closed .status-bar-typing-hold-icon");
    expect(declValue(body, "color")).toBe("var(--statusbar-accent)");
    expect(declValue(body, "opacity")).toBe("1");
  });
});
