import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// #1351 - the coordinator section now carries a REAL header inside
// .coord-quick-access-group. jsdom never applies this stylesheet, so the bytes on
// disk are the only place the visibility gate can be pinned. If the gate slides
// back onto the inner .coord-quick-access strip, the header stays visible in every
// style that hides the strip and each of them grows an orphan header.
const CSS = readFileSync(new URL("./sidebar.css", import.meta.url), "utf8");

describe("coordinator quick-access section CSS", () => {
  it("gates the whole section, header included, on the group wrapper", () => {
    expect(CSS).toMatch(/\.coord-quick-access-group\s*\{\s*display:\s*none;\s*\}/);
    // No rule may target the inner strip as a whole element again.
    expect(CSS).not.toMatch(/\.coord-quick-access\s*\{/);
  });

  it("paints no CSS pseudo-title now that the section has a real header", () => {
    expect(CSS).not.toMatch(/\.coord-quick-access(-group)?::before\s*\{/);
  });
});
