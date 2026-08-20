// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  settingsSnapshot,
  waitFor,
} from "../../shared/testing/ui-harness";

// #1453 — the Start button of the Web Remote Access section ignored the boolean
// that start_web_server returns, and that command answers `false` when the bind
// fails. In the exact scenario of the issue the modal therefore reported
// `Running` and offered `Stop Server` for a server that never bound.

describe("SettingsModal web server start (#1453)", () => {
  let cleanupDom: (() => void) | null = null;

  beforeEach(() => {
    cleanupDom = installBrowserDomStubs();
    resetUiStoresForTests();
  });

  afterEach(() => {
    cleanupDom?.();
    cleanupDom = null;
    resetUiStoresForTests();
    document.body.replaceChildren();
  });

  function findByText(root: HTMLElement, text: string): HTMLButtonElement | null {
    return (
      Array.from(root.querySelectorAll<HTMLButtonElement>("button")).find(
        (button) => button.textContent?.trim() === text
      ) ?? null
    );
  }

  it("keeps reporting Start Server when the start attempt fails to bind", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_settings", settingsSnapshot({ webServerEnabled: true }));
    fake.resolve("get_web_server_status", false);
    fake.resolve("get_coding_agent_catalog", []);
    fake.resolve("list_reseedable_agent_commands", []);
    // The bind failed: the command reports it did not start.
    fake.resolve("start_web_server", false);

    const r = renderWithFakeTransport(
      () => <SettingsModal section="general" onClose={() => {}} />,
      fake
    );
    try {
      await waitFor(() => expect(findByText(r.root, "Start Server")).toBeTruthy());

      click(findByText(r.root, "Start Server")!);

      await waitFor(() => {
        expect(fake.callsFor("start_web_server")).toHaveLength(1);
      });
      expect(findByText(r.root, "Start Server")).toBeTruthy();
      expect(findByText(r.root, "Stop Server")).toBeNull();
    } finally {
      r.cleanup();
    }
  });
});
