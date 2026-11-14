import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const ROOT = new URL("../../", import.meta.url);
const MAP_PATH = "docs/reference/frontend-state-owners.md";

// Keys are relative to this file; the loaders are never called.
const STORE_PATHS = Object.keys(import.meta.glob(["../*/stores/*.ts", "!../*/stores/*.test.ts"]))
  .map((key) => new URL(key, import.meta.url).href.slice(ROOT.href.length))
  .sort();

const MAP_TEXT = readFileSync(new URL(MAP_PATH, ROOT), "utf8");
const MAPPED_PATHS = [...MAP_TEXT.matchAll(/`(src\/[^`]+\/stores\/[^`]+\.ts)`/g)]
  .map((m) => m[1])
  .sort();

describe("frontend state owners map", () => {
  it("finds the store modules", () => {
    expect(STORE_PATHS.length).toBeGreaterThan(0);
  });

  it("lists every store module", () => {
    const missing = STORE_PATHS.filter((path) => !MAPPED_PATHS.includes(path));
    expect(missing, `add these to ${MAP_PATH}`).toEqual([]);
  });

  it("lists only store modules that exist", () => {
    const stale = MAPPED_PATHS.filter((path) => !STORE_PATHS.includes(path));
    expect(stale, `remove these from ${MAP_PATH}`).toEqual([]);
  });
});
