import { describe, expect, it } from "vitest";
import { githubBranchUrl, githubRepoUrl, parseGithubRemote } from "./github-url";

const ref = { owner: "o", repo: "r" };

describe("parseGithubRemote", () => {
  it("parses https remotes, with or without .git and trailing slash", () => {
    expect(parseGithubRemote("https://github.com/mblua/AgentsCommander")).toEqual({
      owner: "mblua",
      repo: "AgentsCommander",
    });
    expect(parseGithubRemote("https://github.com/mblua/AgentsCommander.git")).toEqual({
      owner: "mblua",
      repo: "AgentsCommander",
    });
    expect(parseGithubRemote("https://github.com/mblua/AgentsCommander/")).toEqual({
      owner: "mblua",
      repo: "AgentsCommander",
    });
  });

  it("accepts http, www, and an uppercase host", () => {
    expect(parseGithubRemote("http://github.com/o/r")).toEqual(ref);
    expect(parseGithubRemote("https://www.github.com/o/r")).toEqual(ref);
    expect(parseGithubRemote("https://GITHUB.com/o/r")).toEqual(ref);
  });

  it("parses scp-like remotes", () => {
    expect(parseGithubRemote("git@github.com:mblua/AgentsCommander.git")).toEqual({
      owner: "mblua",
      repo: "AgentsCommander",
    });
    expect(parseGithubRemote("git@github.com:o/r")).toEqual(ref);
    // The scp branch has no WHATWG normalization, so host lowercasing lives in
    // refFromHostAndPath. Pin it.
    expect(parseGithubRemote("git@GITHUB.com:o/r.git")).toEqual(ref);
  });

  it("parses ssh and git schemes", () => {
    expect(parseGithubRemote("ssh://git@github.com/o/r.git")).toEqual(ref);
    expect(parseGithubRemote("git://github.com/o/r.git")).toEqual(ref);
  });

  it("drops the port and the userinfo (defense in depth: Rust already stripped it)", () => {
    expect(parseGithubRemote("https://github.com:443/o/r")).toEqual(ref);
    expect(parseGithubRemote("https://user:token@github.com/o/r.git")).toEqual(ref);
  });

  it("strips .git case-insensitively (N2)", () => {
    expect(parseGithubRemote("https://github.com/o/r.GIT")).toEqual(ref);
    expect(parseGithubRemote("git@github.com:o/r.GIT")).toEqual(ref);
  });

  it("rejects non-github hosts", () => {
    expect(parseGithubRemote("https://gitlab.com/o/r.git")).toBeNull();
    expect(parseGithubRemote("https://github.example.com/o/r.git")).toBeNull();
    expect(parseGithubRemote("git@bitbucket.org:o/r.git")).toBeNull();
  });

  it("rejects local path remotes", () => {
    // SCP_LIKE_RE matches `C:\repos\mirror` with host "C", which is not github.
    expect(parseGithubRemote("C:\\repos\\mirror")).toBeNull();
    expect(parseGithubRemote("/srv/mirrors/r.git")).toBeNull();
  });

  it("rejects anything that is not exactly owner/repo", () => {
    expect(parseGithubRemote("https://github.com/o")).toBeNull();
    expect(parseGithubRemote("https://github.com/o/r/tree/x")).toBeNull();
  });

  it("rejects empty and nullish input", () => {
    expect(parseGithubRemote("")).toBeNull();
    expect(parseGithubRemote("   ")).toBeNull();
    expect(parseGithubRemote(null)).toBeNull();
    expect(parseGithubRemote(undefined)).toBeNull();
  });

  it("rejects traversal segments on BOTH parser branches (S7)", () => {
    // URL branch: WHATWG normalizes `/o/../r` to `/r`, so it fails the
    // two-segment check. (NAME_RE would NOT have caught it: `.` is in its class.)
    expect(parseGithubRemote("https://github.com/o/../r")).toBeNull();
    // scp branch: no normalization at all, so isSafeSegment is what rejects it.
    // Rev 1 parsed this to {owner: "..", repo: "evil"}.
    expect(parseGithubRemote("git@github.com:../evil")).toBeNull();
    expect(parseGithubRemote("git@github.com:./.")).toBeNull();
  });
});

describe("githubRepoUrl", () => {
  it("builds the repo URL", () => {
    expect(githubRepoUrl(ref)).toBe("https://github.com/o/r");
  });
});

describe("githubBranchUrl", () => {
  it("keeps slashes literal and encodes each segment", () => {
    expect(githubBranchUrl(ref, "fix/909-agency-agents-roles-yaml-skip")).toBe(
      "https://github.com/o/r/tree/fix/909-agency-agents-roles-yaml-skip"
    );
    expect(githubBranchUrl(ref, "feature/a b")).toBe("https://github.com/o/r/tree/feature/a%20b");
    expect(githubBranchUrl(ref, "wip/#1")).toBe("https://github.com/o/r/tree/wip/%231");
  });

  it("handles a single-segment branch", () => {
    expect(githubBranchUrl(ref, "main")).toBe("https://github.com/o/r/tree/main");
  });

  it("rejects traversal and empty refs instead of dropping segments (S8)", () => {
    // Dropping the segments would have produced .../tree/evil/repo — a URL the
    // browser normalizes into a DIFFERENT repository on the same host.
    expect(githubBranchUrl(ref, "../../../../evil/repo")).toBeNull();
    expect(githubBranchUrl(ref, "..")).toBeNull();
    expect(githubBranchUrl(ref, ".")).toBeNull();
    expect(githubBranchUrl(ref, "/")).toBeNull();
    expect(githubBranchUrl(ref, "")).toBeNull();
  });
});
