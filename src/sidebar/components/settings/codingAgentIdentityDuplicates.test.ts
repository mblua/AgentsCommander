import { describe, expect, it } from "vitest";
import type { AgentConfig } from "../../../shared/types";
import { sharedIdentityPeers } from "./codingAgentIdentityDuplicates";

function a(label: string, command: string, id_hash?: string): AgentConfig {
  return { id: label, label, command, color: "#000", envs: [], isolatedHome: false, id_hash };
}

describe("sharedIdentityPeers", () => {
  it("lists the other row when 2 of 3 rows share id_hash", () => {
    const rows = [a("Codex 2", "codex", "cid1:aa"), a("just codex", "codex", "cid1:aa"),
      a("Other", "other", "cid1:bb")];
    expect(sharedIdentityPeers(rows, 0)).toEqual(["just codex"]);
    expect(sharedIdentityPeers(rows, 1)).toEqual(["Codex 2"]);
    expect(sharedIdentityPeers(rows, 2)).toEqual([]);
  });

  it("lists two peers in row order when three rows share one hash", () => {
    const rows = [a("A", "x", "h"), a("B", "x", "h"), a("C", "x", "h")];
    expect(sharedIdentityPeers(rows, 0)).toEqual(["B", "C"]);
    expect(sharedIdentityPeers(rows, 1)).toEqual(["A", "C"]);
    expect(sharedIdentityPeers(rows, 2)).toEqual(["A", "B"]);
  });

  it("never matches a missing or empty id_hash", () => {
    const rows = [a("A", "x"), a("B", "x"), a("C", "x", ""), a("D", "x", "")];
    for (let i = 0; i < rows.length; i++) expect(sharedIdentityPeers(rows, i)).toEqual([]);
  });

  it("returns [] for the same command with a different hash", () => {
    const rows = [a("Claude", "claude", "cid1:c1"), a("Claude 2", "claude", "cid1:c2")];
    expect(sharedIdentityPeers(rows, 0)).toEqual([]);
    expect(sharedIdentityPeers(rows, 1)).toEqual([]);
  });
});
