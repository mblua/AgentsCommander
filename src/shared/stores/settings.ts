import { createSignal } from "solid-js";
import type { AppSettings } from "../types";
import { SettingsAPI } from "../ipc";
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
