# Voice-to-text setup

For developers who want to dictate prompts to a session instead of typing. Setup is a one-time API-key + checkbox; afterwards you just hold a button.

For what the feature does and trade-offs, see [`docs/features/voice-to-text.md`](../features/voice-to-text.md).

## 1. Get a Gemini API key

1. Sign in at [aistudio.google.com](https://aistudio.google.com).
2. Click **Get API key** → **Create API key in new project** (or pick an existing project).
3. Copy the key (a long string starting with `AIza...`).

The key has a free quota; transcription beyond the quota is billed per Google's [Gemini API pricing](https://ai.google.dev/pricing).

## 2. Configure AC

In AgentsCommander:

1. Open **Settings → Integrations → Voice**.
2. Toggle **Voice-to-text** on.
3. Paste the Gemini API key into the **Gemini API key** field.
4. Pick a model:
   - **gemini-2.5-flash** (default): faster, cheaper, good for clean audio.
   - **gemini-1.5-pro**: slower, more accurate, useful for noisy or accented audio.
5. (Optional) Configure **Auto-execute**:
   - **Auto-execute**: when on, AC presses Enter after transcription so the agent receives the prompt without you switching to the keyboard.
   - **Delay before auto-execute**: seconds AC waits before pressing Enter (default 15s). Gives you time to read the transcription and cancel if wrong.
6. **Save**.

The key is stored locally in `settings.json` under `geminiApiKey`. Plaintext; protect your user account.

## 3. Try it

In any session:

1. Hold the **mic button** on the session item (sidebar) or in the terminal status bar.
2. Speak.
3. Release. AC sends the audio to Gemini.
4. The transcribed text appears in the PTY. If auto-execute is on, Enter fires after the configured delay.

Keyboard shortcut: **Ctrl+Shift+R** toggles voice recording on the active session.

## Cancelling

- **Mid-recording**: press Escape, or release before 500ms (very short clips are discarded automatically).
- **Mid-transcription** (after release, before AC writes the text): click the **Cancel** button that appears next to the mic indicator.
- **Before auto-execute**: edit or delete the transcribed text in the terminal before the delay elapses.

## Data flow

| Step | Where data goes |
|---|---|
| Audio capture | Local: your microphone via `MediaRecorder` |
| Transcription request | `generativelanguage.googleapis.com` (Gemini API) |
| Response | Back to AC; written to the local PTY |
| Storage | Audio is **not** persisted locally; only the transcribed text reaches the PTY |

For the canonical statement, see [`PRIVACY.md`](../../PRIVACY.md).

## Troubleshooting

| Symptom | Cause | Fix |
|---|---|---|
| Mic button is greyed out | Voice disabled or API key missing | Toggle on, paste a key, save |
| Transcription is wildly wrong | Background noise or accent mismatch | Switch model to `gemini-1.5-pro` |
| Transcription is empty | Recording <500ms or muted mic | Hold longer; check OS mic permissions |
| Browser-level mic permission prompt | First use on this machine | Allow microphone for the AC process |
| Auto-execute fires too fast | Delay too short | Settings → Integrations → Voice → Delay |

More: [`docs/troubleshooting.md#voice-to-text`](../troubleshooting.md#voice-to-text).

## See also

- [Voice-to-text: feature page](../features/voice-to-text.md)
- [Settings reference: voice fields](../reference/settings.md)
- [`PRIVACY.md`](../../PRIVACY.md): what leaves your machine
