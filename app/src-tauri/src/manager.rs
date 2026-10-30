//! The download list: queueing, persistence, live progress, and the bridge
//! between the engine and the UI. All state sits behind one mutex that is
//! never held across an `.await`.

use crate::settings::Settings;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use swiftdl_engine::header::{HeaderMap, HeaderName, HeaderValue};
use swiftdl_engine::{CancellationToken, Client, Outcome, Progress, Request, SpeedLimiter};
use tauri::{AppHandle, Emitter};

#[derive(Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Completed,
    Error,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: u64,
    pub url: String,
    pub filename: String,
    pub path: PathBuf,
    pub size: Option<u64>,
    pub downloaded: u64,
    pub status: Status,
    pub error: Option<String>,
    pub category: String,
    pub added_at: u64,
    pub completed_at: Option<u64>,
    pub resumable: bool,
    /// Cookie/Referer/User-Agent captured by the browser extension. Kept so
    /// resuming still works on sites that need a login; cleared on completion.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Missing byte ranges, for the connection map. Empty when complete or
    /// when the server can't do ranges.
    #[serde(default)]
    pub remaining: Vec<(u64, u64)>,
    /// Bytes per second, smoothed. Live only, not meaningful on disk.
    #[serde(default, skip_deserializing)]
    pub speed: f64,
    #[serde(default, skip_deserializing)]
    pub connections: usize,
}

/// A download sent by the browser extension, waiting for the user to
/// confirm it in the Add dialog.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Incoming {
    pub url: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub referrer: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default)]
    pub user_agent: Option<String>,
}

impl Incoming {
    pub fn headers(&self) -> Vec<(String, String)> {
        let mut h = vec![];
        if let Some(c) = self.cookies.as_ref().filter(|c| !c.is_empty()) {
            h.push(("Cookie".into(), c.clone()));
        }
        if let Some(r) = self.referrer.as_ref().filter(|r| !r.is_empty()) {
            h.push(("Referer".into(), r.clone()));
        }
        if let Some(u) = self.user_agent.as_ref().filter(|u| !u.is_empty()) {
            h.push(("User-Agent".into(), u.clone()));
        }
        h
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddRequest {
    pub url: String,
    pub filename: String,
    pub dir: PathBuf,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// False adds it paused ("Download later").
    pub start: bool,
}

struct Running {
    path: PathBuf,
    cancel: CancellationToken,
    progress: Arc<Progress>,
    last: (u64, Instant),
    speed: f64,
}

struct Inner {
    items: Vec<Item>,
    settings: Settings,
    running: HashMap<u64, Running>,
    pending: Vec<Incoming>,
    next_id: u64,
    dirty: bool,
}

pub struct DownloadManager {
    app: AppHandle,
    client: Client,
    limiter: Arc<SpeedLimiter>,
    data_dir: PathBuf,
    inner: Mutex<Inner>,
}

impl DownloadManager {
    pub fn new(app: AppHandle, data_dir: PathBuf, default_download_dir: PathBuf) -> Result<Arc<Self>> {
        std::fs::create_dir_all(&data_dir)?;
        let mut settings: Settings = load_json(&data_dir.join("settings.json")).unwrap_or_default();
        if settings.download_dir.as_os_str().is_empty() {
            settings.download_dir = default_download_dir;
        }
        let mut items: Vec<Item> = load_json(&data_dir.join("downloads.json")).unwrap_or_default();
        for item in &mut items {
            // The app was closed mid-download: leave it paused, like IDM.
            if item.status == Status::Downloading {
                item.status = Status::Paused;
            }
            if item.status != Status::Completed && item.remaining.is_empty() {
                item.remaining = swiftdl_engine::saved_remaining(&item.path).unwrap_or_default();
            }
        }
        let next_id = items.iter().map(|i| i.id).max().unwrap_or(0) + 1;
        Ok(Arc::new(Self {
            app,
            client: swiftdl_engine::build_client()?,
            limiter: Arc::new(SpeedLimiter::new(settings.speed_limit_kib * 1024)),
            data_dir,
            inner: Mutex::new(Inner {
                items,
                settings,
                running: HashMap::new(),
                pending: vec![],
                next_id,
                dirty: false,
            }),
        }))
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn app_handle(&self) -> &AppHandle {
        &self.app
    }

    pub fn settings(&self) -> Settings {
        self.inner.lock().unwrap().settings.clone()
    }

    pub fn snapshot(&self) -> Vec<Item> {
        self.inner.lock().unwrap().items.clone()
    }

    /// Where a file of this category goes by default.
    pub fn default_dir(&self, category: &str) -> PathBuf {
        let s = self.settings();
        if s.sort_into_categories && category != "Other" {
            s.download_dir.join(category)
        } else {
            s.download_dir
        }
    }

    pub fn add(self: &Arc<Self>, req: AddRequest) -> u64 {
        let filename = req.filename.trim().to_string();
        let path = swiftdl_engine::resolve_dest(&req.dir, &filename);
        let id = {
            let mut g = self.inner.lock().unwrap();
            let id = g.next_id;
            g.next_id += 1;
            g.items.push(Item {
                id,
                url: req.url,
                filename: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or(filename.clone()),
                category: category_for(&filename).to_string(),
                path,
                size: None,
                downloaded: 0,
                status: if req.start { Status::Queued } else { Status::Paused },
                error: None,
                added_at: now_ms(),
                completed_at: None,
                resumable: true,
                headers: req.headers,
                remaining: vec![],
                speed: 0.0,
                connections: 0,
            });
            id
        };
        self.changed();
        self.schedule();
        id
    }

    pub fn pause(self: &Arc<Self>, ids: &[u64]) {
        {
            let mut g = self.inner.lock().unwrap();
            for id in ids {
                if let Some(r) = g.running.get(id) {
                    r.cancel.cancel(); // the task marks it paused when it stops
                } else if let Some(item) = g.items.iter_mut().find(|i| i.id == *id && i.status == Status::Queued) {
                    item.status = Status::Paused;
                }
            }
        }
        self.changed();
    }

    pub fn resume(self: &Arc<Self>, ids: &[u64]) {
        {
            let mut g = self.inner.lock().unwrap();
            for item in g.items.iter_mut().filter(|i| ids.contains(&i.id)) {
                if matches!(item.status, Status::Paused | Status::Error) {
                    item.status = Status::Queued;
                    item.error = None;
                }
            }
        }
        self.changed();
        self.schedule();
    }

    pub fn remove(self: &Arc<Self>, ids: &[u64], delete_files: bool) {
        let removed: Vec<Item> = {
            let mut g = self.inner.lock().unwrap();
            for id in ids {
                if let Some(r) = g.running.get(id) {
                    r.cancel.cancel();
                }
            }
            let (gone, keep) = std::mem::take(&mut g.items).into_iter().partition(|i| ids.contains(&i.id));
            g.items = keep;
            gone
        };
        for item in &removed {
            // Running ones clean up their own temp files when their task ends.
            if item.status != Status::Downloading {
                remove_temp_files(&item.path);
            }
            if delete_files && item.status == Status::Completed {
                let _ = std::fs::remove_file(&item.path);
            }
        }
        self.changed();
    }

    pub fn set_settings(self: &Arc<Self>, settings: Settings) {
        self.limiter.set_rate(settings.speed_limit_kib * 1024);
        self.inner.lock().unwrap().settings = settings;
        self.save();
        self.schedule();
    }

    pub fn item(&self, id: u64) -> Option<Item> {
        self.inner.lock().unwrap().items.iter().find(|i| i.id == id).cloned()
    }

    /// A download arrived from the browser extension.
    pub fn incoming(self: &Arc<Self>, inc: Incoming) {
        if self.settings().ask_before_download {
            self.inner.lock().unwrap().pending.push(inc.clone());
            let _ = self.app.emit("incoming", inc);
            crate::show_main_window(&self.app);
            return;
        }
        // No dialog: probe for the real file name, then start right away.
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let headers = inc.headers();
            let filename = match swiftdl_engine::probe(&this.client, &inc.url, &to_header_map(&headers)).await {
                Ok(remote) => remote.filename,
                Err(_) => inc.filename.clone().unwrap_or_else(|| "download.bin".into()),
            };
            let dir = this.default_dir(category_for(&filename));
            this.add(AddRequest { url: inc.url, filename, dir, headers, start: true });
        });
    }

    pub fn take_pending(&self) -> Vec<Incoming> {
        std::mem::take(&mut self.inner.lock().unwrap().pending)
    }

    /// Starts queued downloads until `max_concurrent` are running.
    fn schedule(self: &Arc<Self>) {
        let to_start: Vec<(u64, Request, Arc<Progress>, CancellationToken)> = {
            let mut g = self.inner.lock().unwrap();
            let Inner { items, running, settings, .. } = &mut *g;
            let free = settings.max_concurrent.max(1).saturating_sub(running.len());
            let connections = settings.connections;
            items
                .iter_mut()
                .filter(|i| i.status == Status::Queued)
                .take(free)
                .map(|item| {
                    let id = item.id;
                    item.status = Status::Downloading;
                    let req = Request {
                        url: item.url.clone(),
                        dest: item.path.clone(),
                        connections,
                        headers: to_header_map(&item.headers),
                        restart: false,
                    };
                    let progress = Arc::new(Progress::default());
                    progress.downloaded.store(item.downloaded, Ordering::Relaxed);
                    let cancel = CancellationToken::new();
                    running.insert(
                        id,
                        Running {
                            path: item.path.clone(),
                            cancel: cancel.clone(),
                            progress: progress.clone(),
                            last: (item.downloaded, Instant::now()),
                            speed: 0.0,
                        },
                    );
                    (id, req, progress, cancel)
                })
                .collect()
        };
        if to_start.is_empty() {
            return;
        }
        for (id, req, progress, cancel) in to_start {
            let this = self.clone();
            tauri::async_runtime::spawn(async move {
                let result = swiftdl_engine::run(&this.client, &req, progress, cancel, this.limiter.clone()).await;
                this.finish(id, result);
            });
        }
        self.changed();
    }

    fn finish(self: &Arc<Self>, id: u64, result: Result<Outcome>) {
        let mut notify = None;
        {
            let mut g = self.inner.lock().unwrap();
            let running = g.running.remove(&id);
            let notify_enabled = g.settings.notify_on_complete;
            match g.items.iter_mut().find(|i| i.id == id) {
                None => {
                    // Removed while running: drop what it left behind.
                    if let Some(r) = &running {
                        remove_temp_files(&r.path);
                    }
                }
                Some(item) => {
                    if let Some(r) = &running {
                        item.downloaded = r.progress.downloaded.load(Ordering::Relaxed);
                        let total = r.progress.total.load(Ordering::Relaxed);
                        if total > 0 {
                            item.size = Some(total);
                        }
                        item.resumable = r.progress.resumable.load(Ordering::Relaxed);
                        item.remaining = r.progress.remaining.lock().unwrap().clone();
                    }
                    item.speed = 0.0;
                    item.connections = 0;
                    match result {
                        Ok(Outcome::Completed) => {
                            item.status = Status::Completed;
                            item.completed_at = Some(now_ms());
                            item.size = item.size.or(Some(item.downloaded));
                            item.downloaded = item.size.unwrap_or(item.downloaded);
                            item.headers.clear();
                            item.remaining.clear();
                            if notify_enabled {
                                notify = Some(item.filename.clone());
                            }
                        }
                        Ok(Outcome::Paused) => item.status = Status::Paused,
                        Err(e) => {
                            item.status = Status::Error;
                            item.error = Some(format!("{e:#}"));
                        }
                    }
                }
            }
        }
        if let Some(name) = notify {
            use tauri_plugin_notification::NotificationExt;
            let _ = self.app.notification().builder().title("Download complete").body(name).show();
        }
        self.changed();
        self.save();
        self.schedule();
    }

    /// Called every 500 ms: copies live progress into items, computes
    /// speeds, and pushes a snapshot to the UI when something changed.
    pub fn tick(&self, n: u64) {
        let (snapshot, active) = {
            let mut g = self.inner.lock().unwrap();
            let Inner { items, running, dirty, .. } = &mut *g;
            let now = Instant::now();
            for (id, r) in running.iter_mut() {
                let done = r.progress.downloaded.load(Ordering::Relaxed);
                let dt = now.duration_since(r.last.1).as_secs_f64();
                if dt > 0.0 {
                    let instant = done.saturating_sub(r.last.0) as f64 / dt;
                    r.speed = if r.speed == 0.0 { instant } else { r.speed * 0.6 + instant * 0.4 };
                }
                r.last = (done, now);
                if let Some(item) = items.iter_mut().find(|i| i.id == *id) {
                    item.downloaded = done;
                    let total = r.progress.total.load(Ordering::Relaxed);
                    if total > 0 {
                        item.size = Some(total);
                    }
                    item.speed = r.speed;
                    item.connections = r.progress.connections.load(Ordering::Relaxed);
                    item.remaining = r.progress.remaining.lock().unwrap().clone();
                }
            }
            let active = !running.is_empty();
            if !active && !*dirty {
                return;
            }
            *dirty = false;
            (items.clone(), active)
        };
        let _ = self.app.emit("downloads", snapshot);
        if active && n % 10 == 0 {
            self.save();
        }
    }

    fn changed(&self) {
        self.inner.lock().unwrap().dirty = true;
    }

    pub fn save(&self) {
        let (items, settings) = {
            let g = self.inner.lock().unwrap();
            (g.items.clone(), g.settings.clone())
        };
        let _ = save_json(&self.data_dir.join("downloads.json"), &items);
        let _ = save_json(&self.data_dir.join("settings.json"), &settings);
    }

    pub fn start_ticker(self: &Arc<Self>) {
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            for n in 0u64.. {
                tick.tick().await;
                this.tick(n);
            }
        });
        self.schedule();
    }

}

pub fn to_header_map(headers: &[(String, String)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (k, v) in headers {
        if let (Ok(k), Ok(v)) = (HeaderName::from_bytes(k.as_bytes()), HeaderValue::from_str(v)) {
            map.insert(k, v);
        }
    }
    map
}

pub fn category_for(filename: &str) -> &'static str {
    let ext = Path::new(filename).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "zip" | "rar" | "7z" | "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "iso" | "img" => "Compressed",
        "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "txt" | "rtf" | "epub" | "csv" | "md" => {
            "Documents"
        }
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "opus" | "wma" => "Music",
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "flv" | "wmv" | "m4v" | "mpg" | "mpeg" | "3gp" | "ts" => "Video",
        "exe" | "msi" | "dmg" | "pkg" | "deb" | "rpm" | "apk" | "appimage" | "msix" | "bat" | "jar" => "Programs",
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "svg" | "bmp" | "heic" | "avif" => "Images",
        _ => "Other",
    }
}

fn remove_temp_files(path: &Path) {
    let _ = std::fs::remove_file(swiftdl_engine::part_path(path));
    let mut state = path.as_os_str().to_owned();
    state.push(".swiftdl");
    let _ = std::fs::remove_file(PathBuf::from(state));
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn save_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
