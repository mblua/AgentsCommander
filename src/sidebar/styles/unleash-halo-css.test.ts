import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { declValue, scanSheet } from "./css-test-helpers";

// #4752 P06 (D9) - the breathing red halo on an unleashed replica tile. jsdom
// never applies this sheet, so its bytes are the only place to pin the rules.
const URL_HALO = new URL("./unleash-halo.css", import.meta.url);
const CSS = readFileSync(URL_HALO, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
const RULES = scanSheet(URL_HALO);
const ALL = readFileSync(new URL("./sidebar-all.css", import.meta.url), "utf8");

const BASE = ".replica-item.unleashed";
const ACTIVE = ".replica-item.unleashed.active";
const REDUCED = "@media (prefers-reduced-motion: reduce)";
const squeeze = (v: string): string => v.replace(/\s+/g, "");
const PEAK = squeeze(
  "0 0 22px 8px var(--unleash-red), " +
    "0 0 44px 14px color-mix(in srgb, var(--unleash-red) 40%, transparent)"
);

/** Text of the block opened by `head`, braces balanced. Throws on a miss. */
function block(head: string): string {
  const start = CSS.indexOf(`${head} {`);
  if (start < 0) throw new Error(`missing block: ${head}`);
  let depth = 0;
  for (let i = CSS.indexOf("{", start); i < CSS.length; i++) {
    if (CSS[i] === "{") depth++;
    if (CSS[i] === "}" && --depth === 0) return CSS.slice(start, i + 1);
  }
  throw new Error(`unclosed block: ${head}`);
}

/** The only rule whose selector list is exactly `selectors`. */
function exact(...selectors: string[]): string {
  const hits = RULES.filter((r) => r.selectors.join("|") === selectors.join("|"));
  if (hits.length !== 1) throw new Error(`expected 1 rule for ${selectors.join(", ")}`);
  return hits[0]!.body;
}

describe("#4752 unleash halo CSS bytes", () => {
  it("1. the tile rule sets the red token and the 1.8s infinite breathe", () => {
    const b = exact(BASE);
    expect(declValue(b, "--unleash-red")).toBe("var(--status-exited)");
    expect(declValue(b, "position")).toBe("relative");
    expect(declValue(b, "z-index")).toBe("2");
    expect(declValue(b, "border-radius")).toBe("4px");
    expect(declValue(b, "animation")).toBe("unleash-breathe 1.8s ease-in-out infinite");
  });

  it("2. the selected tile animates only the glow", () => {
    expect(declValue(exact(ACTIVE), "animation-name")).toBe("unleash-breathe-glow");
    const glow = block("@keyframes unleash-breathe-glow");
    expect(glow).not.toContain("background");
    expect(squeeze(glow)).toContain(PEAK);
  });

  it("3. the breathe keyframes peak with the glow and an 18% red background", () => {
    const frames = squeeze(block("@keyframes unleash-breathe"));
    expect(frames).toContain(PEAK);
    expect(frames).toContain("background:color-mix(insrgb,var(--unleash-red)18%,transparent)");
    expect(frames).toContain("0%,100%{box-shadow:0000transparent;background:transparent;}");
  });

  it("4. reduced motion stops both tiles and shows a static glow", () => {
    const media = block(REDUCED);
    expect(media).toContain(`${BASE},`);
    expect(media).toContain(ACTIVE);
    const b = exact(BASE, ACTIVE);
    expect(declValue(b, "animation")).toBe("none");
    expect(squeeze(declValue(b, "box-shadow"))).toBe(
      squeeze("inset 0 0 0 3px var(--unleash-red), 0 0 12px 3px var(--unleash-red)")
    );
  });

  it("5. no .session-item selector and no rule beyond the halo", () => {
    expect(CSS).not.toContain(".session-item");
    const allowed = [BASE, ACTIVE, "0%", "100%", "50%"];
    for (const r of RULES) {
      for (const s of r.selectors) expect(allowed).toContain(s);
    }
  });

  it("6. sidebar-all.css imports the halo sheet last", () => {
    const imports = ALL.split(/\r?\n/).filter((l) => l.trim().startsWith("@import"));
    expect(imports[imports.length - 1]!.trim()).toBe('@import "./unleash-halo.css";');
  });
});
