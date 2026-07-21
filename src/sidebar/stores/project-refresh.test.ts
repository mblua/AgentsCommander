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

describe("OS-shape aware normalization (#1077)", () => {
  it("preserves case for POSIX-shaped paths so case-only variants stay distinct", () => {
    expect(normalizeProjectPathForCompare("/Repo/A")).toBe("/Repo/A");
    expect(normalizeProjectPathForCompare("/repo/A")).toBe("/repo/A");
    expect(normalizeProjectPathForCompare("/Repo/A")).not.toBe(
      normalizeProjectPathForCompare("/repo/A")
    );
  });

  it("trims redundant trailing slashes but keeps the bare POSIX root", () => {
    expect(normalizeProjectPathForCompare("/home/user/project///")).toBe(
      "/home/user/project"
    );
    expect(normalizeProjectPathForCompare("/")).toBe("/");
  });

  it("preserves a literal backslash inside a POSIX path", () => {
    // A POSIX filename may legally contain a backslash; it must not be folded to
    // a separator or reinterpreted as a Windows alias.
    expect(normalizeProjectPathForCompare("/home/weird\\name")).toBe(
      "/home/weird\\name"
    );
  });

  it("still case-folds Windows drive and UNC aliases to one key", () => {
    expect(normalizeProjectPathForCompare("C:\\Repo\\A")).toBe(
      normalizeProjectPathForCompare("c:/repo/a")
    );
    expect(normalizeProjectPathForCompare("\\\\Server\\Share\\Repo")).toBe(
      "//server/share/repo"
    );
  });

  it("keeps an unsupported device namespace distinct from an ordinary drive", () => {
    // The `\\.\` device namespace must not collapse onto the ordinary `\\?\C:\`
    // / `C:\` keys; its `//./` marker is preserved.
    const device = normalizeProjectPathForCompare("\\\\.\\C:\\Repo");
    expect(device).toBe("//./c:/repo");
    expect(device).not.toBe(normalizeProjectPathForCompare("C:\\Repo"));
    expect(device).not.toBe(normalizeProjectPathForCompare("\\\\?\\C:\\Repo"));
  });
});
