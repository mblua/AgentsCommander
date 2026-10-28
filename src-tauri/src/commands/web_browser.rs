//! Embedded web browser (#3367): a borderless window owned by `main`, laid
//! over the central terminal pane so it reads as part of the app.
//!
//! A separate owned window instead of a second webview inside `main` keeps
//! `main` a single-webview window: Tauri's `get_webview_window("main")`
//! returns `None` once a window hosts several webviews, and the app relies on
//! it in many places. The frontend owns visibility and reports the pane
//! bounds; this module positions the window, tracks the page URL and title,
//! and serves the agent-facing mailbox actions (see `web_browser/mailbox.rs`).

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::webview::{NewWindowResponse, PageLoadEvent};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Runtime, Url, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

mod mailbox;

pub(crate) use mailbox::{
    is_web_browser_action, WEB_BROWSER_NAVIGATE_ACTION, WEB_BROWSER_RELOAD_ACTION,
    WEB_BROWSER_STATUS_ACTION,
};

pub(crate) const WEB_BROWSER_WINDOW_LABEL: &str = "web-browser";
const MAIN_WINDOW_LABEL: &str = "main";
const DEFAULT_URL: &str = "about:blank";

/// Emitted to the frontend whenever the URL, title, loading or visibility
/// changes.
const STATE_EVENT: &str = "web_browser_state";
/// Emitted when an agent navigates while the browser view is not shown, so
/// the frontend brings it up over the terminal pane.
const OPEN_REQUESTED_EVENT: &str = "web_browser_open_requested";

/// Pane rectangle in physical pixels, relative to the main window client
/// area. The frontend multiplies CSS pixels by `devicePixelRatio`, which
/// already folds in both the monitor scale and the webview zoom.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct PaneBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebBrowserSnapshot {
    pub url: String,
    pub title: String,
    pub loading: bool,
    pub visible: bool,
}

#[derive(Default)]
struct BrowserModel {
    url: Option<String>,
    title: String,
    loading: bool,
    visible: bool,
    bounds: Option<PaneBounds>,
    main_listener: bool,
}

impl BrowserModel {
    fn snapshot(&self) -> WebBrowserSnapshot {
        WebBrowserSnapshot {
            url: self.url.clone().unwrap_or_else(|| DEFAULT_URL.to_string()),
            title: self.title.clone(),
            loading: self.loading,
            visible: self.visible,
        }
    }
}

/// Managed state, registered lazily by `state()`.
#[derive(Default)]
pub struct WebBrowserState(Mutex<BrowserModel>);

impl WebBrowserState {
    fn with<T>(&self, f: impl FnOnce(&mut BrowserModel) -> T) -> T {
        let mut model = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut model)
    }
}

/// Turns what a user or agent typed into a loadable URL. Accepts http, https,
/// file and about URLs, absolute file paths, and bare hosts (`example.com`,
/// `localhost:3000`). Anything else, `javascript:` included, is rejected.
pub(crate) fn normalize_url(input: &str) -> Result<Url, String> {
    let input = input.trim();
    if input.is_empty() {
        return Err("url is empty".to_string());
    }
    // Before `Url::parse`, which reads `C:\x` as scheme `c`.
    let path = std::path::Path::new(input);
    if path.is_absolute() {
        return Url::from_file_path(path).map_err(|_| format!("invalid file path: {input}"));
    }
    if let Ok(url) = Url::parse(input) {
        if matches!(url.scheme(), "http" | "https" | "file" | "about") {
            return Ok(url);
        }
    }
    if input.contains(char::is_whitespace) || input.contains("://") {
        return Err(format!("unsupported url: {input}"));
    }
    let host = input.split(['/', ':', '?', '#']).next().unwrap_or_default();
    let scheme = if host == "localhost" || host == "127.0.0.1" {
        "http"
    } else if host.contains('.') {
        "https"
    } else {
        return Err(format!("unsupported url: {input}"));
    };
    Url::parse(&format!("{scheme}://{input}")).map_err(|_| format!("unsupported url: {input}"))
}

/// Physical screen rectangle of the pane given the main client-area origin.
pub(crate) fn screen_rect(origin: (i32, i32), bounds: PaneBounds) -> (i32, i32, u32, u32) {
    let x = origin.0 + bounds.x.round() as i32;
    let y = origin.1 + bounds.y.round() as i32;
    let width = bounds.width.round().max(1.0) as u32;
    let height = bounds.height.round().max(1.0) as u32;
    (x, y, width, height)
}

/// Keeps back/forward to one step per click; the page sees `history.go`.
pub(crate) fn history_script(delta: i32) -> String {
    format!("history.go({})", delta.clamp(-1, 1))
}

/// Registered on first use; `manage` is a no-op when another caller won.
fn state<R: Runtime>(app: &AppHandle<R>) -> tauri::State<'_, WebBrowserState> {
    if app.try_state::<WebBrowserState>().is_none() {
        app.manage(WebBrowserState::default());
    }
    app.state::<WebBrowserState>()
}

fn publish<R: Runtime>(app: &AppHandle<R>) -> WebBrowserSnapshot {
    let snapshot = state(app).with(|model| model.snapshot());
    let _ = app.emit(STATE_EVENT, snapshot.clone());
    snapshot
}

fn browser_window<R: Runtime>(app: &AppHandle<R>) -> Option<WebviewWindow<R>> {
    app.get_webview_window(WEB_BROWSER_WINDOW_LABEL)
}

fn place<R: Runtime>(app: &AppHandle<R>, window: &WebviewWindow<R>) -> Result<(), String> {
    let Some(bounds) = state(app).with(|model| model.bounds) else {
        return Ok(());
    };
    let main = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window is not available".to_string())?;
    let origin = main.inner_position().map_err(|e| e.to_string())?;
    let (x, y, width, height) = screen_rect((origin.x, origin.y), bounds);
    window
        .set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    window
        .set_size(PhysicalSize::new(width, height))
        .map_err(|e| e.to_string())
}

/// Follows the main window: the owned browser window does not move with it.
fn follow_main<R: Runtime>(app: &AppHandle<R>, main: &WebviewWindow<R>) {
    if state(app).with(|model| std::mem::replace(&mut model.main_listener, true)) {
        return;
    }
    let app = app.clone();
    main.on_window_event(move |event| {
        let relevant = matches!(
            event,
            WindowEvent::Moved(_)
                | WindowEvent::Resized(_)
                | WindowEvent::ScaleFactorChanged { .. }
        );
        if !relevant || !state(&app).with(|model| model.visible) {
            return;
        }
        if let Some(window) = browser_window(&app) {
            if let Err(e) = place(&app, &window) {
                log::warn!("[web-browser] reposition failed: {e}");
            }
        }
    });
}

fn record_page_load<R: Runtime>(app: &AppHandle<R>, url: &Url, event: PageLoadEvent) {
    state(app).with(|model| {
        model.url = Some(url.to_string());
        model.loading = matches!(event, PageLoadEvent::Started);
    });
    publish(app);
}

fn record_title<R: Runtime>(app: &AppHandle<R>, title: String) {
    state(app).with(|model| model.title = title);
    publish(app);
}

fn create_window<R: Runtime>(app: &AppHandle<R>) -> Result<WebviewWindow<R>, String> {
    let main = app
        .get_webview_window(MAIN_WINDOW_LABEL)
        .ok_or_else(|| "Main window is not available".to_string())?;
    let url = state(app).with(|model| model.url.clone());
    let url = normalize_url(url.as_deref().unwrap_or(DEFAULT_URL))?;
    let (load_app, title_app, popup_app) = (app.clone(), app.clone(), app.clone());
    let window =
        WebviewWindowBuilder::new(app, WEB_BROWSER_WINDOW_LABEL, WebviewUrl::External(url))
            .title("Web Browser")
            .parent(&main)
            .map_err(|e| e.to_string())?
            .decorations(false)
            .shadow(false)
            .resizable(false)
            .skip_taskbar(true)
            .focused(false)
            .visible(false)
            .on_page_load(move |_, payload| {
                record_page_load(&load_app, payload.url(), payload.event())
            })
            .on_document_title_changed(move |_, title| record_title(&title_app, title))
            // Links that open a new window load in place: one browser, one page.
            .on_new_window(move |url, _| {
                if let Some(window) = browser_window(&popup_app) {
                    let _ = window.navigate(url);
                }
                NewWindowResponse::Deny
            })
            .build()
            .map_err(|e| e.to_string())?;
    follow_main(app, &main);
    Ok(window)
}

/// Sets the URL from a user or agent. Loads it when the window exists;
/// otherwise the next `web_browser_show` opens it.
pub(crate) fn navigate_to<R: Runtime>(
    app: &AppHandle<R>,
    input: &str,
) -> Result<WebBrowserSnapshot, String> {
    let url = normalize_url(input)?;
    state(app).with(|model| {
        model.url = Some(url.to_string());
        model.title.clear();
    });
    if let Some(window) = browser_window(app) {
        window.navigate(url).map_err(|e| e.to_string())?;
    }
    Ok(publish(app))
}

/// Agent navigation: like `navigate_to`, and asks the frontend to show the
/// browser view when it is hidden.
pub(crate) fn agent_navigate<R: Runtime>(
    app: &AppHandle<R>,
    input: &str,
) -> Result<WebBrowserSnapshot, String> {
    let snapshot = navigate_to(app, input)?;
    if !snapshot.visible {
        let _ = app.emit(OPEN_REQUESTED_EVENT, snapshot.clone());
    }
    Ok(snapshot)
}

/// Reloads the current page. Errors when the browser has never been opened.
pub(crate) fn reload<R: Runtime>(app: &AppHandle<R>) -> Result<WebBrowserSnapshot, String> {
    let window = browser_window(app).ok_or_else(|| "web browser is not open".to_string())?;
    window.reload().map_err(|e| e.to_string())?;
    Ok(state(app).with(|model| model.snapshot()))
}

pub(crate) fn snapshot<R: Runtime>(app: &AppHandle<R>) -> WebBrowserSnapshot {
    state(app).with(|model| model.snapshot())
}

/// Shows the browser over the pane at `bounds`, creating it on first use.
/// Called again on every pane resize.
#[tauri::command]
pub async fn web_browser_show(
    app: AppHandle,
    bounds: PaneBounds,
) -> Result<WebBrowserSnapshot, String> {
    let was_visible = state(&app).with(|model| {
        model.bounds = Some(bounds);
        std::mem::replace(&mut model.visible, true)
    });
    let window = match browser_window(&app) {
        Some(window) => window,
        None => create_window(&app)?,
    };
    place(&app, &window)?;
    if !was_visible {
        window.show().map_err(|e| e.to_string())?;
    }
    Ok(publish(&app))
}

/// Hides the browser window; the page stays loaded for the next show.
#[tauri::command]
pub fn web_browser_hide(app: AppHandle) -> Result<WebBrowserSnapshot, String> {
    state(&app).with(|model| model.visible = false);
    if let Some(window) = browser_window(&app) {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(publish(&app))
}

#[tauri::command]
pub fn web_browser_navigate(app: AppHandle, url: String) -> Result<WebBrowserSnapshot, String> {
    navigate_to(&app, &url)
}

/// `delta` -1 goes back, 1 goes forward.
#[tauri::command]
pub fn web_browser_go(app: AppHandle, delta: i32) -> Result<(), String> {
    let window = browser_window(&app).ok_or_else(|| "web browser is not open".to_string())?;
    window
        .eval(history_script(delta))
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn web_browser_reload(app: AppHandle) -> Result<WebBrowserSnapshot, String> {
    reload(&app)
}

#[tauri::command]
pub fn web_browser_snapshot(app: AppHandle) -> WebBrowserSnapshot {
    snapshot(&app)
}

#[cfg(test)]
mod tests;
