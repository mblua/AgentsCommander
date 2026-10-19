import { createSignal } from "solid-js";
import type { AppSettings } from "../types";
import {
  SettingsAPI, onCodingAgentSettingsUpdated, onRoomIdleConfirmationUpdated,
  onTransportConnectionState, getTransportConnectionState,
} from "../ipc";
import { setSoundsEnabled } from "../sound";

const [settings, setSettings] = createSignal<AppSettings | null>(null);
type Connection = ReturnType<typeof getTransportConnectionState>;
let connection: Connection | undefined;
let lifecycle = 0;
let request = 0;
let ready = false;
let setup: Promise<void> | undefined;
let unlisteners: (() => void)[] = [];

function acceptConnection(next: Connection): boolean {
  if (connection && (next.generation < connection.generation ||
    (next.generation === connection.generation && next.state === connection.state))) return false;
  connection = next;
  ++request;
  return true;
}

async function fetchSettings() {
  const realm = lifecycle;
  const generation = connection?.generation;
  const id = ++request;
  if (!ready || connection?.state !== "connected") return;
  const value = await SettingsAPI.get();
  const snapshot = getTransportConnectionState();
  if (realm !== lifecycle || id !== request || !ready ||
    connection?.state !== "connected" || connection.generation !== generation ||
    snapshot.state !== "connected" || snapshot.generation !== generation) return;
  setSettings(value);
  // Missing legacy sound preferences remain audible until explicitly muted.
  setSoundsEnabled(value.soundsEnabled ?? true);
}

function ensureSubscriptions(): Promise<void> {
  if (setup) return setup;
  const realm = lifecycle;
  const acquired: (() => void)[] = [];
  unlisteners = acquired;
  const register = async (pending: Promise<() => void>) => {
    const unlisten = await pending;
    if (realm !== lifecycle) {
      unlisten();
      return false;
    }
    acquired.push(unlisten);
    return true;
  };
  const invalidate = () => {
    if (realm !== lifecycle) return;
    ++request;
    if (ready) settingsStore.refresh();
  };
  setup = Promise.resolve().then(async () => {
    try {
      if (!await register(onCodingAgentSettingsUpdated(invalidate))) return;
      if (!await register(onRoomIdleConfirmationUpdated(invalidate))) return;
      if (!await register(onTransportConnectionState(next => {
        if (realm !== lifecycle || !acceptConnection(next)) return;
        if (ready && next.state === "connected") settingsStore.refresh();
      }))) return;
      acceptConnection(getTransportConnectionState());
      ready = true;
    } catch (error) {
      for (const unlisten of acquired) unlisten();
      if (realm === lifecycle) {
        ++lifecycle;
        ++request;
        connection = undefined;
        unlisteners = [];
        setup = undefined;
      }
      console.error("settingsStore subscriptions failed:", error);
      throw error;
    }
    // A failed bootstrap keeps registered listeners, so the next event/load can retry.
    await fetchSettings();
  });
  return setup;
}

export const settingsStore = {
  get current() { return settings(); },
  get voiceEnabled() {
    const s = settings();
    return !!s?.voiceToTextEnabled && !!s?.geminiApiKey;
  },
  async load() {
    if (!ready) return ensureSubscriptions();
    acceptConnection(getTransportConnectionState());
    await fetchSettings();
  },
  refresh() {
    settingsStore.load().catch(error => {
      console.error("settingsStore.refresh() failed:", error);
    });
  },
};

// Compatibility entry point; every caller shares the realm's one setup/bootstrap.
export function __subscribeCodingAgentSettingsUpdates() {
  return ensureSubscriptions();
}

export function __disposeSettingsSubscriptionsForTests() {
  ++lifecycle;
  ++request;
  ready = false;
  setup = undefined;
  connection = undefined;
  for (const unlisten of unlisteners) unlisten();
  unlisteners = [];
  setSettings(null);
}

if (import.meta.env.MODE !== "test") {
  void ensureSubscriptions().catch(error => {
    console.error("settingsStore bootstrap failed:", error);
  });
}
