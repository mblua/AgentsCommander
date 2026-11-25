import { createSignal } from "solid-js";
import type { AppSettings } from "../../shared/types";
import { SettingsAPI } from "../../shared/ipc";
import { settingsStore } from "../../shared/stores/settings";
import { readTutorialFlags, type TutorialSettings } from "../../shared/tutorial-settings";

// #5228 - saved tutorial flags (hint dismissed, team tutorial completed).
const [loaded, setLoaded] = createSignal(false);
const [hintDismissed, setHintDismissed] = createSignal(false);
const [completed, setCompleted] = createSignal(false);
let loading: Promise<void> | null = null;

async function readFlags(): Promise<void> {
  try {
    const flags = readTutorialFlags(await SettingsAPI.get());
    setHintDismissed(flags.tutorialHintDismissed);
    setCompleted(flags.teamTutorialCompleted);
  } catch (e) {
    console.warn("[tutorial] could not read tutorial flags", e);
  }
  setLoaded(true);
}

async function write(patch: TutorialSettings): Promise<void> {
  try {
    const s = await SettingsAPI.get();
    await SettingsAPI.update({ ...s, ...patch } as AppSettings);
    settingsStore.refresh();
  } catch (e) {
    console.warn("[tutorial] could not save tutorial flags", e);
  }
}

export const tutorialProgressStore = {
  get loaded() { return loaded(); },
  get hintDismissed() { return hintDismissed(); },
  get completed() { return completed(); },
  /** Once per app run. */
  load(): Promise<void> {
    loading ??= readFlags();
    return loading;
  },
  dismissHint(): Promise<void> {
    setHintDismissed(true);
    return write({ tutorialHintDismissed: true });
  },
  /** Sets the signals synchronously, before the first await. */
  markCompleted(): Promise<void> {
    setCompleted(true);
    setHintDismissed(true);
    return write({ tutorialHintDismissed: true, teamTutorialCompleted: true });
  },
};

// Test-only reset (gated to Vite MODE === "test", which vitest sets).
export function __resetTutorialProgressForTests() {
  if (import.meta.env.MODE !== "test") return;
  setLoaded(false);
  setHintDismissed(false);
  setCompleted(false);
  loading = null;
}
