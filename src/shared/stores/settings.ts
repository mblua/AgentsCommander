import { createSignal } from "solid-js";
import type { AppSettings } from "../types";
import { SettingsAPI, onCodingAgentSettingsUpdated } from "../ipc";
import { setSoundsEnabled } from "../sound";

const [settings, setSettings] = createSignal<AppSettings | null>(null);

export const settingsStore = {
  get current() {
    return settings();
  },

  get voiceEnabled() {
    const s = settings();
    return !!s?.voiceToTextEnabled && !!s?.geminiApiKey;
  },

  async load() {
    const s = await SettingsAPI.get();
    setSettings(s);
    // Push the global mute state into sound.ts so every play* call sees the
    // current value (#158). Default true is intentional — old settings.json
    // files predate `soundsEnabled` and must remain audible until the user
    // explicitly mutes.
    setSoundsEnabled(s.soundsEnabled ?? true);
  },

  refresh() {
    settingsStore.load().catch((err) => {
      console.error("settingsStore.refresh() failed:", err);
    });
  },
};

// #786: a `coding-agent` CLI mutation applied through the running GUI's
// MailboxPoller emits `coding_agent_settings_updated`. Re-fetch so long-lived
// consumers of this app-lifetime store observe the change. Fire-and-refetch,
// mirroring `onCodingAgentProfilesUpdated`. Surfaces that already fetch on open
// (AgentPickerModal, SettingsModal draft) are unaffected.
export function __subscribeCodingAgentSettingsUpdates() {
  return onCodingAgentSettingsUpdated(() => {
    settingsStore.refresh();
  });
}

// Subscribe once at module init in the real app, and never unlisten (the store
// lives for the whole app). Skipped under vitest so importing the store does not
// force the default transport to construct in unit tests that have not installed
// a fake transport yet; the dedicated test calls the exported fn explicitly.
if (import.meta.env.MODE !== "test") {
  void __subscribeCodingAgentSettingsUpdates();
}
