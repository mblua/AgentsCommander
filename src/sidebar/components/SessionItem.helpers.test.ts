// @vitest-environment jsdom
import { describe, expect, it, vi } from "vitest";
import {
  detachUi,
  micButtonClass,
  micButtonTitle,
  repoBranchLabel,
  sessionChipName,
  sessionDisplayName,
  sessionRowState,
  telegramUi,
} from "./SessionItem";

// #2611 — the SessionItem JSX ternaries moved into these helpers. The expected
// strings are byte-equal to the old inline templates (double spaces included).

describe("SessionItem helpers (#2611)", () => {
  it("sessionRowState reads `inactive` only when not active", () => {
    const inactive = vi.fn(() => true);
    expect(sessionRowState(true, inactive)).toBe("active");
    expect(inactive).not.toHaveBeenCalled();
    expect(sessionRowState(false, inactive)).toBe("inactive");
    expect(sessionRowState(false, () => false)).toBe("idle");
  });

  it.each([
    [false, false, null, true, "session-item-mic    "],
    [true, false, null, true, "session-item-mic recording   "],
    [false, true, null, true, "session-item-mic  processing  "],
    [false, false, "no mic", true, "session-item-mic   error "],
    [false, false, null, false, "session-item-mic    disabled"],
    [true, true, "x", false, "session-item-mic recording processing error disabled"],
    [false, false, "", true, "session-item-mic    "],
  ])("micButtonClass(%j, %j, %j, %j)", (recording, processing, micError, voiceEnabled, expected) => {
    expect(micButtonClass(recording, processing, micError, voiceEnabled)).toBe(expected);
  });

  it("micButtonTitle keeps the old check order and lazy reads", () => {
    const recording = vi.fn(() => true);
    const processing = vi.fn(() => true);
    const micError = vi.fn(() => "no mic");
    expect(micButtonTitle(false, recording, processing, micError)).toBe(
      "Enable voice-to-text in Settings and set a Gemini API key to use this.",
    );
    expect(recording).not.toHaveBeenCalled();
    expect(micButtonTitle(true, recording, processing, micError)).toBe("Stop recording");
    expect(processing).not.toHaveBeenCalled();
    expect(micButtonTitle(true, () => false, processing, micError)).toBe("Transcribing...");
    expect(micError).not.toHaveBeenCalled();
    expect(micButtonTitle(true, () => false, () => false, micError)).toBe("no mic");
    expect(micButtonTitle(true, () => false, () => false, () => null)).toBe("Voice to text");
    expect(micButtonTitle(true, () => false, () => false, () => "")).toBe("Voice to text");
  });

  it("repoBranchLabel", () => {
    expect(repoBranchLabel({ label: "repo-a", branch: "main" })).toBe("repo-a/main");
    expect(repoBranchLabel({ label: "repo-a", branch: null })).toBe("repo-a");
    expect(repoBranchLabel({ label: "repo-a", branch: "" })).toBe("repo-a");
    expect(repoBranchLabel({ label: "repo-a" })).toBe("repo-a");
  });

  it("detachUi returns the old strings", () => {
    expect(detachUi(true)).toEqual({
      title: "Re-attach session",
      state: "detached",
      menuLabel: "Re-attach session",
    });
    expect(detachUi(false)).toEqual({
      title: "Detach session",
      state: "attached",
      menuLabel: "Detach session",
    });
  });

  it("telegramUi returns the old title and style", () => {
    expect(telegramUi({ botLabel: "Ops Bot", color: "#ff0000" })).toEqual({
      title: "Detach Telegram: Ops Bot",
      style: { color: "#ff0000" },
    });
    expect(telegramUi(null)).toEqual({ title: "Attach Telegram", style: {} });
    expect(telegramUi(undefined)).toEqual({ title: "Attach Telegram", style: {} });
  });

  const origin = (value?: string) => () => value;

  it.each([
    // project path: agent dir @ project folder, both __agent_ and _agent_ prefixes
    ["C:\\Proj\\.ac\\wg-1-team\\__agent_dev", undefined, "dev@Proj", "dev"],
    ["/home/u/Proj/.ac/_agent_dev/", undefined, "dev@Proj", "dev"],
    ["/home/u/Proj/.ac/wg-1-team/__agent_dev", "Origin", "dev@Origin", "dev"],
    // "@" inside names must not mis-split
    ["/home/u/my@proj/.ac/wg-1-team/__agent_a@b", undefined, "a@b@my@proj", "a@b"],
    ["/home/u/Pr@j/.ac/wg-1-team/__agent_x", "O@rigin", "x@O@rigin", "x"],
    // two-segment fallback
    ["/home/u/some@dir/leaf@x", undefined, "some@dir/leaf@x", "leaf@x"],
    ["C:\\a\\b\\", undefined, "a/b", "b"],
    // single segment and empty working directory
    ["solo", undefined, "solo", "solo"],
    ["", undefined, "sess-name", "sess-name"],
  ])("sessionDisplayName/sessionChipName(%j, %j)", (wd, originProject, display, chip) => {
    const full = sessionDisplayName({ workingDirectory: wd, name: "sess-name" }, origin(originProject));
    expect(full).toBe(display);
    expect(sessionChipName(full, wd, origin(originProject))).toBe(chip);
  });

  it("sessionDisplayName falls back to the session name for a lone separator", () => {
    expect(sessionDisplayName({ workingDirectory: "/", name: "n" }, origin())).toBe("n");
  });
});
