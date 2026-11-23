// Sound button state, label, the one sink check and the sticky toast.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ tauri: true, getStatus: vi.fn() }));
vi.mock("../shared/ipc", () => ({ AudioSinkAPI: { getStatus: mocks.getStatus } }));
vi.mock("../shared/platform", () => ({
  get isTauri() {
    return mocks.tauri;
  },
}));

import { createAudioSinkAPI, type AudioSinkStatus } from "../shared/audio-sink";
import { toastStore } from "../shared/stores/toasts";
import { FakeTransport } from "../shared/testing/fake-transport";
import {
  __setAudioSinkStatusForTests,
  audioSinkStatus,
  checkAudioSink,
  showAudioSinkToast,
  soundButtonLabel,
  soundButtonState,
  type SoundButtonState,
} from "./audio-sink-warning";

const EXPECTED_MESSAGE =
  "Sound alerts cannot play: the GStreamer element autoaudiosink is missing. Install GStreamer good plugins, then restart AgentsCommander:\n" +
  "- Arch: sudo pacman -S gst-plugins-good\n" +
  "- Debian/Ubuntu: sudo apt install gstreamer1.0-plugins-good\n" +
  "- Fedora: sudo dnf install gstreamer1-plugins-good";

beforeEach(() => {
  __setAudioSinkStatusForTests("unknown");
  toastStore.clear();
  vi.clearAllMocks();
  mocks.tauri = true;
});

afterEach(() => {
  vi.useRealTimers();
});

describe("soundButtonState (#4843)", () => {
  const TABLE: [boolean, boolean, AudioSinkStatus, SoundButtonState][] = [
    [false, false, "ok", "disabled"],
    [false, false, "missing", "disabled"],
    [false, false, "unknown", "disabled"],
    [false, true, "ok", "disabled"],
    [false, true, "missing", "disabled"],
    [false, true, "unknown", "disabled"],
    [true, false, "ok", "muted"],
    [true, false, "missing", "muted"],
    [true, false, "unknown", "muted"],
    [true, true, "ok", "audible"],
    [true, true, "missing", "alert"],
    [true, true, "unknown", "audible"],
  ];

  it.each(TABLE)("loaded=%s enabled=%s sink=%s -> %s", (loaded, enabled, sink, state) => {
    expect(soundButtonState(loaded, enabled, sink)).toBe(state);
  });
});

describe("soundButtonLabel (#4843)", () => {
  it.each([
    ["disabled", "Mute all app sounds"],
    ["audible", "Mute all app sounds"],
    ["alert", "Sound alerts cannot play. Click for details and to mute."],
    ["muted", "Unmute app sounds"],
  ] as const)("%s -> %s", (state, label) => {
    expect(soundButtonLabel(state)).toBe(label);
  });
});

describe("checkAudioSink (#4843)", () => {
  it("does not ask the backend outside Tauri", async () => {
    mocks.tauri = false;
    await expect(checkAudioSink()).resolves.toBeUndefined();
    expect(mocks.getStatus).not.toHaveBeenCalled();
    expect(audioSinkStatus()).toBe("unknown");
  });

  it("stores missing and calls getStatus once with no arguments", async () => {
    mocks.getStatus.mockResolvedValueOnce("missing");
    await checkAudioSink();
    expect(audioSinkStatus()).toBe("missing");
    expect(mocks.getStatus).toHaveBeenCalledTimes(1);
    expect(mocks.getStatus.mock.calls[0]).toEqual([]);
  });

  it.each([
    ["ok", "ok"],
    ["bogus", "unknown"],
  ] as const)("resolved %s -> %s", async (value, status) => {
    mocks.getStatus.mockResolvedValueOnce(value);
    await checkAudioSink();
    expect(audioSinkStatus()).toBe(status);
  });

  it("a rejected invoke resolves and leaves unknown", async () => {
    __setAudioSinkStatusForTests("missing");
    mocks.getStatus.mockRejectedValueOnce(new Error("x"));
    await expect(checkAudioSink()).resolves.toBeUndefined();
    expect(audioSinkStatus()).toBe("unknown");
  });
});

describe("showAudioSinkToast (#4843)", () => {
  it("shows one sticky error toast, deduplicated by tag", () => {
    vi.useFakeTimers();
    showAudioSinkToast();
    showAudioSinkToast();
    vi.advanceTimersByTime(600000);
    expect(toastStore.items).toHaveLength(1);
    const [toast] = toastStore.items;
    expect(toast?.kind).toBe("error");
    expect(toast?.tag).toBe("audio-sink-missing");
    expect(toast?.message).toBe(EXPECTED_MESSAGE);
    expect(toast?.exiting).toBe(false);
  });
});

describe("createAudioSinkAPI (#4843)", () => {
  it("invokes get_audio_sink_status with no arguments", async () => {
    const fake = new FakeTransport();
    fake.resolve("get_audio_sink_status", "missing");
    await expect(createAudioSinkAPI(fake).getStatus()).resolves.toBe("missing");
    expect(fake.calls).toEqual([{ cmd: "get_audio_sink_status", args: {} }]);
  });
});
