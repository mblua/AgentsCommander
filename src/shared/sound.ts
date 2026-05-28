// Audio playback utilities. Synthesized via Web Audio API to avoid
// bundling a binary asset and to render identically across the Tauri
// webview and the browser-served WS transport.

let cachedContext: AudioContext | null = null;

// Global app-sound master switch (#158). Single source of truth that every
// playback function in this module checks at entry. Decoupled from any
// settings store so this module stays leaf-level (no circular imports);
// settings code pushes the value via setSoundsEnabled on load/refresh and
// on toolbar mute toggle. Default true matches the Rust-side default and
// keeps users audible if the FE settings load fails.
let soundsEnabled = true;

export function setSoundsEnabled(enabled: boolean): void {
  soundsEnabled = enabled;
}

// Coalesce window for back-to-back beep calls. When several workgroups
// transition to idle in the same effect tick, the watcher fires
// playTeamIdleBeep() multiple times synchronously; without coalescing,
// the oscillators stack at the same currentTime and sum to N× peak
// gain (loud, harsh chord instead of one soft tone).
const COALESCE_WINDOW_S = 0.03;
let lastBeepStartedAt = Number.NEGATIVE_INFINITY;

function getAudioContext(): AudioContext | null {
  if (cachedContext) return cachedContext;
  const Ctor =
    window.AudioContext ??
    (window as unknown as { webkitAudioContext?: typeof AudioContext })
      .webkitAudioContext;
  if (!Ctor) return null;
  cachedContext = new Ctor();
  return cachedContext;
}

// Chromium-based webviews (incl. Tauri) start AudioContext in "suspended"
// until a user gesture. If the very first beep fires before the user
// has clicked or typed inside the window, ctx.resume() rejects silently.
// Pre-arming on the first global gesture unlocks the context so later
// beeps play even if the user has alt-tabbed away by then.
export function primeAudio(): void {
  const unlock = () => {
    const ctx = getAudioContext();
    if (ctx && ctx.state === "suspended") {
      ctx.resume().catch(() => {});
    }
  };
  window.addEventListener("mousedown", unlock, { once: true });
  window.addEventListener("keydown", unlock, { once: true });
  window.addEventListener("touchstart", unlock, { once: true });
}

/**
 * Soft two-step beep that fires when an entire team transitions from
 * busy → all-idle. Two short tones (660 Hz then 880 Hz, ~120 ms each)
 * with a quick attack/release envelope so it doesn't click. Total under
 * ~280 ms - short enough not to be annoying, distinct enough to register
 * as "team finished".
 *
 * Errors (no AudioContext, suspended context that can't be resumed) are
 * swallowed - failing to beep must never break the FE.
 */
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
): void {
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
}
