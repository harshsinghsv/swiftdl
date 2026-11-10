//! Functions the UI calls through `invoke(...)`.

use crate::manager::{AddRequest, DownloadManager, Incoming, Item, category_for, to_header_map};
use crate::settings::Settings;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::State;
use tauri_plugin_opener::OpenerExt;

type Mgr<'a> = State<'a, Arc<DownloadManager>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    items: Vec<Item>,
    settings: Settings,
    pending: Vec<Incoming>,
    version: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeInfo {
    filename: String,
    size: Option<u64>,
    resumable: bool,
    category: String,
    dir: PathBuf,
}

#[tauri::command]
pub fn get_state(m: Mgr) -> AppState {
    AppState { items: m.snapshot(), settings: m.settings(), pending: m.take_pending(), version: env!("CARGO_PKG_VERSION") }
}

#[tauri::command]
pub async fn probe_url(m: Mgr<'_>, url: String, headers: Vec<(String, String)>) -> Result<ProbeInfo, String> {
    let remote = swiftdl_engine::probe(m.client(), url.trim(), &to_header_map(&headers)).await.map_err(|e| format!("{e:#}"))?;
    let category = category_for(&remote.filename).to_string();
    Ok(ProbeInfo {
        dir: m.default_dir(&category),
        filename: remote.filename,
        size: remote.size,
        resumable: remote.resumable,
        category,
    })
}

#[tauri::command]
pub fn default_dir(m: Mgr, filename: String) -> PathBuf {
    m.default_dir(category_for(&filename))
}

#[tauri::command]
pub fn add_download(m: Mgr, req: AddRequest) -> Result<u64, String> {
    check_add(&req)?;
    Ok(m.add(req))
}

/// What a download may come from: web links, and for torrents also magnet
/// links and .torrent files on this computer.
fn check_add(req: &AddRequest) -> Result<(), String> {
    let url = req.url.trim();
    let web = url.starts_with("http://") || url.starts_with("https://");
    let torrent = req.torrent_files.is_some() && (url.starts_with("magnet:") || std::path::Path::new(url).is_file());
    if !web && !torrent {
        return Err("Only web links, magnet links and .torrent files are supported".into());
    }
    if req.filename.trim().is_empty() {
        return Err("File name is empty".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(url: &str, torrent: bool) -> AddRequest {
        serde_json::from_value(serde_json::json!({
            "url": url, "filename": "name", "dir": "C:/Downloads", "start": true,
            "torrentFiles": if torrent { serde_json::json!([]) } else { serde_json::Value::Null },
        }))
        .unwrap()
    }

    #[test]
    fn accepts_every_kind_of_source() {
        assert!(check_add(&req("https://example.com/a.iso", false)).is_ok());
        assert!(check_add(&req("magnet:?xt=urn:btih:dd8255ecdc7ca55fb0bbf81323d87062db1f6d1c", true)).is_ok());
        assert!(check_add(&req("https://example.com/a.torrent", true)).is_ok());
        let file = std::env::temp_dir().join("swiftdl-test.torrent");
        std::fs::write(&file, b"d4:infod4:name1:aee").unwrap();
        assert!(check_add(&req(&file.to_string_lossy(), true)).is_ok(), "a .torrent file on disk");
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn refuses_other_sources() {
        assert!(check_add(&req("magnet:?xt=urn:btih:abc", false)).is_err(), "magnet only as a torrent");
        assert!(check_add(&req("file:///etc/passwd", false)).is_err());
        assert!(check_add(&req("C:/not/there.torrent", true)).is_err());
    }
}

#[tauri::command]
pub fn pause(m: Mgr, ids: Vec<u64>) {
    m.pause(&ids);
}

#[tauri::command]
pub fn resume(m: Mgr, ids: Vec<u64>) {
    m.resume(&ids);
}

#[tauri::command]
pub fn remove(m: Mgr, ids: Vec<u64>, delete_files: bool) {
    m.remove(&ids, delete_files);
}

#[tauri::command]
pub fn open_file(m: Mgr, id: u64) -> Result<(), String> {
    let item = m.item(id).ok_or("not found")?;
    m.app_handle().opener().open_path(item.path.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_folder(m: Mgr, id: u64) -> Result<(), String> {
    let item = m.item(id).ok_or("not found")?;
    let target = if item.path.exists() { item.path.clone() } else { swiftdl_engine::part_path(&item.path) };
    if target.exists() {
        m.app_handle().opener().reveal_item_in_dir(target).map_err(|e| e.to_string())
    } else {
        let dir = item.path.parent().map(PathBuf::from).unwrap_or_default();
        m.app_handle().opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
    }
}

#[tauri::command]
pub fn set_settings(m: Mgr, settings: Settings) {
    m.set_settings(settings);
}

#[tauri::command]
pub fn open_url(m: Mgr, url: String) -> Result<(), String> {
    m.app_handle().opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn extension_dir(m: Mgr) -> Option<PathBuf> {
    crate::browser::bundled_extension_dir(m.app_handle())
}

#[tauri::command]
pub fn open_extension_folder(m: Mgr) -> Result<(), String> {
    let dir = crate::browser::bundled_extension_dir(m.app_handle()).ok_or("extension folder not found")?;
    m.app_handle().opener().open_path(dir.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_item(m: Mgr, id: u64) -> Option<Item> {
    m.item(id)
}

/// The browser download a "New download" window was opened for.
#[tauri::command]
pub fn confirm_info(m: Mgr, token: u64) -> Option<Incoming> {
    m.confirm_info(token)
}

#[tauri::command]
pub fn forget_confirm(m: Mgr, token: u64) {
    m.forget_confirm(token);
}

#[tauri::command]
pub fn show_main(m: Mgr) {
    crate::show_main_window(m.app_handle());
}

#[tauri::command]
pub fn integration_status() -> crate::browser::IntegrationStatus {
    crate::browser::status()
}

#[tauri::command]
pub fn open_store(m: Mgr, browser: String) -> Result<(), String> {
    crate::browser::open_store_in(m.app_handle(), &browser).map_err(|e| format!("{e:#}"))
}

/// Opens the log file in the system's text viewer, for bug reports.
#[tauri::command]
pub fn open_log(m: Mgr) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let p = crate::log::path();
    if !p.exists() { crate::log::line("log opened"); }
    m.app_handle().opener().open_path(p.to_string_lossy(), None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_extensions_page(browser: String) -> Result<(), String> {
    crate::browser::open_extensions_page(&browser).map_err(|e| format!("{e:#}"))
}

/// Runs off the main thread (async command), so the blocking picker is safe.
#[tauri::command]
pub async fn pick_folder(m: Mgr<'_>, start: Option<PathBuf>) -> Result<Option<PathBuf>, String> {
    use tauri_plugin_dialog::DialogExt;
    let mut dialog = m.app_handle().dialog().file();
    if let Some(dir) = start.filter(|d| d.is_dir()) {
        dialog = dialog.set_directory(dir);
    }
    Ok(dialog.blocking_pick_folder().and_then(|p| p.into_path().ok()))
}

#[tauri::command]
pub fn is_torrent_url(url: String, filename: Option<String>) -> bool {
    crate::torrent::is_torrent(&url, filename.as_deref())
}

#[tauri::command]
pub async fn probe_torrent(m: Mgr<'_>, url: String, headers: Vec<(String, String)>) -> Result<crate::torrent::TorrentInfo, String> {
    crate::torrent::probe(m.client(), &url, &to_header_map(&headers)).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn is_media_url(url: String) -> bool {
    crate::media::is_media_url(url.trim())
}

/// What the video lookup is doing, when it is more than reading the page.
#[tauri::command]
pub fn probe_note() -> Option<String> {
    crate::media::PROBE_NOTE.lock().unwrap().clone()
}

#[tauri::command]
pub async fn probe_media(m: Mgr<'_>, url: String, headers: Vec<(String, String)>) -> Result<crate::media::MediaInfo, String> {
    crate::media::probe(m.client(), url.trim(), headers).await.map_err(|e| format!("{e:#}"))
}

/// A video-site link pasted into the main window: choose its quality in a
/// "New download" window, like a link caught from the browser.
/// Puts a waiting download first in the queue.
#[tauri::command]
pub fn download_next(m: Mgr, id: u64) -> bool {
    m.download_next(id)
}

/// Replaces a stopped download's address and resumes it.
#[tauri::command]
pub fn set_url(m: Mgr, id: u64, url: String) -> Result<(), String> {
    m.set_url(id, &url)
}

/// Adds every link in pasted text (lists and `[01-50]` patterns). Returns how many.
#[tauri::command]
pub fn add_links(m: Mgr, text: String) -> usize {
    m.add_links(&text)
}

#[tauri::command]
pub fn open_new_download(m: Mgr, url: String) {
    m.new_download_window(Incoming { url: url.trim().to_string(), filename: None, referrer: None, cookies: None, user_agent: None, qualities: None, pick: None });
}

#[tauri::command]
pub async fn check_update(m: Mgr<'_>) -> Result<Option<crate::update::Update>, String> {
    crate::update::check(m.client()).await.map_err(|e| {
        crate::log::line(&format!("update check failed: {e:#}"));
        format!("{e:#}")
    })
}

/// Installs a release: on Windows downloads and runs the installer, then quits
/// so it can replace this app; elsewhere opens the download in the browser.
#[tauri::command]
pub async fn install_update(m: Mgr<'_>, url: String) -> Result<(), String> {
    if cfg!(windows) && url.ends_with(".exe") {
        crate::update::run_installer(m.client(), &url).await.map_err(|e| {
            crate::log::line(&format!("update install failed: {e:#}"));
            format!("{e:#}")
        })?;
        m.save();
        m.app_handle().exit(0);
        Ok(())
    } else {
        m.app_handle().opener().open_url(url, None::<&str>).map_err(|e| e.to_string())
    }
}
