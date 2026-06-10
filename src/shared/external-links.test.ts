// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  EXTERNAL_LINK_REQUEST_EVENT,
  getConfirmableExternalUrl,
  requestExternalLinkConfirmation,
} from "./external-links";

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

  describe("requestExternalLinkConfirmation", () => {
    afterEach(() => {
      vi.restoreAllMocks();
    });

    it("dispatches a normalized custom event for external URLs", () => {
      const listener = vi.fn();
      document.addEventListener(EXTERNAL_LINK_REQUEST_EVENT, listener);

      expect(requestExternalLinkConfirmation("http://example.com")).toBe(true);

      expect(listener).toHaveBeenCalledTimes(1);
      expect(listener.mock.calls[0][0]).toMatchObject({
        detail: { url: "http://example.com/" },
      });

      document.removeEventListener(EXTERNAL_LINK_REQUEST_EVENT, listener);
    });

    it("does not dispatch for same-origin or unsupported URLs", () => {
      const listener = vi.fn();
      document.addEventListener(EXTERNAL_LINK_REQUEST_EVENT, listener);

      expect(requestExternalLinkConfirmation("/settings")).toBe(false);
      expect(requestExternalLinkConfirmation("mailto:support@example.com")).toBe(false);

      expect(listener).not.toHaveBeenCalled();
      document.removeEventListener(EXTERNAL_LINK_REQUEST_EVENT, listener);
    });
  });
});
