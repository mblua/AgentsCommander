// #714 Lightweight frontend pre-validation of the screenshot-capture hotkey.
//
// This MIRRORS the backend MVP parser (`crate::screenshot::parse_screenshot_hotkey`):
// exactly one Ctrl/Control modifier plus one alphanumeric key. The backend stays
// the single authority for what actually registers with the OS; this only gives
// the Settings form instant feedback so an obviously-malformed value is caught
// before save. Keep this regex a SUBSET of what the backend accepts — if the FE
// were to pass a value the backend cannot register, that surfaces later via the
// hotkey-status error toast, which is acceptable but noisier.
const SCREENSHOT_HOTKEY_RE = /^(ctrl|control)\+[a-z0-9]$/i;

/** Returns a human-readable error string if `raw` is not a valid screenshot
 *  hotkey, or `null` when it is acceptable. */
export function validateScreenshotHotkeySyntax(raw: string): string | null {
  const trimmed = raw.trim();
  if (!trimmed) return "Screenshot hotkey is required";
  if (!SCREENSHOT_HOTKEY_RE.test(trimmed)) {
    return "Screenshot hotkey must look like Ctrl+Q";
  }
  return null;
}
