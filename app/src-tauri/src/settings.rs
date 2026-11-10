use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Empty means "the system Downloads folder", filled in at startup.
    pub download_dir: PathBuf,
    /// Connections per download.
    pub connections: usize,
    /// Downloads running at the same time; the rest wait in the queue.
    pub max_concurrent: usize,
    /// Global limit across all downloads, KiB/s. 0 = unlimited.
    pub speed_limit_kib: u64,
    /// Save into Downloads\Video, Downloads\Music... by file type.
    pub sort_into_categories: bool,
    /// Show the Add dialog for downloads caught from the browser.
    pub ask_before_download: bool,
    pub notify_on_complete: bool,
    /// Ask installed browsers to offer the extension (they prompt once).
    pub offer_extension: bool,
    /// The user closed the "connect your browser" card.
    pub onboarding_dismissed: bool,
    /// Open a small progress window for each download as it starts.
    pub show_progress_window: bool,
    /// Closing the main window keeps SwiftDL running in the tray.
    pub close_to_tray: bool,
    /// "system", "light" or "dark".
    pub theme: String,
    /// After a torrent finishes: "stop", a share ratio such as "1" or "2",
    /// or "forever".
    pub seed_until: String,
    /// Once all downloads are done: "nothing", "close", "sleep" or "shutdown".
    /// Used once, then back to "nothing".
    pub when_done: String,
    /// Hold the queue until this time (ms since epoch). 0 = no schedule.
    pub start_at: u64,
    /// Proxy for downloads, e.g. "http://127.0.0.1:8080". Empty follows the
    /// system's. Downloads use a change after SwiftDL restarts.
    pub proxy: String,
    /// When switching to SwiftDL, offer to download a link on the clipboard.
    pub watch_clipboard: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            download_dir: PathBuf::new(),
            connections: 8,
            max_concurrent: 3,
            speed_limit_kib: 0,
            sort_into_categories: true,
            ask_before_download: true,
            notify_on_complete: true,
            offer_extension: true,
            onboarding_dismissed: false,
            show_progress_window: true,
            close_to_tray: true,
            theme: "system".into(),
            seed_until: "stop".into(),
            when_done: "nothing".into(),
            start_at: 0,
            proxy: String::new(),
            watch_clipboard: false,
        }
    }
}
