import { createSignal } from "solid-js";
import { DebugAPI, PtyAPI, SettingsAPI, VoiceAPI } from "./ipc";
import { getConsoleText } from "./console-capture";

interface OperationLease {
  sessionId: string;
  generation: number;
}

const [recordingSessionId, setRecordingSessionId] = createSignal<string | null>(null);
const [processingSessionId, setProcessingSessionId] = createSignal<string | null>(null);
const [micError, setMicError] = createSignal<string | null>(null);
const [recordingSeconds, setRecordingSeconds] = createSignal(0);
const [audioLevel, setAudioLevel] = createSignal(0);
const [autoExecuteSessionId, setAutoExecuteSessionId] = createSignal<string | null>(null);
const [autoExecuteCountdown, setAutoExecuteCountdown] = createSignal(0);
const [typingWarnSessionId, setTypingWarnSessionId] = createSignal<string | null>(null);

let leaseGeneration = 0;
let currentLease: OperationLease | null = null;
let backendRecordingSessionId: string | null = null;
let recorder: MediaRecorder | null = null;
let currentStream: MediaStream | null = null;
let audioContext: AudioContext | null = null;
let analyser: AnalyserNode | null = null;
let chunks: Blob[] = [];
let recordingTimer: ReturnType<typeof setInterval> | null = null;
let levelTimer: ReturnType<typeof setInterval> | null = null;
let autoExecuteTimer: ReturnType<typeof setInterval> | null = null;
let typingWarningTimer: ReturnType<typeof setTimeout> | null = null;
let micErrorTimer: ReturnType<typeof setTimeout> | null = null;

function isCurrent(lease: OperationLease): boolean {
  return (
    currentLease?.generation === lease.generation &&
    currentLease.sessionId === lease.sessionId
  );
}

function describeError(error: unknown, fallback: string): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message || error.name || fallback;
  return fallback;
}

function stopTracks(stream: MediaStream | null): void {
  stream?.getTracks().forEach((track) => track.stop());
}

function stopAudioLevelMonitor(): void {
  if (levelTimer) {
    clearInterval(levelTimer);
    levelTimer = null;
  }
  const context = audioContext;
  audioContext = null;
  analyser = null;
  if (context) {
    void context.close().catch((error: unknown) => {
      console.warn("[Voice] AudioContext close failed:", error);
    });
  }
  setAudioLevel(0);
}

function clearRecordingTimer(): void {
  if (recordingTimer) {
    clearInterval(recordingTimer);
    recordingTimer = null;
  }
  stopAudioLevelMonitor();
}

function cancelTypingWarning(): void {
  if (typingWarningTimer) {
    clearTimeout(typingWarningTimer);
    typingWarningTimer = null;
  }
  setTypingWarnSessionId(null);
}

function cancelAutoExecute(): void {
  if (autoExecuteTimer) {
    clearInterval(autoExecuteTimer);
    autoExecuteTimer = null;
  }
  setAutoExecuteSessionId(null);
  setAutoExecuteCountdown(0);
}

function clearMicErrorLater(): void {
  if (micErrorTimer) clearTimeout(micErrorTimer);
  micErrorTimer = setTimeout(() => {
    setMicError(null);
    micErrorTimer = null;
  }, 5000);
}

function clearMicErrorState(): void {
  if (micErrorTimer) {
    clearTimeout(micErrorTimer);
    micErrorTimer = null;
  }
  setMicError(null);
}

function clearLocalResources(): void {
  const activeRecorder = recorder;
  recorder = null;
  if (activeRecorder) {
    activeRecorder.onstop = null;
    activeRecorder.ondataavailable = null;
    activeRecorder.onerror = null;
    if (activeRecorder.state !== "inactive") activeRecorder.stop();
  }
  stopTracks(currentStream);
  currentStream = null;
  chunks = [];
  clearRecordingTimer();
  cancelAutoExecute();
  cancelTypingWarning();
  clearMicErrorState();
  setRecordingSessionId(null);
  setProcessingSessionId(null);
}

function clearBackendRecording(): void {
  const sessionId = backendRecordingSessionId;
  backendRecordingSessionId = null;
  if (!sessionId) return;
  void VoiceAPI.markRecording(sessionId, false).catch((error: unknown) => {
    console.error("[Voice] Failed to clear backend recording state:", error);
  });
}

function revokeCurrentLease(): void {
  leaseGeneration += 1;
  currentLease = null;
  clearBackendRecording();
  clearLocalResources();
}

function beginLease(sessionId: string): OperationLease {
  revokeCurrentLease();
  const lease = { sessionId, generation: leaseGeneration + 1 };
  leaseGeneration = lease.generation;
  currentLease = lease;
  return lease;
}

function startAudioLevelMonitor(stream: MediaStream, lease: OperationLease): void {
  let context: AudioContext | null = null;
  try {
    context = new AudioContext();
    audioContext = context;
    const nextAnalyser = context.createAnalyser();
    nextAnalyser.fftSize = 256;
    context.createMediaStreamSource(stream).connect(nextAnalyser);
    const samples = new Uint8Array(nextAnalyser.frequencyBinCount);
    analyser = nextAnalyser;
    levelTimer = setInterval(() => {
      if (!isCurrent(lease) || !analyser) return;
      analyser.getByteFrequencyData(samples);
      const sum = samples.reduce((total, sample) => total + sample, 0);
      setAudioLevel(sum / samples.length / 255);
    }, 50);
  } catch {
    // Audio-level visualization is optional; recording remains usable.
    if (audioContext === context) stopAudioLevelMonitor();
  }
}

function showTypingWarning(sessionId: string, lease: OperationLease): void {
  if (!isCurrent(lease)) return;
  cancelTypingWarning();
  setTypingWarnSessionId(sessionId);
  typingWarningTimer = setTimeout(() => {
    if (isCurrent(lease)) setTypingWarnSessionId(null);
    typingWarningTimer = null;
  }, 5000);
}

function startAutoExecuteCountdown(
  sessionId: string,
  delay: number,
  lease: OperationLease,
): void {
  if (!isCurrent(lease)) return;
  cancelAutoExecute();
  let remaining = delay;
  setAutoExecuteSessionId(sessionId);
  setAutoExecuteCountdown(remaining);
  autoExecuteTimer = setInterval(() => {
    if (!isCurrent(lease)) {
      cancelAutoExecute();
      return;
    }
    remaining -= 1;
    setAutoExecuteCountdown(remaining);
    if (remaining > 0) return;
    cancelAutoExecute();
    if (!isCurrent(lease)) return;
    void PtyAPI.write(sessionId, new TextEncoder().encode("\r")).catch((error: unknown) => {
      if (isCurrent(lease)) console.error("[Voice] Auto-execute failed:", error);
    });
  }, 1000);
}

async function processStoppedRecording(
  lease: OperationLease,
  stream: MediaStream,
  recordedChunks: Blob[],
  mimeType: string,
): Promise<void> {
  stopTracks(stream);
  if (currentStream === stream) currentStream = null;
  clearRecordingTimer();
  setRecordingSessionId(null);
  recorder = null;

  if (backendRecordingSessionId === lease.sessionId) {
    backendRecordingSessionId = null;
    try {
      await VoiceAPI.markRecording(lease.sessionId, false);
    } catch (error) {
      console.error("[Voice] Failed to clear backend recording state:", error);
    }
  }
  if (!isCurrent(lease) || recordedChunks.length === 0) return;

  setProcessingSessionId(lease.sessionId);
  try {
    const buffer = await new Blob(recordedChunks, { type: mimeType }).arrayBuffer();
    if (!isCurrent(lease)) return;
    const text = await VoiceAPI.transcribe(Array.from(new Uint8Array(buffer)), mimeType);
    if (!isCurrent(lease) || !text) return;
    await PtyAPI.write(lease.sessionId, new TextEncoder().encode(text));
    if (!isCurrent(lease)) return;
    const hadTyping = await VoiceAPI.hadTyping(lease.sessionId);
    if (!isCurrent(lease)) return;
    if (hadTyping) {
      showTypingWarning(lease.sessionId, lease);
      return;
    }
    let settings: Awaited<ReturnType<typeof SettingsAPI.get>>;
    try {
      settings = await SettingsAPI.get();
    } catch {
      // Preserve the existing behavior: settings failure skips auto-execute.
      return;
    }
    if (!isCurrent(lease)) return;
    if (settings.voiceAutoExecute) {
      startAutoExecuteCountdown(
        lease.sessionId,
        settings.voiceAutoExecuteDelay || 15,
        lease,
      );
    }
  } catch (error) {
    if (!isCurrent(lease)) return;
    const message = describeError(error, "Transcription failed");
    console.error("[Voice] Transcription failed:", message);
    setMicError(message);
    void DebugAPI.saveLogs(getConsoleText()).catch((saveError: unknown) => {
      console.error("[Voice] Failed to save debug logs:", saveError);
    });
    clearMicErrorLater();
  } finally {
    if (isCurrent(lease)) setProcessingSessionId(null);
  }
}

async function start(sessionId: string): Promise<void> {
  const lease = beginLease(sessionId);
  setMicError(null);
  setRecordingSeconds(0);
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    if (!isCurrent(lease)) {
      stopTracks(stream);
      return;
    }

    const preferredMime = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
      ? "audio/webm;codecs=opus"
      : undefined;
    const nextRecorder = new MediaRecorder(
      stream,
      preferredMime ? { mimeType: preferredMime } : undefined,
    );
    const mimeType = nextRecorder.mimeType || "audio/webm";
    currentStream = stream;
    recorder = nextRecorder;
    chunks = [];

    nextRecorder.ondataavailable = (event) => {
      if (isCurrent(lease) && event.data.size > 0) chunks.push(event.data);
    };
    nextRecorder.onerror = (event) => {
      if (isCurrent(lease)) console.error("[Voice] MediaRecorder error:", event);
    };
    nextRecorder.onstop = () => {
      const recordedChunks = [...chunks];
      chunks = [];
      void processStoppedRecording(lease, stream, recordedChunks, mimeType);
    };

    await VoiceAPI.markRecording(sessionId, true);
    if (!isCurrent(lease)) {
      try {
        await VoiceAPI.markRecording(sessionId, false);
      } catch (error) {
        console.error("[Voice] Failed to compensate stale recording state:", error);
      }
      nextRecorder.onstop = null;
      stopTracks(stream);
      return;
    }
    backendRecordingSessionId = sessionId;
    setRecordingSessionId(sessionId);
    recordingTimer = setInterval(() => {
      if (isCurrent(lease)) setRecordingSeconds((seconds) => seconds + 1);
    }, 1000);
    startAudioLevelMonitor(stream, lease);
    nextRecorder.start();
  } catch (error) {
    if (!isCurrent(lease)) return;
    const message = describeError(error, "Microphone access failed");
    console.error("[Voice] Microphone access failed:", message, error);
    revokeCurrentLease();
    setMicError(message);
    void DebugAPI.saveLogs(getConsoleText()).catch((saveError: unknown) => {
      console.error("[Voice] Failed to save debug logs:", saveError);
    });
    clearMicErrorLater();
  }
}

function stop(): void {
  if (recorder && recorder.state !== "inactive") recorder.stop();
}

function cancel(): void {
  revokeCurrentLease();
}

function toggle(sessionId: string): void {
  if (processingSessionId()) return;
  if (recordingSessionId()) stop();
  else void start(sessionId);
}

function revokeSession(sessionId: string): void {
  const ownsState =
    currentLease?.sessionId === sessionId ||
    recordingSessionId() === sessionId ||
    processingSessionId() === sessionId ||
    autoExecuteSessionId() === sessionId ||
    typingWarnSessionId() === sessionId;
  if (ownsState) revokeCurrentLease();
}

function revokeLiveBinding(): void {
  revokeCurrentLease();
}

export function formatRecordingTime(seconds: number): string {
  const minutes = Math.floor(seconds / 60);
  const remainder = seconds % 60;
  return `${minutes}:${remainder.toString().padStart(2, "0")}`;
}

export const voiceRecorder = {
  recordingSessionId,
  processingSessionId,
  micError,
  recordingSeconds,
  audioLevel,
  autoExecuteSessionId,
  autoExecuteCountdown,
  typingWarnSessionId,
  start,
  stop,
  cancel,
  toggle,
  cancelAutoExecute,
  revokeSession,
  revokeLiveBinding,
};
