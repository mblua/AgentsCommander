/// Runtime environment detection.
/// True when running inside a Tauri WebView, false in a plain browser.
export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const isBrowser = !isTauri;

/// #777 True when the host OS is Windows, read from the user agent because the
/// webview exposes no direct OS API. Used for platform-specific UI text, e.g.
/// the default-shell hint in src/sidebar/components/SettingsModal.tsx:1931.
export const isWindows =
  typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
