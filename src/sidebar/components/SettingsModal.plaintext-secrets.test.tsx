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

// #1347 — the Gemini API Key and each Telegram Bot Token carry a red notice
// saying the value is persisted unencrypted, naming the instance settings.json.
// The notice is informational only: it never blocks saving.

describe("SettingsModal plaintext-secret notices (#1347)", () => {
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

  function byTestId<T extends Element = Element>(root: HTMLElement, testId: string): T | null {
    return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
  }

  function renderIntegrations(settingsFilePath: string | null) {
    const fake = new FakeTransport();
    fake.resolve("get_settings", {
      ...settingsSnapshot({
        voiceToTextEnabled: true,
        telegramBots: [
          { id: "bot-1", label: "First", token: "111:AAA", chatId: 11, color: "#ff0000" },
          { id: "bot-2", label: "Second", token: "222:BBB", chatId: 22, color: "#00ff00" },
        ],
      }),
      settingsFilePath,
    });
    fake.resolve("get_web_server_status", false);
    fake.resolve("get_coding_agent_catalog", []);
    fake.resolve("list_reseedable_agent_commands", []);
    return renderWithFakeTransport(
      () => <SettingsModal section="integrations" onClose={() => {}} />,
      fake
    );
  }

  it("names the resolved settings.json under the Gemini key and every bot token", async () => {
    const path = "D:\\ac\\.agentscommander_ac2\\settings.json";
    const r = renderIntegrations(path);
    try {
      await waitFor(() =>
        expect(byTestId(r.root, "settings.integrations.geminiApiKey.plaintextWarning")).toBeTruthy()
      );

      const gemini = byTestId(r.root, "settings.integrations.geminiApiKey.plaintextWarning")!;
      expect(gemini.textContent).toContain("Stored unencrypted in");
      expect(gemini.textContent).toContain(path);
      expect(gemini.className).toContain("settings-hint");
      expect(gemini.className).toContain("settings-hint-error");

      // One notice per bot, so the warning sits where each token is pasted.
      for (const i of [0, 1]) {
        const bot = byTestId(r.root, `settings.integrations.telegramBots.${i}.plaintextWarning`);
        expect(bot).toBeTruthy();
        expect(bot!.textContent).toContain(path);
      }

      // Informational only: save stays enabled.
      const save = byTestId<HTMLButtonElement>(r.root, "settings.save")!;
      expect(save.disabled).toBe(false);
    } finally {
      r.cleanup();
    }
  });

  it("degrades to a path-less notice when the backend cannot resolve its config dir", async () => {
    const r = renderIntegrations(null);
    try {
      await waitFor(() =>
        expect(byTestId(r.root, "settings.integrations.geminiApiKey.plaintextWarning")).toBeTruthy()
      );

      const gemini = byTestId(r.root, "settings.integrations.geminiApiKey.plaintextWarning")!;
      expect(gemini.textContent).toContain("Stored unencrypted in");
      expect(gemini.textContent).toContain("settings.json");
      expect(gemini.textContent).not.toContain("null");

      for (const i of [0, 1]) {
        const bot = byTestId(r.root, `settings.integrations.telegramBots.${i}.plaintextWarning`);
        expect(bot).toBeTruthy();
        expect(bot!.textContent).toBe(gemini.textContent);
      }
    } finally {
      r.cleanup();
    }
  });
});
