import { createSignal } from "solid-js";
import { VoiceAPI, PtyAPI, DebugAPI, SettingsAPI } from "./ipc";
import { getConsoleText } from "./console-capture";

const [recordingSessionId, setRecordingSessionId] = createSignal<string | null>(null);
const [processingSessionId, setProcessingSessionId] = createSignal<string | null>(null);
const [micError, setMicError] = createSignal<string | null>(null);
const [recordingSeconds, setRecordingSeconds] = createSignal(0);
const [audioLevel, setAudioLevel] = createSignal(0);
const [autoExecuteSessionId, setAutoExecuteSessionId] = createSignal<string | null>(null);
const [autoExecuteCountdown, setAutoExecuteCountdown] = createSignal(0);
const [typingWarnSessionId, setTypingWarnSessionId] = createSignal<string | null>(null);

let recorder: MediaRecorder | null = null;
let currentStream: MediaStream | null = null;
let audioCtx: AudioContext | null = null;
let analyser: AnalyserNode | null = null;
let chunks: Blob[] = [];
let mimeType = "";
let recordingTimer: ReturnType<typeof setInterval> | null = null;
let levelTimer: ReturnType<typeof setInterval> | null = null;
let autoExecTimer: ReturnType<typeof setInterval> | null = null;
let typingWarnTimer: ReturnType<typeof setTimeout> | null = null;

function startAudioLevelMonitor(stream: MediaStream) {
  try {
    audioCtx = new AudioContext();
    analyser = audioCtx.createAnalyser();
    analyser.fftSize = 256;
    const source = audioCtx.createMediaStreamSource(stream);
    source.connect(analyser);
    const dataArray = new Uint8Array(analyser.frequencyBinCount);

    levelTimer = setInterval(() => {
      if (!analyser) return;
      analyser.getByteFrequencyData(dataArray);
      const sum = dataArray.reduce((a, b) => a + b, 0);
      const avg = sum / dataArray.length / 255;
      setAudioLevel(avg);
    }, 50);
  } catch {
  }
}

function stopAudioLevelMonitor() {
  if (levelTimer) {
    clearInterval(levelTimer);
    levelTimer = null;
  }
  if (audioCtx) {
    audioCtx.close().catch(() => {});
    audioCtx = null;
    analyser = null;
  }
  setAudioLevel(0);
}

function clearTimers() {
  if (recordingTimer) {
    clearInterval(recordingTimer);
    recordingTimer = null;
  }
  stopAudioLevelMonitor();
}

function cleanupRecording() {
  if (currentStream) {
    currentStream.getTracks().forEach((t) => t.stop());
    currentStream = null;
  }
  clearTimers();
  setRecordingSessionId(null);
  recorder = null;
  chunks = [];
}

async function start(sessionId: string) {
  cancelAutoExecute();
  cancelTypingWarning();

  if (recordingSessionId()) {
    stop();
    await new Promise((r) => setTimeout(r, 50));
  }

  setMicError(null);
  setRecordingSeconds(0);

  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    currentStream = stream;
    chunks = [];

    const preferredMime = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
      ? "audio/webm;codecs=opus"
      : undefined;
    const rec = new MediaRecorder(stream, preferredMime ? { mimeType: preferredMime } : undefined);
    recorder = rec;
    mimeType = rec.mimeType || "audio/webm";
    setRecordingSessionId(sessionId);

    recordingTimer = setInterval(() => {
      setRecordingSeconds((s) => s + 1);
    }, 1000);

    startAudioLevelMonitor(stream);

    rec.ondataavailable = (e) => {
      if (e.data.size > 0) chunks.push(e.data);
    };

    rec.onerror = (e) => {
      console.error("[Voice] MediaRecorder error:", e);
    };

    rec.onstop = async () => {
      const stoppedSessionId = recordingSessionId();

      if (stoppedSessionId) await VoiceAPI.markRecording(stoppedSessionId, false);

      stream.getTracks().forEach((t) => t.stop());
      currentStream = null;
      clearTimers();
      setRecordingSessionId(null);
      recorder = null;

      if (chunks.length === 0 || !stoppedSessionId) return;

      setProcessingSessionId(stoppedSessionId);

      const blob = new Blob(chunks, { type: mimeType });
      const buffer = await blob.arrayBuffer();
      const bytes = Array.from(new Uint8Array(buffer));

      try {
        const text = await VoiceAPI.transcribe(bytes, mimeType);
        if (text) {
          const encoder = new TextEncoder();
          await PtyAPI.write(stoppedSessionId, encoder.encode(text));

          const hadTyping = await VoiceAPI.hadTyping(stoppedSessionId);
          if (hadTyping) {
            showTypingWarning(stoppedSessionId);
          } else {
            try {
              const settings = await SettingsAPI.get();
              if (settings.voiceAutoExecute) {
                const delay = settings.voiceAutoExecuteDelay || 15;
                startAutoExecuteCountdown(stoppedSessionId, delay);
              }
            } catch {
            }
          }
        }
      } catch (err: any) {
        const msg = typeof err === "string" ? err : err?.message || "Transcription failed";
        console.error("[Voice] Transcription failed:", msg);
        setMicError(msg);
        DebugAPI.saveLogs(getConsoleText()).catch(() => {});
        setTimeout(() => setMicError(null), 5000);
      } finally {
        setProcessingSessionId(null);
      }
    };

    void VoiceAPI.markRecording(sessionId, true);
    rec.start();
  } catch (err: any) {
    const msg = err?.message || err?.name || "Microphone access failed";
    console.error("[Voice] Microphone access failed:", msg, err);
    setMicError(msg);
    DebugAPI.saveLogs(getConsoleText()).catch(() => {});
    setRecordingSessionId(null);
    recorder = null;
    currentStream = null;
    clearTimers();
    setTimeout(() => setMicError(null), 5000);
  }
}

function stop() {
  if (recorder && recorder.state !== "inactive") {
    recorder.stop();
  }
}

function cancel() {
  const sid = recordingSessionId();
  cancelAutoExecute();
  cancelTypingWarning();
  if (recorder) {
    recorder.onstop = null;
    if (recorder.state !== "inactive") {
      recorder.stop();
    }
  }
  if (sid) void VoiceAPI.markRecording(sid, false);
  cleanupRecording();
}

function toggle(sessionId: string) {
  if (processingSessionId()) return;
  if (recordingSessionId()) {
    stop();
  } else {
    void start(sessionId);
  }
}

function startAutoExecuteCountdown(sessionId: string, delay: number) {
  cancelAutoExecute();
  let remaining = delay;
  setAutoExecuteSessionId(sessionId);
  setAutoExecuteCountdown(remaining);

  autoExecTimer = setInterval(async () => {
    remaining--;
    setAutoExecuteCountdown(remaining);
    if (remaining <= 0) {
      clearInterval(autoExecTimer!);
      autoExecTimer = null;
      setAutoExecuteSessionId(null);
      setAutoExecuteCountdown(0);
      try {
        await PtyAPI.write(sessionId, new TextEncoder().encode("\r"));
      } catch (err) {
        console.error("[Voice] Auto-execute failed:", err);
      }
    }
  }, 1000);
}

function showTypingWarning(sessionId: string) {
  cancelTypingWarning();
  setTypingWarnSessionId(sessionId);
  typingWarnTimer = setTimeout(() => {
    setTypingWarnSessionId(null);
    typingWarnTimer = null;
  }, 5000);
}

function cancelTypingWarning() {
  if (typingWarnTimer) {
    clearTimeout(typingWarnTimer);
    typingWarnTimer = null;
  }
  setTypingWarnSessionId(null);
}

function cancelAutoExecute() {
  if (autoExecTimer) {
    clearInterval(autoExecTimer);
    autoExecTimer = null;
  }
  setAutoExecuteSessionId(null);
  setAutoExecuteCountdown(0);
}

export function formatRecordingTime(s: number): string {
  const m = Math.floor(s / 60);
  const sec = s % 60;
  return `${m}:${sec.toString().padStart(2, "0")}`;
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
};
