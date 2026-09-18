// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  __resetSoundStateForTests,
  __soundDiagnosticsForTests,
  playNonStopAlarm,
  playTeamIdleBeep,
  primeAudio,
  setSoundsEnabled,
  stopNonStopAlarm,
} from "./sound";

type FakeOscillator = {
  frequency: { value: number };
  startTime: number;
  stopCalls: number[];
  throwOnStopCall: number;
};

type FakeGainCall = {
  method: "setValueAtTime" | "linearRampToValueAtTime";
  value: number;
  time: number;
};

type FakeGain = {
  calls: FakeGainCall[];
};

// The fake records `stopCalls`, never a "was stop called" boolean. scheduleTone
// already schedules its own stop on every oscillator the moment it is built, so a
// boolean would read true for all 110 nodes before any mute and would keep this
// suite green against a build with no mute-stop at all.
class FakeAudioContext {
  static constructions = 0;
  static instances: FakeAudioContext[] = [];
  static lastInstance: FakeAudioContext | null = null;
  static defaultState: AudioContextState = "running";

  readonly oscillators: FakeOscillator[] = [];
  readonly gains: FakeGain[] = [];
  state: AudioContextState = FakeAudioContext.defaultState;
  currentTime = 0;
  resumeCalls = 0;
  resumeImpl: (() => void) | null = null;
  destination = {} as AudioDestinationNode;

  constructor() {
    FakeAudioContext.constructions += 1;
    FakeAudioContext.instances.push(this);
    FakeAudioContext.lastInstance = this;
  }

  resume(): Promise<void> {
    this.resumeCalls += 1;
    const impl = this.resumeImpl;
    if (!impl) return Promise.resolve();
    try {
      impl();
      return Promise.resolve();
    } catch (error) {
      return Promise.reject(error);
    }
  }

  createOscillator(): OscillatorNode {
    const record: FakeOscillator = {
      frequency: { value: 0 },
      startTime: 0,
      stopCalls: [],
      throwOnStopCall: 0,
    };
    this.oscillators.push(record);
    const node = {
      type: "sine",
      frequency: record.frequency,
      connect: () => {},
      start: (when: number) => {
        record.startTime = when;
      },
      stop: (when: number) => {
        record.stopCalls.push(when);
        if (record.throwOnStopCall > 0 && record.stopCalls.length >= record.throwOnStopCall) {
          throw new Error("fake oscillator: stop rejected");
        }
      },
    };
    return node as unknown as OscillatorNode;
  }

  createGain(): GainNode {
    const record: FakeGain = { calls: [] };
    this.gains.push(record);
    const gain = {
      gain: {
        setValueAtTime: (value: number, time: number) => {
          record.calls.push({ method: "setValueAtTime", value, time });
        },
        linearRampToValueAtTime: (value: number, time: number) => {
          record.calls.push({ method: "linearRampToValueAtTime", value, time });
        },
      },
      connect: () => {},
    };
    return gain as unknown as GainNode;
  }
}

const originalAudioContextDescriptor = Object.getOwnPropertyDescriptor(window, "AudioContext");

function installAudioContext(ctor: unknown): void {
  Object.defineProperty(window, "AudioContext", {
    value: ctor,
    configurable: true,
    writable: true,
  });
}

function resetFakeAudioContext(): void {
  FakeAudioContext.constructions = 0;
  FakeAudioContext.instances = [];
  FakeAudioContext.lastInstance = null;
  FakeAudioContext.defaultState = "running";
}

function allOscillators(): FakeOscillator[] {
  return FakeAudioContext.instances.flatMap((instance) => instance.oscillators);
}

function fakeContext(): FakeAudioContext {
  const instance = FakeAudioContext.lastInstance;
  if (!instance) throw new Error("the module did not construct a fake AudioContext");
  return instance;
}

// The cut predicate, verbatim: a tone was cut at `at` when
// stopCalls.length === 2 && stopCalls[1] <= at. Entry [0] is scheduleTone's own
// scheduled end; entry [1] is the explicit cut written by stopNonStopAlarm.
// Asserting both the length and the ordering against currentTime separates them:
// a boolean "was stop called" fake would satisfy every node the moment
// playNonStopAlarm returned, and the length bound also catches a double cut.
function expectToneCut(node: FakeOscillator, at: number): void {
  expect(node.stopCalls).toHaveLength(2);
  expect(node.stopCalls[1]).toBeLessThanOrEqual(at);
}

beforeEach(() => {
  __resetSoundStateForTests();
  resetFakeAudioContext();
  installAudioContext(FakeAudioContext);
  setSoundsEnabled(true);
});

afterEach(() => {
  vi.restoreAllMocks();
  if (originalAudioContextDescriptor) {
    Object.defineProperty(window, "AudioContext", originalAudioContextDescriptor);
    return;
  }
  Reflect.deleteProperty(window as unknown as Record<string, unknown>, "AudioContext");
});

describe("sound.ts non-stop alarm", () => {
  it("case 1: the global mute returns before any context or node exists", () => {
    setSoundsEnabled(false);
    playNonStopAlarm("p", 3);

    expect(allOscillators()).toHaveLength(0);
    expect(FakeAudioContext.constructions).toBe(0);
  });

  it("case 2: unmuted playback is the positive control for case 1", () => {
    playNonStopAlarm("p", 3);

    expect(FakeAudioContext.constructions).toBe(1);
    expect(allOscillators()).toHaveLength(6);
  });

  it("case 3: ten seconds is 19 bursts of 880 Hz spaced 0.55 s apart", () => {
    playNonStopAlarm("p", 10);
    const nodes = fakeContext().oscillators;

    expect(nodes).toHaveLength(19);
    for (const node of nodes) {
      expect(node.frequency.value).toBe(880);
      // stopCalls[0] is scheduleTone's own scheduled end: startTime + tone +
      // 0.02. 0.42 pins ALARM_TONE_S at 0.4; a longer tone swallows the 0.15 s
      // gap and turns the pulsed alarm into a continuous one.
      expect(node.stopCalls[0] - node.startTime).toBeCloseTo(0.42, 9);
    }
    for (let index = 1; index < nodes.length; index += 1) {
      const delta = nodes[index].startTime - nodes[index - 1].startTime;
      expect(delta).toBeCloseTo(0.55, 9);
    }
    // The alarm's gain envelope, recorded from the fake param: the peak is
    // 0.12, so a peakGain mutation to 1.0 shows up here.
    expect(fakeContext().gains[0].calls.map((call) => call.value)).toEqual([0, 0.12, 0.12, 0]);
  });

  it("case 4: seconds are clamped into 1..60", () => {
    const cases: Array<[number, number]> = [
      [0, 2],
      [-5, 2],
      [Number.NaN, 2],
      // 1.5 is the floor pin: rounding, ceiling or dropping it gives 4 bursts.
      [1.5, 2],
      [600, 110],
    ];

    for (const [seconds, expected] of cases) {
      const before = allOscillators().length;
      playNonStopAlarm("p", seconds);
      expect(allOscillators().length - before).toBe(expected);
    }
  });

  it("case 5: stop cuts every node of the train and rewinds the timeline", () => {
    playNonStopAlarm("p", 10);
    const ctx = fakeContext();
    expect(ctx.oscillators).toHaveLength(19);

    // Move the clock off 0 so the rewind must land on "now", not on 0.
    ctx.currentTime = 3;
    const at = ctx.currentTime;
    stopNonStopAlarm("p");

    for (const node of ctx.oscillators) expectToneCut(node, at);
    expect(__soundDiagnosticsForTests().liveKeys).toEqual([]);

    // Post-stop: without the rewind the next alarm inherits the cut train's
    // end (10.45 s) and starts there instead of here.
    playNonStopAlarm("q", 1);
    const next = ctx.oscillators.slice(19);
    expect(next).toHaveLength(2);
    expect(next[0].startTime).toBeCloseTo(at, 9);
  });

  it("case 6: a node that rejects its cut does not spare the rest", () => {
    playNonStopAlarm("p", 10);
    const ctx = fakeContext();

    // The 5th node rejects only its second stop call, so scheduleTone's own
    // build-time scheduling call still succeeds and the rejection lands on the
    // explicit cut.
    ctx.oscillators[4].throwOnStopCall = 2;

    const at = ctx.currentTime;
    expect(() => stopNonStopAlarm("p")).not.toThrow();

    ctx.oscillators.forEach((node, index) => {
      if (index === 4) return;
      expectToneCut(node, at);
    });
    expect(__soundDiagnosticsForTests().liveKeys).toEqual([]);
  });

  it("case 7: an unknown key is a no-op", () => {
    expect(() => stopNonStopAlarm("nope")).not.toThrow();

    expect(allOscillators()).toHaveLength(0);
    expect(FakeAudioContext.constructions).toBe(0);
  });

  it("case 8: consecutive alarms are laid end to end, not overlapped", () => {
    playNonStopAlarm("a", 5);
    const ctx = fakeContext();
    const first = ctx.oscillators.slice();

    playNonStopAlarm("b", 5);
    const second = ctx.oscillators.slice(first.length);

    expect(first).toHaveLength(10);
    expect(second).toHaveLength(10);
    const firstEnd = first[first.length - 1].startTime + 0.55;
    expect(second[0].startTime).toBeGreaterThanOrEqual(firstEnd - 1e-9);
    expect(second[0].startTime).toBeCloseTo(firstEnd, 9);
  });

  it("case 9: a re-fire cuts the superseded train, the explicit stop the new one", () => {
    playNonStopAlarm("p", 10);
    const ctx = fakeContext();
    const superseded = ctx.oscillators.slice();

    playNonStopAlarm("p", 10);
    const current = ctx.oscillators.slice(superseded.length);

    const at = ctx.currentTime;
    stopNonStopAlarm("p");

    expect(superseded).toHaveLength(19);
    expect(current).toHaveLength(19);
    for (const node of [...superseded, ...current]) expectToneCut(node, at);
    expect(__soundDiagnosticsForTests().liveKeys).toEqual([]);
  });

  it("case 10: a suspended context drops the alarm, counts it and asks for a resume", () => {
    FakeAudioContext.defaultState = "suspended";
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});

    playNonStopAlarm("p", 3);
    playNonStopAlarm("q", 3);
    const ctx = fakeContext();

    expect(ctx.oscillators).toHaveLength(0);
    expect(ctx.resumeCalls).toBe(2);
    const diagnostics = __soundDiagnosticsForTests();
    // Two dropped alarms must count two: a plain assignment would still read 1.
    expect(diagnostics.suppressedAlarms).toBe(2);
    expect(diagnostics.liveKeys).toEqual([]);
    expect(warnSpy).toHaveBeenCalledTimes(2);
    const message = String(warnSpy.mock.calls[0][0]);
    expect(message.startsWith("[non-stop] alarm suppressed: AudioContext is 'suspended'")).toBe(
      true,
    );
  });

  it("case 11: a running context plays and counts no suppression", () => {
    FakeAudioContext.defaultState = "running";

    playNonStopAlarm("p", 3);
    const ctx = fakeContext();

    expect(ctx.oscillators).toHaveLength(6);
    expect(__soundDiagnosticsForTests().suppressedAlarms).toBe(0);
  });

  it("case 12: primeAudio keeps every gesture armed until the context runs", async () => {
    primeAudio();
    // A second call must not stack a second set of listeners: one gesture below
    // would then request two resumes.
    primeAudio();
    const ctx = fakeContext();
    ctx.state = "suspended";

    window.dispatchEvent(new MouseEvent("mousedown"));
    expect(ctx.resumeCalls).toBe(1);

    ctx.resumeImpl = () => {
      ctx.state = "running";
    };
    window.dispatchEvent(new MouseEvent("mousedown"));
    expect(ctx.resumeCalls).toBe(2);

    window.dispatchEvent(new MouseEvent("mousedown"));
    expect(ctx.resumeCalls).toBe(2);

    await Promise.resolve();
    // Trap: only a still-armed listener would resume a suspended context again.
    ctx.state = "suspended";
    window.dispatchEvent(new MouseEvent("mousedown"));
    window.dispatchEvent(new Event("keydown"));
    window.dispatchEvent(new Event("touchstart"));
    expect(ctx.resumeCalls).toBe(2);
  });

  it("case 13: playTeamIdleBeep keeps its tones, its mute gate and its coalescing", async () => {
    await playTeamIdleBeep();
    const ctx = fakeContext();
    expect(ctx.oscillators).toHaveLength(2);
    expect(ctx.oscillators[0].frequency.value).toBe(660);
    expect(ctx.oscillators[1].frequency.value).toBe(880);

    await playTeamIdleBeep();
    expect(ctx.oscillators).toHaveLength(2);

    ctx.currentTime = 1;
    await playTeamIdleBeep();
    expect(ctx.oscillators).toHaveLength(4);

    setSoundsEnabled(false);
    await playTeamIdleBeep();
    expect(ctx.oscillators).toHaveLength(4);

    // The test reset clears the coalescing window too: a beep after it must
    // schedule even though the previous pair sits at the same fake currentTime.
    __resetSoundStateForTests();
    setSoundsEnabled(true);
    await playTeamIdleBeep();
    expect(allOscillators()).toHaveLength(6);
  });

  it("case 14: a throwing AudioContext constructor cannot reach the mount path", () => {
    let attempts = 0;
    class ThrowingAudioContext {
      constructor() {
        attempts += 1;
        throw new Error("no audio device");
      }
    }
    installAudioContext(ThrowingAudioContext);

    expect(() => primeAudio()).not.toThrow();
    expect(attempts).toBeGreaterThanOrEqual(1);

    expect(() => playNonStopAlarm("p", 3)).not.toThrow();
    expect(allOscillators()).toHaveLength(0);
    expect(__soundDiagnosticsForTests().liveKeys).toEqual([]);
  });

  it("case 15: the alarm path returns undefined and registers its nodes in the same turn", () => {
    const result = playNonStopAlarm("p", 3);
    expect(result).toBeUndefined();
    expect(fakeContext().oscillators).toHaveLength(6);

    __resetSoundStateForTests();
    resetFakeAudioContext();
    FakeAudioContext.defaultState = "suspended";
    installAudioContext(FakeAudioContext);
    setSoundsEnabled(true);

    const dropped = playNonStopAlarm("p", 3);
    expect(dropped).toBeUndefined();
    expect(() => stopNonStopAlarm("p")).not.toThrow();
    expect(__soundDiagnosticsForTests().liveKeys).toEqual([]);
  });

  it("case 16: muting kills an alarm that is already sounding", () => {
    // Honest scope: this proves the explicit cut call reaches every node, not
    // that the speakers fall silent. Audible silence is proven only by the
    // Phase 4 manual check 1b; this case is the regression gate.
    playNonStopAlarm("p", 60);
    const ctx = fakeContext();
    expect(ctx.oscillators).toHaveLength(110);
    expect(__soundDiagnosticsForTests().liveKeys).toEqual(["p"]);

    const at = ctx.currentTime;
    setSoundsEnabled(false);

    for (const node of ctx.oscillators) expectToneCut(node, at);
    expect(__soundDiagnosticsForTests().liveKeys).toEqual([]);
    expect(ctx.oscillators).toHaveLength(110);

    setSoundsEnabled(true);
    expect(ctx.oscillators).toHaveLength(110);
    expect(__soundDiagnosticsForTests().liveKeys).toEqual([]);
    expect(() => setSoundsEnabled(false)).not.toThrow();
  });
});
