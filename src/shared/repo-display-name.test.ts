import { describe, expect, it } from "vitest";
import { repoDisplayName } from "./repo-display-name";

describe("repoDisplayName", () => {
  it.each([
    ["https://github.com/o/r.git", "r"],
    ["https://github.com/o/r", "r"],
    ["git@github.com:o/r.git", "r"],
    ["https://host/o/r.GIT", "r.GIT"],
    ["https://host/o/r/", "https://host/o/r/"],
    ["noslash", "noslash"],
    ["", ""],
  ])("%s -> %s", (url, expected) => {
    expect(repoDisplayName(url)).toBe(expected);
  });
});
