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
    if !req.url.starts_with("http://") && !req.url.starts_with("https://") {
        return Err("Only http:// and https:// links are supported".into());
    }
    if req.filename.trim().is_empty() {
        return Err("File name is empty".into());
    }
    Ok(m.add(req))
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
