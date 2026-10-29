import { afterEach, describe, expect, it } from "vitest";
import { __resetWebBrowserStoreForTests, webBrowserStore } from "./webBrowser";

const page = (url: string) => ({ url, title: "", loading: false, visible: true });

describe("webBrowserStore error (#3367)", () => {
  afterEach(() => __resetWebBrowserStoreForTests());

  it("keeps the error through show/hide snapshots of the same page", () => {
    webBrowserStore.apply(page("https://www.iana.org/"));
    webBrowserStore.fail("unsupported url: javascript:alert(1)");
    webBrowserStore.apply({ ...page("https://www.iana.org/"), visible: false });
    webBrowserStore.apply(page("https://www.iana.org/"));
    expect(webBrowserStore.error).toBe("unsupported url: javascript:alert(1)");
  });

  it("clears the error when a new page loads or on request", () => {
    webBrowserStore.fail("unsupported url: x");
    webBrowserStore.apply(page("https://example.com/"));
    expect(webBrowserStore.error).toBeNull();
    webBrowserStore.fail("unsupported url: x");
    webBrowserStore.clearError();
    expect(webBrowserStore.error).toBeNull();
  });
});
