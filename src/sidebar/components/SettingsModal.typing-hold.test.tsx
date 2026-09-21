// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  settingsSnapshot,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { AppSettings } from "../../shared/types";

// #2337 — the typing-hold window is a backend-validated setting (1..3600 whole
// seconds). The draft keeps the user's text so an invalid entry is refused
// visibly instead of being coerced, and Save is blocked while it is invalid.

const FIELD = '[data-ac-testid="settings.general.typingHoldSeconds"]';
const ERROR = '[data-ac-testid="settings.general.typingHoldSeconds.error"]';
const SAVE = '[data-ac-testid="settings.save"]';

describe("SettingsModal typing hold (#2337)", () => {
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

  function renderModal(overrides: Partial<AppSettings> = {}) {
    const fake = new FakeTransport();
    fake.resolve("get_settings", settingsSnapshot(overrides));
    fake.resolve("get_web_server_status", false);
    fake.resolve("get_coding_agent_catalog", []);
    fake.resolve("list_reseedable_agent_commands", []);
    fake.resolve("save_settings_draft", undefined);
    const rendered = renderWithFakeTransport(
      () => <SettingsModal section="general" onClose={() => {}} />,
      fake,
    );
    return { fake, rendered };
  }

  function field(root: HTMLElement): HTMLInputElement {
    return root.querySelector<HTMLInputElement>(FIELD)!;
  }

  function type(input: HTMLInputElement, value: string) {
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }

  async function waitForField(root: HTMLElement): Promise<HTMLInputElement> {
    await waitFor(() => expect(root.querySelector(FIELD)).toBeTruthy());
    return field(root);
  }

  it("shows the saved value and persists an edited one through save_settings_draft", async () => {
    const { fake, rendered } = renderModal({ typingHoldSeconds: 45 });
    try {
      const input = await waitForField(rendered.root);
      // Wait for the modal's own get_settings, not the seed painted from the store.
      await waitFor(() => expect(input.value).toBe("45"));
      expect(rendered.root.querySelector(ERROR)).toBeNull();

      type(input, "90");
      rendered.root.querySelector<HTMLButtonElement>(SAVE)!.click();

      await waitFor(() => expect(fake.lastCall("save_settings_draft")).toBeTruthy());
      const saved = (fake.lastCall("save_settings_draft")!.args as { draft: AppSettings }).draft;
      expect(saved.typingHoldSeconds).toBe(90);
    } finally {
      rendered.cleanup();
    }
  });

  it("defaults to 30 when the snapshot predates the field", async () => {
    const { rendered } = renderModal();
    try {
      const input = await waitForField(rendered.root);
      await waitFor(() => expect(input.value).toBe("30"));
      expect(rendered.root.querySelector(ERROR)).toBeNull();
      expect(rendered.root.querySelector<HTMLButtonElement>(SAVE)!.disabled).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });

  it("blocks save with an inline error for a blank, fractional or out-of-range draft", async () => {
    const { fake, rendered } = renderModal({ typingHoldSeconds: 120 });
    try {
      const input = await waitForField(rendered.root);
      // Distinct from both the 30 seed/default and the previous test's 45, so
      // reaching it proves the modal's own get_settings has landed.
      await waitFor(() => expect(input.value).toBe("120"));
      for (const invalid of ["", "12.5", "0", "3601"]) {
        type(input, invalid);
        await waitFor(() => expect(rendered.root.querySelector(ERROR)).toBeTruthy());
        expect(
          rendered.root.querySelector<HTMLButtonElement>(SAVE)!.disabled,
          `save must be blocked for ${JSON.stringify(invalid)}`,
        ).toBe(true);
        expect(rendered.root.querySelector(".modal-save-error")?.textContent).toContain(
          "Typing hold",
        );
      }
      // The last valid number is never what a blocked save persists.
      expect(fake.lastCall("save_settings_draft")).toBeFalsy();

      type(input, "60");
      await waitFor(() => expect(rendered.root.querySelector(ERROR)).toBeNull());
      expect(rendered.root.querySelector<HTMLButtonElement>(SAVE)!.disabled).toBe(false);
    } finally {
      rendered.cleanup();
    }
  });
});
