// @vitest-environment jsdom
import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import SettingsModal from "./SettingsModal";
import { FakeTransport } from "../../shared/testing/fake-transport";
import {
  click,
  input,
  installBrowserDomStubs,
  renderWithFakeTransport,
  resetUiStoresForTests,
  settingsSnapshot,
  waitFor,
} from "../../shared/testing/ui-harness";
import type { AppSettings } from "../../shared/types";

// #2232 phase 9 tests 1 to 4 and 14 — the Co-managed (Jev) settings group.
//
// The group is a SIBLING of the Voice to Text section, outside its
// <Show when={voiceToTextEnabled}> (plan section 4.1). Every test below therefore
// runs with voiceToTextEnabled false: a group placed inside that Show would not
// render at all, and test 1 fails.

const FIELD = {
  key: "settings.integrations.jevApiKey",
  model: "settings.integrations.jevModel",
  endpoint: "settings.integrations.jevEndpoint",
  timeout: "settings.integrations.jevTimeoutSecs",
  threshold: "settings.integrations.jevThreshold",
  margin: "settings.integrations.jevMargin",
} as const;

/** The compiled defaults, asserted in test 14 and read from Rust in test 2. */
const COMPILED = {
  key: "",
  model: "jev-1.13.0",
  endpoint: "https://api.typesafe.ai/v1/systemone",
  timeout: "20",
  threshold: 0.7,
  margin: 0.15,
} as const;

function byTestId<T extends Element = Element>(root: HTMLElement, testId: string): T | null {
  return root.querySelector<T>(`[data-ac-testid="${testId}"]`);
}

function inputValue(root: HTMLElement, testId: string): string {
  const el = byTestId<HTMLInputElement>(root, testId);
  if (!el) throw new Error(`missing settings field: ${testId}`);
  return el.value;
}

function renderIntegrations(overrides: Partial<AppSettings> = {}) {
  const fake = new FakeTransport();
  fake.resolve("get_settings", settingsSnapshot(overrides));
  fake.resolve("get_web_server_status", false);
  fake.resolve("get_coding_agent_catalog", []);
  fake.resolve("list_reseedable_agent_commands", []);
  fake.resolve("save_settings_draft", undefined);
  const rendered = renderWithFakeTransport(
    () => <SettingsModal section="integrations" onClose={() => {}} />,
    fake
  );
  return { ...rendered, fake };
}

/** `src-tauri/src/config/settings.rs`, the authority for every default below. */
function rustSource(): string {
  // Vite rewrites the literal-string form of new URL(..., import.meta.url) into a
  // served asset URL under jsdom; a variable base keeps the real file: URL that
  // node:fs accepts (AgentUpdateOverlay.test.tsx).
  const moduleUrl = import.meta.url;
  return readFileSync(
    new URL("../../../src-tauri/src/config/settings.rs", moduleUrl),
    "utf8"
  );
}

function rustStringDefault(source: string, fn: string): string {
  const match = new RegExp(`fn ${fn}\\(\\)\\s*->\\s*String\\s*\\{[^"]*"([^"]*)"`).exec(source);
  if (!match) throw new Error(`settings.rs: ${fn} not found`);
  return match[1];
}

function rustNumberDefault(source: string, fn: string): number {
  const match = new RegExp(`fn ${fn}\\(\\)\\s*->\\s*[A-Za-z0-9_]+\\s*\\{\\s*([0-9.]+)\\s*\\}`).exec(
    source
  );
  if (!match) throw new Error(`settings.rs: ${fn} not found`);
  return Number.parseFloat(match[1]);
}

describe("SettingsModal Jev group (#2232 phase 9)", () => {
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

  it("1. renders all six fields outside the Voice to Text Show, round-trips them and saves", async () => {
    const r = renderIntegrations({ voiceToTextEnabled: false });
    try {
      await waitFor(() => expect(byTestId(r.root, FIELD.key)).toBeTruthy());

      // The in-Show control: with voice-to-text off, only the Gemini key's notice
      // disappears; the Jev group must still be there (plan section 4.1).
      expect(byTestId(r.root, "settings.integrations.geminiApiKey.plaintextWarning")).toBeNull();

      const entered: Array<[string, string]> = [
        [FIELD.key, "jev-key-1"],
        [FIELD.model, "jev-1.13.0-rc1"],
        [FIELD.endpoint, "https://example.test/v1/systemone"],
        [FIELD.timeout, "31"],
        [FIELD.threshold, "0.42"],
        [FIELD.margin, "0.05"],
      ];
      for (const [testId, value] of entered) {
        const field = byTestId<HTMLInputElement>(r.root, testId);
        expect(field).toBeTruthy();
        input(field!, value);
      }
      for (const [testId, value] of entered) {
        expect(inputValue(r.root, testId)).toBe(value);
      }

      click(byTestId<HTMLButtonElement>(r.root, "settings.save")!);
      await waitFor(() => expect(r.fake.lastCall("save_settings_draft")).toBeTruthy());
      const draft = r.fake.lastCall("save_settings_draft")!.args.draft as AppSettings;
      expect(draft.jevApiKey).toBe("jev-key-1");
      expect(draft.jevModel).toBe("jev-1.13.0-rc1");
      expect(draft.jevEndpoint).toBe("https://example.test/v1/systemone");
      expect(draft.jevTimeoutSecs).toBe(31);
      expect(draft.jevThreshold).toBeCloseTo(0.42);
      expect(draft.jevMargin).toBeCloseTo(0.05);
    } finally {
      r.cleanup();
    }
  });

  it("2. every form default matches the value compiled in settings.rs", async () => {
    const rust = rustSource();
    const r = renderIntegrations();
    try {
      await waitFor(() => expect(byTestId(r.root, FIELD.key)).toBeTruthy());

      // `pub jev_api_key: String` carries a bare #[serde(default)], whose String
      // default is "": the empty, valid, inert key the form shows.
      expect(inputValue(r.root, FIELD.key)).toBe(COMPILED.key);
      expect(inputValue(r.root, FIELD.model)).toBe(rustStringDefault(rust, "default_jev_model"));
      expect(inputValue(r.root, FIELD.endpoint)).toBe(
        rustStringDefault(rust, "default_jev_endpoint")
      );
      expect(inputValue(r.root, FIELD.timeout)).toBe(
        String(rustNumberDefault(rust, "default_jev_timeout_secs"))
      );
      expect(Number(inputValue(r.root, FIELD.threshold))).toBeCloseTo(
        rustNumberDefault(rust, "default_jev_threshold"),
        6
      );
      expect(Number(inputValue(r.root, FIELD.margin))).toBeCloseTo(
        rustNumberDefault(rust, "default_jev_margin"),
        6
      );
    } finally {
      r.cleanup();
    }
  });

  it("3. an empty Jev API key saves without a validation error and without a prompt", async () => {
    const r = renderIntegrations({ jevApiKey: "" });
    try {
      await waitFor(() => expect(byTestId(r.root, FIELD.key)).toBeTruthy());
      expect(inputValue(r.root, FIELD.key)).toBe("");

      const save = byTestId<HTMLButtonElement>(r.root, "settings.save")!;
      expect(save.disabled).toBe(false);
      click(save);
      await waitFor(() => expect(r.fake.lastCall("save_settings_draft")).toBeTruthy());
      expect(r.root.querySelector(".modal-save-error")).toBeNull();
      const draft = r.fake.lastCall("save_settings_draft")!.args.draft as AppSettings;
      expect(draft.jevApiKey).toBe("");
    } finally {
      r.cleanup();
    }
  });

  it("4. the model field renders the version-bound warning line", async () => {
    const r = renderIntegrations();
    try {
      await waitFor(() => expect(byTestId(r.root, FIELD.model)).toBeTruthy());
      const warning = byTestId(r.root, "settings.integrations.jevModel.warning");
      expect(warning).toBeTruthy();
      expect(warning!.textContent).toContain("jev-1.13.0");
      expect(warning!.textContent).toContain("0.70");
      expect(warning!.textContent).toContain("0.15");
      expect(warning!.textContent).toContain("unmeasured");
    } finally {
      r.cleanup();
    }
  });

  it("14. an absent jev* key is inert: every control shows its compiled default and nothing throws", async () => {
    // `settingsSnapshot()` has none of the six keys: that is the decision section
    // 4.2 pins, and this is its executable form.
    const r = renderIntegrations();
    try {
      await waitFor(() => expect(byTestId(r.root, FIELD.key)).toBeTruthy());
      expect(inputValue(r.root, FIELD.key)).toBe(COMPILED.key);
      expect(inputValue(r.root, FIELD.model)).toBe(COMPILED.model);
      expect(inputValue(r.root, FIELD.endpoint)).toBe(COMPILED.endpoint);
      expect(inputValue(r.root, FIELD.timeout)).toBe(COMPILED.timeout);
      expect(Number(inputValue(r.root, FIELD.threshold))).toBeCloseTo(COMPILED.threshold, 6);
      expect(Number(inputValue(r.root, FIELD.margin))).toBeCloseTo(COMPILED.margin, 6);
      expect(byTestId(r.root, "settings.save")).toBeTruthy();
    } finally {
      r.cleanup();
    }
  });
});
