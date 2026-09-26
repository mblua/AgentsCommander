import { describe, expect, it, vi } from "vitest";
import {
  REMOTE_TOKEN_KEY,
  isRemoteToken,
  resolveRemoteToken,
  storeRemoteTokenFromUrl,
} from "./remote-token";

const VALID = "123e4567-e89b-42d3-a456-426614174000";
const OTHER_VALID = "9f1c2b3a-0d4e-4f5a-8b6c-7d8e9fa0b1c2";

const INVALID = [
  "x&y=1",
  "a#b",
  "a/b",
  VALID.toUpperCase(),
  `{${VALID}}`,
  VALID.slice(1),
  `${VALID}0`,
  "",
] as const;

const params = (token: string | null) =>
  new URLSearchParams(token === null ? "" : { [REMOTE_TOKEN_KEY]: token });
const storage = (stored: string | null) => ({ getItem: () => stored });

describe("remote token (#2646)", () => {
  it("accepts only a lowercase hyphenated UUID", () => {
    expect(isRemoteToken(VALID)).toBe(true);
    for (const value of [...INVALID, null, undefined]) {
      expect(isRemoteToken(value)).toBe(false);
    }
  });

  it.each([
    ["a valid URL token wins over the stored one", VALID, OTHER_VALID, VALID],
    ["an invalid URL token yields empty and does not fall back", "x&y=1", OTHER_VALID, ""],
    ["an absent URL token uses a valid stored token", null, OTHER_VALID, OTHER_VALID],
    ["an empty URL token uses a valid stored token, as with ||", "", OTHER_VALID, OTHER_VALID],
    ["an absent URL token with an invalid stored token yields empty", null, "a#b", ""],
    ["nothing anywhere yields empty", null, null, ""],
  ] as const)("resolves the WS token: %s", (_label, fromUrl, stored, expected) => {
    expect(resolveRemoteToken(params(fromUrl), storage(stored))).toBe(expected);
  });

  it.each([VALID, ...INVALID])("stores only a valid URL token (%j)", (token) => {
    const setItem = vi.fn();
    storeRemoteTokenFromUrl(params(token), { setItem });
    if (token === VALID) expect(setItem).toHaveBeenCalledWith(REMOTE_TOKEN_KEY, VALID);
    else expect(setItem).not.toHaveBeenCalled();
  });
});
