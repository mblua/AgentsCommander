import { describe, expect, it } from "vitest";
import { getConfirmableExternalUrl } from "./external-links";

const appUrl = "http://tauri.localhost/index.html?window=main";

describe("getConfirmableExternalUrl", () => {
  it("returns normalized external http and https URLs", () => {
    expect(getConfirmableExternalUrl("https://example.com/docs", appUrl)).toBe(
      "https://example.com/docs"
    );
    expect(getConfirmableExternalUrl("http://example.com", appUrl)).toBe(
      "http://example.com/"
    );
  });

  it("does not intercept same-origin app links", () => {
    expect(getConfirmableExternalUrl("/settings", appUrl)).toBeNull();
    expect(
      getConfirmableExternalUrl("http://tauri.localhost/guide", appUrl)
    ).toBeNull();
  });

  it("does not intercept unsupported or malformed URLs", () => {
    expect(getConfirmableExternalUrl("mailto:support@example.com", appUrl)).toBeNull();
    expect(getConfirmableExternalUrl("javascript:alert(1)", appUrl)).toBeNull();
    expect(getConfirmableExternalUrl("https://[", appUrl)).toBeNull();
  });
});
