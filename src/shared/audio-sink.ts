// #4843 - the sound sink IPC contract.
import type { Transport } from "./transport";

/** Mirrors `AudioSinkStatus` in `src-tauri/src/process.rs` (serde lowercase). */
export type AudioSinkStatus = "ok" | "missing" | "unknown";

/** #4843 - `get_audio_sink_status` over an injected transport (Tauri or WS). */
export function createAudioSinkAPI(transport: Pick<Transport, "invoke">) {
  return {
    getStatus: () => transport.invoke<AudioSinkStatus>("get_audio_sink_status"),
  };
}
