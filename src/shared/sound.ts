
let cachedContext: AudioContext | null = null;

let soundsEnabled = true;

export function setSoundsEnabled(enabled: boolean): void {
  soundsEnabled = enabled;
}

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
