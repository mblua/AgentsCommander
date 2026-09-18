let cachedContext: AudioContext | null = null;

let soundsEnabled = true;

export function setSoundsEnabled(enabled: boolean): void {
  soundsEnabled = enabled;
  if (!enabled) stopAllNonStopAlarms();
}

const COALESCE_WINDOW_S = 0.03;
let lastBeepStartedAt = Number.NEGATIVE_INFINITY;

const ALARM_FREQ_HZ = 880;
const ALARM_TONE_S = 0.4;
const ALARM_PERIOD_S = 0.55;
const ALARM_MIN_SECONDS = 1;
const ALARM_MAX_SECONDS = 60;

// Invariant F1' (replaces F1). stopNonStopAlarm is the ONLY function that removes
// an entry from liveAlarms, and it removes the whole entry at once. There is
// deliberately no `onended` self-cleanup: a superseded alarm's nodes would fire
// `onended` after a newer alarm for the same key was already stored, and removing
// "its own" key would delete the newer entry, leaving it unstoppable. That is the
// same orphaning failure the old Rust F1 comment
// (`non_stop_watchdog.rs:264-278`) protected against. A re-fire supersedes by
// stop-then-insert that runs to completion in one synchronous JS turn, so the pair
// is atomic with respect to any incoming `stop`; never make this function suspend,
// for the reason recorded in the header of `playNonStopAlarm` in this plan. A key
// that fires and never stops holds at most one entry of at most
// `ceil(60 / 0.55) = 110` finished nodes, bounded by project count, and is
// replaced on its next fire.
const liveAlarms = new Map<string, OscillatorNode[]>();
let alarmTimelineEnd = 0;
let suppressedAlarms = 0;

let primed = false;

function getAudioContext(): AudioContext | null {
  if (cachedContext) return cachedContext;
  const Ctor =
    window.AudioContext ??
    (window as unknown as { webkitAudioContext?: typeof AudioContext })
      .webkitAudioContext;
  if (!Ctor) return null;
  try {
    cachedContext = new Ctor();
  } catch {
    // No audio this session. cachedContext stays null, so a later call may retry.
    return null;
  }
  return cachedContext;
}

export function primeAudio(): void {
  if (primed) return;
  if (!getAudioContext()) return;
  primed = true;

  const unlock = () => {
    const ctx = getAudioContext();
    if (!ctx) return;
    if (ctx.state === "running") {
      removeUnlockListeners();
      return;
    }
    void ctx
      .resume()
      .then(() => {
        if (ctx.state === "running") removeUnlockListeners();
      })
      .catch(() => {});
  };
  // One named handler for all three registrations and removals: a removal with a
  // different reference would leave a live listener behind.
  const removeUnlockListeners = () => {
    window.removeEventListener("mousedown", unlock);
    window.removeEventListener("keydown", unlock);
    window.removeEventListener("touchstart", unlock);
  };

  // Not `{ once: true }`: the first gesture may arrive before the context can be
  // resumed, or its resume may fail. While the context is not running the
  // listeners stay armed, so any later gesture is another chance to unlock.
  window.addEventListener("mousedown", unlock);
  window.addEventListener("keydown", unlock);
  window.addEventListener("touchstart", unlock);
}

// The pulsed alarm (880 Hz tone for 0.4 s, 0.15 s gap) mirrors the Win32 `Beep`
// shape at `non_stop_watchdog.rs:301-302`, and the whole train is scheduled on the
// audio clock in one synchronous JS turn. It must never suspend: a resume driven
// by a user gesture can stay pending under an autoplay policy, and a pending
// window would let an incoming stop land before the key is registered, then
// schedule a burst for an episode that already recovered. One turn also makes the
// stop-then-insert pair atomic with respect to a stop.
export function playNonStopAlarm(key: string, seconds: number): void {
  if (!soundsEnabled) return;
  const ctx = getAudioContext();
  if (!ctx) return;
  if (ctx.state !== "running") {
    suppressedAlarms += 1;
    console.warn(
      `[non-stop] alarm suppressed: AudioContext is '${ctx.state}' (autoplay policy, no user gesture yet)`,
    );
    void ctx.resume().catch(() => {});
    return;
  }

  stopNonStopAlarm(key);

  const clamped = Number.isFinite(seconds)
    ? Math.min(ALARM_MAX_SECONDS, Math.max(ALARM_MIN_SECONDS, Math.floor(seconds)))
    : ALARM_MIN_SECONDS;
  const bursts = Math.ceil(clamped / ALARM_PERIOD_S);
  const start = Math.max(ctx.currentTime, alarmTimelineEnd);
  const nodes: OscillatorNode[] = [];
  for (let i = 0; i < bursts; i += 1) {
    nodes.push(
      scheduleTone(ctx, ALARM_FREQ_HZ, start + i * ALARM_PERIOD_S, ALARM_TONE_S),
    );
  }
  alarmTimelineEnd = start + bursts * ALARM_PERIOD_S;
  liveAlarms.set(key, nodes);
}

export function stopNonStopAlarm(key: string): void {
  const nodes = liveAlarms.get(key);
  if (!nodes) return;
  liveAlarms.delete(key);
  const ctx = getAudioContext();
  const at = ctx ? ctx.currentTime : 0;
  for (const osc of nodes) {
    try {
      osc.stop(at);
    } catch {
      // already stopped
    }
  }
  if (liveAlarms.size === 0 && ctx) alarmTimelineEnd = ctx.currentTime;
}

export function stopAllNonStopAlarms(): void {
  for (const key of Array.from(liveAlarms.keys())) stopNonStopAlarm(key);
}

export function __soundDiagnosticsForTests(): {
  suppressedAlarms: number;
  liveKeys: string[];
} {
  return { suppressedAlarms, liveKeys: Array.from(liveAlarms.keys()) };
}

export function __resetSoundStateForTests(): void {
  liveAlarms.clear();
  alarmTimelineEnd = 0;
  suppressedAlarms = 0;
  cachedContext = null;
  lastBeepStartedAt = Number.NEGATIVE_INFINITY;
  primed = false;
}

export async function playTeamIdleBeep(): Promise<void> {
  if (!soundsEnabled) return;
  const ctx = getAudioContext();
  if (!ctx) return;
  if (ctx.state === "suspended") {
    try {
      await ctx.resume();
    } catch {
      return;
    }
  }

  const now = ctx.currentTime;
  if (now - lastBeepStartedAt < COALESCE_WINDOW_S) return;
  lastBeepStartedAt = now;

  scheduleTone(ctx, 660, now, 0.12);
  scheduleTone(ctx, 880, now + 0.13, 0.14);
}

function scheduleTone(
  ctx: AudioContext,
  frequency: number,
  startTime: number,
  duration: number,
): OscillatorNode {
  const osc = ctx.createOscillator();
  const gain = ctx.createGain();

  osc.type = "sine";
  osc.frequency.value = frequency;

  const peakGain = 0.12;
  const attack = 0.012;
  const release = 0.06;
  gain.gain.setValueAtTime(0, startTime);
  gain.gain.linearRampToValueAtTime(peakGain, startTime + attack);
  gain.gain.setValueAtTime(peakGain, startTime + duration - release);
  gain.gain.linearRampToValueAtTime(0, startTime + duration);

  osc.connect(gain);
  gain.connect(ctx.destination);

  osc.start(startTime);
  osc.stop(startTime + duration + 0.02);

  return osc;
}
