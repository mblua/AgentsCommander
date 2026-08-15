import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #1351 - the coordinator section now carries a REAL header inside
// .coord-quick-access-group. jsdom never applies this stylesheet, so the bytes on
// disk are the only place the visibility gate can be pinned. If the gate slides
// back onto the inner .coord-quick-access strip, the header stays visible in every
// style that hides the strip and each of them grows an orphan header.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");

// #1351 amend — the header's computed style is identical outside .ac-wg-group, but its
// POSITION comes from the container's box. Revision 1 only checked the former, so the
// section shipped 6px to the left of its siblings in the user's own sidebar style. The
// left inset is pinned here, computed from the bytes on disk, for every enabling style.
const ENABLING_STYLES = ["noir-minimal", "arctic-ops", "deep-space", "obsidian-mesh", "neon-circuit"];

function ruleBody(style: string, className: string): string {
  const re = new RegExp(`^\\[data-sidebar-style="${style}"\\] \\.${className} \\{([^}]*)\\}`, "m");
  const match = CSS.match(re);
  if (!match) throw new Error(`missing rule: [data-sidebar-style="${style}"] .${className}`);
  return match[1];
}

function declarations(body: string): Array<[string, string]> {
  return body
    .split(";")
    .map((d) => d.trim())
    .filter((d) => d.includes(":"))
    .map((d) => [
      d.slice(0, d.indexOf(":")).trim(),
      d.slice(d.indexOf(":") + 1).trim().replace(/\s+/g, " "),
    ]);
}

// Left side of a 1-to-4 value margin/padding shorthand.
const leftOf = (parts: string[]): string =>
  parts.length >= 4 ? parts[3] : parts.length >= 2 ? parts[1] : parts[0];

// margin-left + border-left-width + padding-left, in px. Later declarations win,
// mirroring the cascade inside a single rule body.
function leftInset(body: string): number {
  let margin = 0;
  let border = 0;
  let padding = 0;
  for (const [prop, value] of declarations(body)) {
    const parts = value.split(" ");
    if (prop === "margin") margin = Number.parseFloat(leftOf(parts));
    else if (prop === "margin-left") margin = Number.parseFloat(parts[0]);
    else if (prop === "padding") padding = Number.parseFloat(leftOf(parts));
    else if (prop === "padding-left") padding = Number.parseFloat(parts[0]);
    else if (prop === "border" || prop === "border-left") border = Number.parseFloat(parts[0]);
    else if (prop === "border-left-width") border = Number.parseFloat(parts[0]);
  }
  return margin + border + padding;
}

function declValue(body: string, prop: string): string | undefined {
  let found: string | undefined;
  for (const [p, v] of declarations(body)) if (p === prop) found = v;
  return found;
}

describe("coordinator quick-access section CSS", () => {
  it("gates the whole section, header included, on the group wrapper", () => {
    expect(CSS).toMatch(/\.coord-quick-access-group\s*\{\s*display:\s*none;\s*\}/);
    // No rule may target the inner strip as a whole element again.
    expect(CSS).not.toMatch(/\.coord-quick-access\s*\{/);
  });

  it("paints no CSS pseudo-title now that the section has a real header", () => {
    expect(CSS).not.toMatch(/\.coord-quick-access(-group)?::before\s*\{/);
  });

  it("starts on the same column as its sibling sections in every enabling style", () => {
    for (const style of ENABLING_STYLES) {
      const sister = ruleBody(style, "ac-wg-group");
      const group = ruleBody(style, "coord-quick-access-group");
      expect(leftInset(group), `${style}: left inset`).toBe(leftInset(sister));
      expect(declValue(group, "border-top"), `${style}: separator`).toBe(
        declValue(sister, "border-top")
      );
    }
  });
});
