// The sidebar sound button when GStreamer cannot play sound (Linux).
import { createSignal } from "solid-js";
import type { AudioSinkStatus } from "../shared/audio-sink";
import { AudioSinkAPI } from "../shared/ipc";
import { isTauri } from "../shared/platform";
import { toastStore } from "../shared/stores/toasts";

export type SoundButtonState = "disabled" | "audible" | "alert" | "muted";

export const AUDIO_SINK_TOAST_TAG = "audio-sink-missing";

export const AUDIO_SINK_MESSAGE = [
  "Sound alerts cannot play: the GStreamer element autoaudiosink is missing. " +
    "Install GStreamer good plugins, then restart AgentsCommander:",
  "- Arch: sudo pacman -S gst-plugins-good",
  "- Debian/Ubuntu: sudo apt install gstreamer1.0-plugins-good",
  "- Fedora: sudo dnf install gstreamer1-plugins-good",
].join("\n");

const LABELS: Record<SoundButtonState, string> = {
  disabled: "Mute all app sounds",
  audible: "Mute all app sounds",
  alert: "Sound alerts cannot play. Click for details and to mute.",
  muted: "Unmute app sounds",
};

const [status, setStatus] = createSignal<AudioSinkStatus>("unknown");

/** The sink status; "unknown" until the one check answers. */
export const audioSinkStatus = status;

/** Ask the backend; called once per sidebar start. Never rejects. */
export async function checkAudioSink(): Promise<void> {
  try {
    if (!isTauri) return;
    const value = await AudioSinkAPI.getStatus();
    setStatus(value === "ok" || value === "missing" ? value : "unknown");
  } catch {
    setStatus("unknown");
  }
}

export function soundButtonState(
  loaded: boolean,
  enabled: boolean,
  sink: AudioSinkStatus,
): SoundButtonState {
  if (!loaded) return "disabled";
  if (!enabled) return "muted";
  return sink === "missing" ? "alert" : "audible";
}

export function soundButtonLabel(state: SoundButtonState): string {
  return LABELS[state];
}

/** Sticky, deduplicated by tag. */
export function showAudioSinkToast(): void {
  toastStore.error(AUDIO_SINK_MESSAGE, { durationMs: null, tag: AUDIO_SINK_TOAST_TAG });
}

export function __setAudioSinkStatusForTests(next: AudioSinkStatus): void {
  if (import.meta.env.MODE !== "test") {
    throw new Error("__setAudioSinkStatusForTests is test-only");
  }
  setStatus(next);
}
