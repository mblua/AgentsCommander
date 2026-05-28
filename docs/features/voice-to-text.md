# Voice-to-text

For developers who want to dictate prompts to a coding agent instead of typing — useful for long prompts or hands-busy moments.

For end-to-end setup (getting a Gemini API key, configuring AC, picking a model) see [`docs/integrations/voice.md`](../integrations/voice.md). This page describes how the feature works and what to expect.

## How it works

1. Hold the **mic button** on a session item (sidebar) or in the terminal status bar.
2. AC starts a `MediaRecorder` capturing audio (WebM/Opus).
3. Release the button. AC sends the audio blob to the Google Gemini API.
4. The transcribed text is written directly into the session's PTY.
5. If **auto-execute** is on, AC presses Enter after a short configurable delay so the agent receives the prompt without you switching to the keyboard.

You can cancel a recording mid-press with Escape, or before transcription completes with the **Cancel** button that appears.

## When to use it

- **Long prompts.** Dictating two paragraphs is faster than typing them.
- **Hands-busy work.** Coding agent running, you reading a doc on another monitor, voice the next instruction.
- **Accessibility.** Removes the typing requirement for prompt entry.

## When not to use it

- **Privacy-sensitive content.** Audio is sent to the Google Gemini API. See [`PRIVACY.md`](../../PRIVACY.md).
- **Noisy environments.** Background noise tanks accuracy.
- **Languages other than the model's strengths.** Gemini's transcription accuracy varies by language; try the larger `gemini-1.5-pro` model if the default is weak in yours.

## Configuration

| Setting | Default | What it does |
|---|---|---|
| `voiceToTextEnabled` | `false` | Master switch. Must be true for the mic button to be clickable. |
| `geminiApiKey` | empty | Your Gemini API key. Without it, the button stays disabled. |
| `geminiModel` | `gemini-2.5-flash` | The transcription model. `gemini-1.5-pro` is more accurate but slower and more expensive. |
| `voiceAutoExecute` | `true` | Press Enter after transcription. |
| `voiceAutoExecuteDelay` | `15` (seconds) | Delay before pressing Enter, so you can review or cancel. |

All values live in `settings.json` under the keys above. See [Settings reference](../reference/settings.md).

## Keyboard shortcut

Press **Ctrl+Shift+R** anywhere in the app to toggle voice recording on the active session. Holding it works the same as holding the mic button.

## What gets sent over the wire

Only the audio blob plus your model + API key go to `generativelanguage.googleapis.com`. No PTY output, no session metadata, no AC version. Transcribed text comes back; AC writes it locally and the round-trip ends.

The Gemini API's privacy policy applies to the audio AC transmits — see [Google's terms](https://policies.google.com/privacy).

## Cost

Voice transcription uses the Gemini Files / Generate API. Pricing depends on the model and audio length — Google publishes current rates on their developer pricing page. Switching from `gemini-2.5-flash` to `gemini-1.5-pro` typically costs more per second of audio.

## Failure modes

| Symptom | Cause | Fix |
|---|---|---|
| Mic button is greyed out | `voiceToTextEnabled: false` or missing API key | Set both under Settings → Integrations → Voice |
| Transcription returns garbled text | Background noise or accent mismatch | Switch to `gemini-1.5-pro` |
| Transcription returns empty string | Recording was <500ms or mic is muted | Hold longer; check OS microphone permissions for the AC process |
| Audio prompt for browser-level permission | First use on this machine | Allow microphone access for the AC process |

More cases: [`docs/troubleshooting.md#voice-to-text`](../troubleshooting.md#voice-to-text).

## See also

- [Voice setup](../integrations/voice.md) — get a Gemini key and wire it in
- [`PRIVACY.md`](../../PRIVACY.md) — what leaves your machine and when
