// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { __setTransportForTests } from "./ipc";
import { FakeTransport } from "./testing/fake-transport";
import { baseSettings } from "./testing/ui-harness";
import { SESSION_A } from "./testing/session-selection";
import { voiceRecorder } from "./voice-recorder";

function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
  let resolve = (_value: T): void => undefined;
  const promise = new Promise<T>((nextResolve) => {
    resolve = nextResolve;
  });
  return { promise, resolve };
}

class FakeTrack {
  readonly stop = vi.fn();
}

class FakeStream {
  readonly track = new FakeTrack();
  getTracks(): FakeTrack[] {
    return [this.track];
  }
}

class FakeBlobEvent extends Event implements BlobEvent {
  readonly data: Blob;
  readonly timecode = 0;

  constructor(data: Blob) {
    super("dataavailable");
    this.data = data;
  }
}

class FakeMediaRecorder {
  static readonly instances: FakeMediaRecorder[] = [];
  static isTypeSupported(): boolean {
    return true;
  }

  state: RecordingState = "inactive";
  readonly mimeType = "audio/webm;codecs=opus";
  ondataavailable: ((event: BlobEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  onstop: ((event: Event) => void) | null = null;

  constructor(_stream: MediaStream, _options?: MediaRecorderOptions) {
    FakeMediaRecorder.instances.push(this);
  }

  start(): void {
    this.state = "recording";
  }

  stop(): void {
    if (this.state === "inactive") return;
    this.state = "inactive";
    this.onstop?.(new Event("stop"));
  }

  emitData(blob: Blob): void {
    this.ondataavailable?.(new FakeBlobEvent(blob));
  }
}

function setupTransport(fake: FakeTransport): void {
  fake.resolve("voice_mark_recording", undefined);
  fake.resolve("voice_transcribe", "hello");
  fake.resolve("voice_had_typing", false);
  fake.resolve("pty_write", undefined);
  fake.resolve("get_settings", baseSettings({ voiceAutoExecute: false }));
  fake.resolve("save_debug_logs", undefined);
}

describe("voiceRecorder revocable leases", () => {
  let restoreTransport: (() => void) | null = null;
  let mediaDevicesDescriptor: PropertyDescriptor | undefined;
  let blobArrayBufferDescriptor: PropertyDescriptor | undefined;

  beforeEach(() => {
    voiceRecorder.revokeLiveBinding();
    FakeMediaRecorder.instances.length = 0;
    vi.stubGlobal("MediaRecorder", FakeMediaRecorder);
    vi.stubGlobal("AudioContext", class {
      close(): Promise<void> { return Promise.resolve(); }
      createAnalyser() {
        return {
          fftSize: 0,
          frequencyBinCount: 1,
          getByteFrequencyData: (_data: Uint8Array) => undefined,
        };
      }
      createMediaStreamSource() {
        return { connect: (_target: unknown) => undefined };
      }
    });
    mediaDevicesDescriptor = Object.getOwnPropertyDescriptor(navigator, "mediaDevices");
    blobArrayBufferDescriptor = Object.getOwnPropertyDescriptor(Blob.prototype, "arrayBuffer");
    Object.defineProperty(Blob.prototype, "arrayBuffer", {
      configurable: true,
      value: () => Promise.resolve(new Uint8Array([1]).buffer),
    });
  });

  afterEach(() => {
    voiceRecorder.revokeLiveBinding();
    restoreTransport?.();
    restoreTransport = null;
    if (mediaDevicesDescriptor) {
      Object.defineProperty(navigator, "mediaDevices", mediaDevicesDescriptor);
    } else {
      Reflect.deleteProperty(navigator, "mediaDevices");
    }
    if (blobArrayBufferDescriptor) {
      Object.defineProperty(Blob.prototype, "arrayBuffer", blobArrayBufferDescriptor);
    } else {
      Reflect.deleteProperty(Blob.prototype, "arrayBuffer");
    }
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
    vi.useRealTimers();
  });

  it("revokes while getUserMedia is pending and stops the late stream", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    restoreTransport = __setTransportForTests(fake);
    const media = deferred<FakeStream>();
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: { getUserMedia: () => media.promise },
    });
    const stream = new FakeStream();
    const started = voiceRecorder.start(SESSION_A);
    voiceRecorder.revokeSession(SESSION_A);
    media.resolve(stream);
    await started;
    expect(stream.track.stop).toHaveBeenCalledOnce();
    expect(fake.callsFor("voice_mark_recording")).toHaveLength(0);
    expect(fake.callsFor("pty_write")).toHaveLength(0);
  });

  it("revokes a pending transcription before any text or Enter write", async () => {
    const fake = new FakeTransport();
    setupTransport(fake);
    const transcription = deferred<string>();
    fake.onInvoke("voice_transcribe", () => transcription.promise);
    restoreTransport = __setTransportForTests(fake);
    const stream = new FakeStream();
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: { getUserMedia: () => Promise.resolve(stream) },
    });
    await voiceRecorder.start(SESSION_A);
    const recorder = FakeMediaRecorder.instances[0];
    recorder.emitData(new Blob(["audio"]));
    voiceRecorder.stop();
    await vi.waitFor(() => expect(fake.callsFor("voice_transcribe")).toHaveLength(1));
    voiceRecorder.revokeSession(SESSION_A);
    transcription.resolve("late text");
    await Promise.resolve();
    await Promise.resolve();
    expect(fake.callsFor("pty_write")).toHaveLength(0);
    expect(stream.track.stop).toHaveBeenCalled();
  });

  it("revokes while typing detection is pending and never schedules Enter", async () => {
    vi.useFakeTimers();
    const fake = new FakeTransport();
    setupTransport(fake);
    const typing = deferred<boolean>();
    fake.onInvoke("voice_had_typing", () => typing.promise);
    fake.resolve("get_settings", baseSettings({ voiceAutoExecute: true, voiceAutoExecuteDelay: 1 }));
    restoreTransport = __setTransportForTests(fake);
    const stream = new FakeStream();
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: { getUserMedia: () => Promise.resolve(stream) },
    });
    await voiceRecorder.start(SESSION_A);
    FakeMediaRecorder.instances[0].emitData(new Blob(["audio"]));
    voiceRecorder.stop();
    await vi.waitFor(() => expect(fake.callsFor("voice_had_typing")).toHaveLength(1));
    const writesBeforeRevoke = fake.callsFor("pty_write").length;
    voiceRecorder.revokeLiveBinding();
    typing.resolve(false);
    await Promise.resolve();
    await vi.advanceTimersByTimeAsync(2_000);
    expect(fake.callsFor("pty_write")).toHaveLength(writesBeforeRevoke);
    vi.useRealTimers();
  });

  it("revokes while settings lookup is pending and prevents auto-execute", async () => {
    vi.useFakeTimers();
    const fake = new FakeTransport();
    setupTransport(fake);
    const settings = deferred<ReturnType<typeof baseSettings>>();
    fake.onInvoke("get_settings", () => settings.promise);
    restoreTransport = __setTransportForTests(fake);
    const stream = new FakeStream();
    Object.defineProperty(navigator, "mediaDevices", {
      configurable: true,
      value: { getUserMedia: () => Promise.resolve(stream) },
    });
    await voiceRecorder.start(SESSION_A);
    FakeMediaRecorder.instances[0].emitData(new Blob(["audio"]));
    voiceRecorder.stop();
    await vi.waitFor(() => expect(fake.callsFor("get_settings")).toHaveLength(1));
    const writesBeforeRevoke = fake.callsFor("pty_write").length;
    voiceRecorder.revokeSession(SESSION_A);
    settings.resolve(baseSettings({ voiceAutoExecute: true, voiceAutoExecuteDelay: 1 }));
    await Promise.resolve();
    await vi.advanceTimersByTimeAsync(2_000);
    expect(fake.callsFor("pty_write")).toHaveLength(writesBeforeRevoke);
    vi.useRealTimers();
  });
});
