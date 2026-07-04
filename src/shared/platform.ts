/// Runtime environment detection.
/// True when running inside a Tauri WebView, false in a plain browser.
export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const isBrowser = !isTauri;

/// #777 True when the host OS is Windows. Used to gate Windows-only features in
/// the UI (e.g. the Non-stop Sound Alert, whose backend beep is a Win32 no-op
/// off Windows). UA-based because the webview has no direct OS API; WebView2
/// (Windows), WKWebView (macOS), and WebKitGTK (Linux) all report the OS token.
export const isWindows =
  typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);
