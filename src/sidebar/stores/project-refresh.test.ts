import { describe, expect, it } from "vitest";
import {
  findLoadedProjectPathForRefresh,
  normalizeProjectPathForCompare,
} from "./project-refresh";

describe("project refresh path matching", () => {
  it("normalizes slashes case and trailing separators", () => {
    expect(normalizeProjectPathForCompare("C:\\Users\\Maria\\Project\\")).toBe(
      "c:/users/maria/project"
    );
    expect(normalizeProjectPathForCompare("C:/Users/Maria/Project///")).toBe(
      "c:/users/maria/project"
    );
  });

  it("normalizes Windows extended-length path prefixes", () => {
    expect(
      normalizeProjectPathForCompare("\\\\?\\C:\\Users\\Maria\\Project\\")
    ).toBe("c:/users/maria/project");
    expect(
      normalizeProjectPathForCompare("\\\\?\\UNC\\Server\\Share\\Project\\")
    ).toBe("//server/share/project");
  });

  it("returns the loaded path for an equivalent incoming path", () => {
    const loadedPath = "C:\\Users\\Maria\\Project";

    expect(
      findLoadedProjectPathForRefresh(
        [{ path: loadedPath }],
        "c:/users/maria/project/"
      )
    ).toBe(loadedPath);
  });

  it("matches a loaded normal Windows path with an incoming extended path", () => {
    const loadedPath = "C:\\Users\\Maria\\Project";

    expect(
      findLoadedProjectPathForRefresh(
        [{ path: loadedPath }],
        "\\\\?\\C:\\Users\\Maria\\Project"
      )
    ).toBe(loadedPath);
  });

  it("returns null for an unknown project path", () => {
    expect(
      findLoadedProjectPathForRefresh(
        [{ path: "C:\\Users\\Maria\\Project" }],
        "C:\\Users\\Maria\\Other"
      )
    ).toBeNull();
  });
});
