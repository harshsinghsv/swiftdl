//! Small per-download windows, like IDM's: "New download" when the browser
//! hands over a file, then live progress, then "Download complete".

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Frameless windows on Windows 11 get a 1px light border from the system,
/// which looks like a white frame in dark mode. Ask for no border and
/// rounded corners; the shadow stays.
pub fn polish(window: &WebviewWindow) {
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        #[link(name = "dwmapi")]
        unsafe extern "system" {
            fn DwmSetWindowAttribute(hwnd: *mut c_void, attr: u32, value: *const c_void, size: u32) -> i32;
        }
        const DWMWA_WINDOW_CORNER_PREFERENCE: u32 = 33;
        const DWMWA_BORDER_COLOR: u32 = 34;
        const DWMWCP_ROUND: u32 = 2;
        const DWMWA_COLOR_NONE: u32 = 0xFFFF_FFFE;
        if let Ok(hwnd) = window.hwnd() {
            unsafe {
                DwmSetWindowAttribute(hwnd.0, DWMWA_WINDOW_CORNER_PREFERENCE, (&DWMWCP_ROUND as *const u32).cast(), 4);
                DwmSetWindowAttribute(hwnd.0, DWMWA_BORDER_COLOR, (&DWMWA_COLOR_NONE as *const u32).cast(), 4);
            }
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

/// Card width plus the transparent margin the page uses for its shadow.
const WINDOW_WIDTH: f64 = 520.0 + 2.0 * 16.0;

/// Puts a window in front of whatever the user is doing, like IDM's dialogs.
/// Windows only lets a background app flash its taskbar button, but a
/// window that is briefly "always on top" still appears above everything;
/// it then drops back to a normal window, staying in front.
fn bring_to_front(w: &WebviewWindow) {
    let _ = w.unminimize();
    let _ = w.show();
    let _ = w.set_always_on_top(true);
    let _ = w.set_focus();
    let w = w.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(600));
        let _ = w.set_focus();
        let _ = w.set_always_on_top(false);
    });
}

/// Opens `download.html?{query}` in a window called `label`, or brings an
/// existing one forward.
pub fn open(app: &AppHandle, label: &str, query: &str, title: &str) -> tauri::Result<()> {
    if let Some(w) = app.get_webview_window(label) {
        bring_to_front(&w);
        return Ok(());
    }
    // Transparent and frameless: the page draws the whole window (rounded
    // card and shadow), so no native frame, border or unpainted edge shows.
    WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("download.html?{query}").into()))
        .title(title)
        .inner_size(WINDOW_WIDTH, 360.0)
        .resizable(false)
        .maximizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .center()
        .focused(true)
        .always_on_top(true)
        .build()
        .map(|w| bring_to_front(&w))?;
    Ok(())
}
