//! Video sites (YouTube and others), through yt-dlp.
//!
//! These sites don't serve a plain file. YouTube's player streams through a
//! private protocol with no download address, so SwiftDL hands these pages
//! to yt-dlp, which keeps up with such changes, and to ffmpeg, which joins
//! the separate video and audio streams. Both are fetched into the app's
//! data folder the first time they're needed, and yt-dlp is kept current.

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::Value;
use std::io::{BufRead, BufReader, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::Ordering;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};
use swiftdl_engine::{CancellationToken, Client, Outcome, Progress};

/// Sites sent to yt-dlp instead of the regular engine.
const SITES: &[&str] = &[
    "youtube.com", "youtu.be", "youtube-nocookie.com", "vimeo.com", "x.com", "twitter.com", "reddit.com",
    "instagram.com", "tiktok.com", "facebook.com", "twitch.tv", "dailymotion.com", "soundcloud.com", "bilibili.com",
];

/// A video stream: an HLS playlist (.m3u8) or a DASH manifest (.mpd). These
/// come in many small pieces, so yt-dlp fetches them and ffmpeg joins them.
pub fn is_stream_url(url: &str) -> bool {
    let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    path.starts_with("http") && (path.ends_with(".m3u8") || path.ends_with(".mpd"))
}

pub fn is_media_url(url: &str) -> bool {
    if is_stream_url(url) {
        return true;
    }
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    SITES.iter().any(|s| host == *s || host.ends_with(&format!(".{s}")))
}

/// Live status text for a media download ("Merging video and audio").
pub type Note = Arc<Mutex<Option<String>>>;

fn set_note(note: &Note, text: impl Into<String>) {
    *note.lock().unwrap() = Some(text.into());
}

// ---------- tools ----------

fn tools_dir() -> PathBuf {
    crate::browser::data_dir().join("tools")
}

fn ytdlp_path() -> PathBuf {
    tools_dir().join(if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" })
}

fn ytdlp_asset() -> &'static str {
    if cfg!(windows) {
        "yt-dlp.exe"
    } else if cfg!(target_os = "macos") {
        "yt-dlp_macos"
    } else if cfg!(target_arch = "aarch64") {
        "yt-dlp_linux_aarch64"
    } else {
        "yt-dlp_linux"
    }
}

const FFMPEG_EXE: &str = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };

/// Folder holding ffmpeg: our own copy (Windows), or the system's.
fn ffmpeg_dir() -> Option<PathBuf> {
    let own = tools_dir().join("ffmpeg");
    if own.join(FFMPEG_EXE).exists() {
        return Some(own);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"].map(PathBuf::from))
        .find(|d| d.join(FFMPEG_EXE).is_file())
}

static TOOLS: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

struct Tools {
    ytdlp: PathBuf,
    ffmpeg: Option<PathBuf>,
}

/// Downloads yt-dlp (and ffmpeg on Windows) if they're missing.
async fn ensure_tools(client: &Client, note: &Note, need_ffmpeg: bool) -> Result<Tools> {
    let _guard = TOOLS.lock().await;
    std::fs::create_dir_all(tools_dir())?;
    let ytdlp = ytdlp_path();
    if !ytdlp.exists() {
        let url = format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{}", ytdlp_asset());
        fetch(client, &url, &ytdlp, note, "Getting yt-dlp").await.context("couldn't download yt-dlp")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ytdlp, std::fs::Permissions::from_mode(0o755))?;
        }
        stamp_update();
    } else {
        update_in_background(&ytdlp);
    }
    if need_ffmpeg && ffmpeg_dir().is_none() && cfg!(windows) {
        let own = tools_dir().join("ffmpeg").join(FFMPEG_EXE);
        crate::log::line(&format!("ffmpeg not found ({}: {:?}), downloading it", own.display(), std::fs::metadata(&own).err()));
        let zip = tools_dir().join("ffmpeg.zip");
        let url = "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl-shared.zip";
        fetch(client, url, &zip, note, "Getting ffmpeg").await.context("couldn't download ffmpeg")?;
        set_note(note, "Unpacking ffmpeg");
        let unpacked = unpack_ffmpeg(&zip, &tools_dir().join("ffmpeg"));
        let _ = std::fs::remove_file(&zip);
        unpacked.context("couldn't unpack ffmpeg")?;
    }
    *note.lock().unwrap() = None;
    Ok(Tools { ytdlp, ffmpeg: ffmpeg_dir() })
}

/// Downloads `url` to `dest` with SwiftDL's own engine: many connections at
/// once, and resumable if the app is closed halfway. A single connection to
/// GitHub can crawl along at a few KB/s; ffmpeg alone is over 100 MB.
async fn fetch(client: &Client, url: &str, dest: &Path, note: &Note, label: &str) -> Result<()> {
    set_note(note, label);
    let req = swiftdl_engine::Request {
        url: url.to_string(),
        dest: dest.to_path_buf(),
        connections: 8,
        headers: Default::default(),
        restart: false,
        mirrors: vec![],
    };
    let progress = Arc::new(Progress::default());
    let ticker = {
        let (progress, note, label) = (progress.clone(), note.clone(), label.to_string());
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(400)).await;
                let (got, total) = (progress.downloaded.load(Ordering::Relaxed), progress.total.load(Ordering::Relaxed));
                if total > 0 {
                    set_note(&note, format!("{label} · {}%", got * 100 / total));
                }
            }
        })
    };
    let limiter = Arc::new(swiftdl_engine::SpeedLimiter::new(0));
    let done = swiftdl_engine::run(client, &req, progress, CancellationToken::new(), limiter).await;
    ticker.abort();
    match done? {
        Outcome::Completed => Ok(()),
        Outcome::Paused => bail!("the download stopped"),
    }
}

/// Copies the programs and libraries from the archive's `bin` folder.
fn unpack_ffmpeg(zip: &Path, dest: &Path) -> Result<()> {
    std::fs::create_dir_all(dest)?;
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip)?)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let name = entry.name()?.replace('\\', "/");
        let Some((dir, file)) = name.rsplit_once('/') else { continue };
        if !dir.ends_with("/bin") || file.is_empty() {
            continue;
        }
        let mut out = std::fs::File::create(dest.join(file))?;
        let mut buf = vec![];
        entry.read_to_end(&mut buf)?;
        out.write_all(&buf)?;
    }
    if !dest.join(FFMPEG_EXE).exists() {
        bail!("ffmpeg wasn't in the archive");
    }
    Ok(())
}

fn stamp_path() -> PathBuf {
    tools_dir().join("yt-dlp.checked")
}

fn stamp_update() {
    let _ = std::fs::write(stamp_path(), b"");
}

/// YouTube changes often and yt-dlp follows; check for a new yt-dlp once a day.
fn update_in_background(ytdlp: &Path) {
    let fresh = std::fs::metadata(stamp_path())
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < Duration::from_secs(24 * 3600));
    if fresh {
        return;
    }
    stamp_update();
    let ytdlp = ytdlp.to_path_buf();
    std::thread::spawn(move || {
        let _ = command(&ytdlp).arg("-U").stdout(Stdio::null()).stderr(Stdio::null()).status();
    });
}

/// ffmpeg itself, if we have it.
pub fn ffmpeg_path() -> Option<PathBuf> {
    ffmpeg_dir().map(|d| d.join(FFMPEG_EXE))
}

/// Gets ffmpeg first if it's missing (Windows), for joining streams.
pub async fn ensure_ffmpeg(client: &Client, note: &Note) -> Result<()> {
    ensure_tools(client, note, true).await.map(|_| ())
}

pub fn quiet_command(program: &Path) -> Command {
    command(program)
}

fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

/// Passes the browser's headers on to yt-dlp. Stream servers often refuse
/// requests without the page's cookies, referrer or user agent.
/// The proxy from Settings, passed to yt-dlp (empty: its own default).
static PROXY: LazyLock<Mutex<String>> = LazyLock::new(Default::default);

pub fn set_proxy(proxy: &str) {
    *PROXY.lock().unwrap() = proxy.trim().to_string();
}

/// The local address from Settings, passed to yt-dlp as --source-address.
static BIND: LazyLock<Mutex<String>> = LazyLock::new(Default::default);

pub fn set_bind(address: &str) {
    *BIND.lock().unwrap() = address.trim().to_string();
}

fn add_headers(cmd: &mut Command, headers: &[(String, String)]) {
    let proxy = PROXY.lock().unwrap().clone();
    if !proxy.is_empty() {
        cmd.arg("--proxy").arg(proxy);
    }
    let bind = BIND.lock().unwrap().clone();
    if !bind.is_empty() {
        cmd.arg("--source-address").arg(bind);
    }
    for (name, value) in headers.iter().filter(|(_, v)| !v.is_empty()) {
        match name.to_ascii_lowercase().as_str() {
            "referer" => { cmd.arg("--referer").arg(value); }
            "user-agent" => { cmd.arg("--user-agent").arg(value); }
            _ => { cmd.arg("--add-headers").arg(format!("{name}:{value}")); }
        }
    }
}

/// The browser headers to give yt-dlp. On YouTube, a signed-in browser's
/// cookies make yt-dlp use a client whose formats need a JavaScript runtime:
/// without one it finds no formats at all ("Requested format is not
/// available"). So the quick, runtime-free tries go without cookies; the retry
/// with a runtime keeps them, for age-restricted and private videos.
fn headers_for(url: &str, headers: &[(String, String)], with_js: bool) -> Vec<(String, String)> {
    let drop_cookies = !with_js && youtube_id(url).is_some();
    headers.iter().filter(|(k, _)| !(drop_cookies && k.eq_ignore_ascii_case("cookie"))).cloned().collect()
}

/// A yt-dlp command, told which JavaScript runtime to use. Current yt-dlp
/// needs one for full YouTube extraction; without it some formats go missing.
fn ytdlp_command(ytdlp: &Path) -> Command {
    let mut cmd = command(ytdlp);
    if let Some(rt) = js_runtime() {
        cmd.args(["--js-runtimes", rt]);
    }
    cmd
}

/// The first JavaScript runtime yt-dlp supports that's on PATH.
fn js_runtime() -> Option<&'static str> {
    static FOUND: std::sync::OnceLock<Option<&'static str>> = std::sync::OnceLock::new();
    *FOUND.get_or_init(|| {
        let path = std::env::var_os("PATH")?;
        let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd"] } else { &[""] };
        ["deno", "node", "bun", "quickjs"].into_iter().find(|name| {
            let bin = if *name == "quickjs" { "qjs" } else { name };
            std::env::split_paths(&path).any(|d| exts.iter().any(|e| d.join(format!("{bin}{e}")).is_file()))
        })
    })
}

// ---------- probing ----------

#[derive(Serialize, serde::Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Quality {
    pub label: String,
    /// yt-dlp format selector.
    pub format: String,
    pub ext: String,
    pub size: Option<u64>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub title: String,
    pub site: String,
    pub duration: Option<f64>,
    pub qualities: Vec<Quality>,
}

/// What looking up a video is doing right now ("Getting yt-dlp · 40%"), for
/// the New download window. None while yt-dlp itself is reading the page.
pub static PROBE_NOTE: LazyLock<Note> = LazyLock::new(Note::default);

/// Longest wait for yt-dlp to read a page before giving up.
const PROBE_LIMIT: Duration = Duration::from_secs(90);

/// Asks yt-dlp what the page offers, as a short list of qualities.
///
/// The first try runs without a JavaScript runtime: listing the qualities
/// doesn't need one, and with one YouTube takes 30-40 s instead of about 5.
/// Only if that comes back short (an error, or nothing above 480p) does it
/// ask again with the runtime. The download itself always gets the runtime.
pub async fn probe(client: &Client, url: &str, headers: Vec<(String, String)>) -> Result<MediaInfo> {
    let tools = ensure_tools(client, &PROBE_NOTE, false).await?;
    *PROBE_NOTE.lock().unwrap() = None;
    let quick = read_info(tools.ytdlp.clone(), url.to_string(), headers.clone(), false).await;
    let enough = |i: &MediaInfo| i.qualities.iter().any(|q| q.label.split(|c: char| !c.is_ascii_digit()).next().unwrap_or("").parse::<u32>().is_ok_and(|h| h > 480));
    match quick {
        Ok(info) if enough(&info) || js_runtime().is_none() => Ok(info),
        quick => {
            if let Err(e) = &quick {
                crate::log::line(&format!("quick lookup failed, trying with {}: {e:#}", js_runtime().unwrap_or("")));
            }
            match read_info(tools.ytdlp, url.to_string(), headers, true).await {
                Ok(info) => Ok(info),
                // Keep a short list over nothing.
                Err(e) => quick.or(Err(e)),
            }
        }
    }
}

/// Runs `yt-dlp -J` and returns its JSON, giving up after PROBE_LIMIT.
async fn ytdlp_json(ytdlp: PathBuf, url: String, headers: Vec<(String, String)>, with_js: bool) -> Result<Vec<u8>> {
    let url_for_log = url.clone();
    let out = tauri::async_runtime::spawn_blocking(move || -> Result<std::process::Output> {
        let mut cmd = if with_js { ytdlp_command(&ytdlp) } else { command(&ytdlp) };
        cmd.args(["-J", "--no-playlist", "--no-warnings", "--socket-timeout", "20"]);
        add_headers(&mut cmd, &headers_for(&url, &headers, with_js));
        // Plain anonymous pipes, as the common Windows launchers make. Rust's own
        // `Stdio::piped()` is a named pipe, and on some PCs yt-dlp's Python can't
        // write to it ("[Errno 22] Invalid argument", seen on Bilibili).
        let (mut so, so_w) = os_pipe::pipe()?;
        let (mut se, se_w) = os_pipe::pipe()?;
        let mut child = cmd.arg(&url).stdout(so_w).stderr(se_w).spawn().context("couldn't start yt-dlp")?;
        // The command still holds the write ends; without this the readers never see the end.
        drop(cmd);
        // Read both pipes on their own threads so a full pipe can't stall yt-dlp.
        let out = std::thread::spawn(move || { let mut b = vec![]; let _ = so.read_to_end(&mut b); b });
        let err = std::thread::spawn(move || { let mut b = vec![]; let _ = se.read_to_end(&mut b); b });
        let start = std::time::Instant::now();
        let status = loop {
            if let Some(s) = child.try_wait()? {
                break s;
            }
            if start.elapsed() > PROBE_LIMIT {
                let _ = child.kill();
                let _ = child.wait();
                bail!("The site didn't answer in {} seconds. Check your connection and try again.", PROBE_LIMIT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        Ok(std::process::Output { status, stdout: out.join().unwrap_or_default(), stderr: err.join().unwrap_or_default() })
    })
    .await??;
    if !out.status.success() {
        crate::log::line(&format!("yt-dlp couldn't read {url_for_log}: {}", String::from_utf8_lossy(&out.stderr).trim()));
        bail!("{}", last_error(&String::from_utf8_lossy(&out.stderr)).unwrap_or_else(|| "yt-dlp couldn't read this page".into()));
    }
    Ok(out.stdout)
}

// ---------- YouTube: look the video up before the download is started ----------
//
// Most of the wait before a YouTube download is yt-dlp unscrambling the
// video's addresses. SwiftDL starts that the moment the quality menu opens
// and saves the answer; the download then hands it to yt-dlp
// (--load-info-json) and starts right away.

/// The video id of a YouTube watch, shorts or youtu.be link.
fn youtube_id(url: &str) -> Option<String> {
    let u = url::Url::parse(url).ok()?;
    let host = u.host_str()?.trim_start_matches("www.").trim_start_matches("m.");
    let id = match host {
        "youtube.com" | "music.youtube.com" => u.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.into_owned())
            .or_else(|| u.path().strip_prefix("/shorts/").map(|s| s.trim_end_matches('/').to_string())),
        "youtu.be" => Some(u.path().trim_start_matches('/').to_string()),
        _ => None,
    }?;
    (id.len() >= 6 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then_some(id)
}

fn lookup_path(url: &str) -> Option<PathBuf> {
    Some(crate::browser::data_dir().join("cache").join(format!("yt-{}.json", youtube_id(url)?)))
}

/// A saved lookup young enough that its addresses still work (YouTube's last
/// about six hours; keep well inside that).
fn fresh_lookup(url: &str) -> Option<PathBuf> {
    let p = lookup_path(url)?;
    let age = std::fs::metadata(&p).ok()?.modified().ok()?.elapsed().ok()?;
    (age < Duration::from_secs(2 * 3600)).then_some(p)
}

/// Videos being looked up right now.
static PREPARING: LazyLock<Mutex<std::collections::HashSet<String>>> = LazyLock::new(Default::default);

/// Looks a YouTube video up in the background and saves the answer.
pub async fn prepare(client: &Client, url: &str, headers: Vec<(String, String)>) {
    let (Some(id), Some(path)) = (youtube_id(url), lookup_path(url)) else { return };
    if fresh_lookup(url).is_some() || !PREPARING.lock().unwrap().insert(id.clone()) {
        return;
    }
    let done = async {
        let tools = ensure_tools(client, &Note::default(), false).await?;
        let watch = format!("https://www.youtube.com/watch?v={id}");
        // With a JS runtime when there is one: only then are YouTube's speed
        // checks solved, and without that its servers send the file at a
        // crawl. It takes a few seconds, but runs while the user picks a
        // quality. Without a runtime (or if that fails), the quick lookup.
        let quick = || ytdlp_json(tools.ytdlp.clone(), watch.clone(), headers.clone(), false);
        let json = if js_runtime().is_some() {
            match ytdlp_json(tools.ytdlp.clone(), watch.clone(), headers.clone(), true).await {
                Ok(j) => j,
                Err(e) => {
                    crate::log::line(&format!("lookup of {id} with {} failed, trying without: {e:#}", js_runtime().unwrap_or("")));
                    quick().await?
                }
            }
        } else {
            quick().await?
        };
        std::fs::create_dir_all(path.parent().unwrap())?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &json)?;
        std::fs::rename(&tmp, &path)?;
        anyhow::Ok(())
    }
    .await;
    if let Err(e) = done {
        crate::log::line(&format!("couldn't look up {url} ahead of time: {e:#}"));
    }
    PREPARING.lock().unwrap().remove(&id);
}

/// The saved lookup for `url`, waiting for one that is still running.
async fn wait_for_lookup(url: &str) -> Option<PathBuf> {
    let id = youtube_id(url)?;
    for _ in 0..300 {
        if !PREPARING.lock().unwrap().contains(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    fresh_lookup(url)
}

async fn read_info(ytdlp: PathBuf, url: String, headers: Vec<(String, String)>, with_js: bool) -> Result<MediaInfo> {
    let stdout = ytdlp_json(ytdlp, url, headers, with_js).await?;
    let info: Value = serde_json::from_slice(&stdout).context("yt-dlp gave an unexpected answer")?;
    Ok(MediaInfo {
        title: info["title"].as_str().unwrap_or("video").to_string(),
        site: info["extractor_key"].as_str().unwrap_or("").to_string(),
        duration: info["duration"].as_f64(),
        qualities: qualities(&info),
    })
}

fn size_of(f: &Value) -> Option<u64> {
    f["filesize"].as_u64().or_else(|| f["filesize_approx"].as_u64())
}

/// One entry per video height, best first, then audio only.
fn qualities(info: &Value) -> Vec<Quality> {
    let formats = info["formats"].as_array().cloned().unwrap_or_default();
    let has = |f: &Value, key: &str| f[key].as_str().is_some_and(|c| c != "none");
    let videos: Vec<&Value> = formats.iter().filter(|f| has(f, "vcodec") && f["height"].as_u64().unwrap_or(0) > 0).collect();
    let audios: Vec<&Value> = formats.iter().filter(|f| has(f, "acodec") && !has(f, "vcodec")).collect();
    let tbr = |f: &&Value| f["tbr"].as_f64().unwrap_or(0.0);
    let best_audio = audios.iter().copied().max_by(|a, b| tbr(a).total_cmp(&tbr(b)));
    let audio_size = best_audio.and_then(size_of);

    let mut heights: Vec<u64> = videos.iter().filter_map(|f| f["height"].as_u64()).collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    let mut list: Vec<Quality> = heights
        .into_iter()
        .map(|h| {
            let at: Vec<&&Value> = videos.iter().filter(|f| f["height"].as_u64() == Some(h)).collect();
            let best = at.iter().copied().max_by(|a, b| tbr(a).total_cmp(&tbr(b))).copied();
            let fps = at.iter().filter_map(|f| f["fps"].as_f64()).fold(0.0, f64::max);
            let progressive = best.is_some_and(|f| has(f, "acodec"));
            let size = best.and_then(size_of).map(|v| if progressive { v } else { v + audio_size.unwrap_or(0) });
            Quality {
                label: if fps > 30.5 { format!("{h}p{}", fps.round()) } else { format!("{h}p") },
                // H.264 + AAC first: it plays everywhere. AV1/VP9 only
                // where YouTube has nothing else (1440p and 4K).
                format: format!(
                    "bv*[height={h}][vcodec^=avc1]+ba[ext=m4a]/bv*[height={h}]+ba[ext=m4a]/bv*[height={h}]+ba/b[height={h}]/bv*[height<={h}]+ba/b"
                ),
                ext: "mp4".into(),
                size,
            }
        })
        .collect();
    if let Some(a) = best_audio {
        let m4a = audios.iter().any(|f| f["ext"].as_str() == Some("m4a"));
        list.push(Quality {
            label: "Audio only".into(),
            format: "ba[ext=m4a]/ba".into(),
            ext: if m4a { "m4a".into() } else { a["ext"].as_str().unwrap_or("m4a").into() },
            size: if m4a { audios.iter().filter(|f| f["ext"].as_str() == Some("m4a")).filter_map(|f| size_of(f)).max() } else { size_of(a) },
        });
    }
    if list.is_empty() {
        list.push(Quality { label: "Best quality".into(), format: "bv*+ba/b".into(), ext: info["ext"].as_str().unwrap_or("mp4").into(), size: None });
    }
    list
}

fn last_error(stderr: &str) -> Option<String> {
    let e = stderr.lines().rev().find_map(|l| l.strip_prefix("ERROR: ")).map(|e| e.trim().to_string())?;
    if e.contains("Requested format is not available") && js_runtime().is_none() {
        return Some("This site needs a JavaScript runtime for yt-dlp. Install Node.js or Deno, then try again.".into());
    }
    // A stream address the site no longer serves: links like these expire, or
    // work once, and "generic" is yt-dlp's name for a plain address.
    if e.contains("[generic]") && (e.contains("HTTP Error 404") || e.contains("HTTP Error 403") || e.contains("HTTP Error 410")) {
        return Some("The site no longer gives out this stream link. Such links can expire or work only once: play the video again in the browser, then use SwiftDL on it.".into());
    }
    // yt-dlp's advice names its own flags, which mean nothing here.
    let lower = e.to_lowercase();
    if lower.contains("--cookies") || lower.contains("logged-in") || lower.contains("authentication is required") || lower.contains("login required") {
        return Some("This site needs you to be signed in. Download it with the SwiftDL extension in a browser where you're signed in, so your session is passed on.".into());
    }
    Some(e)
}

// ---------- downloading ----------

pub struct Job {
    pub url: String,
    pub format: String,
    pub dest: PathBuf,
    /// The browser's Cookie, Referer and User-Agent, when it came from one.
    pub headers: Vec<(String, String)>,
}

/// Runs yt-dlp for one download, reporting into `progress` and `note`.
/// Cancelling stops yt-dlp; its partial files stay, so running the same job
/// again continues where it left off.
pub async fn run(client: &Client, job: Job, progress: Arc<Progress>, note: Note, cancel: CancellationToken) -> Result<Outcome> {
    // Streams arrive in pieces that ffmpeg turns into one MP4.
    let stream = is_stream_url(&job.url);
    let merges = job.format.contains('+') || stream;
    let tools = ensure_tools(client, &note, merges).await?;
    if merges && tools.ffmpeg.is_none() {
        bail!("Joining video and audio needs ffmpeg. Install it (brew install ffmpeg, or sudo apt install ffmpeg) and try again.");
    }
    if cancel.is_cancelled() {
        return Ok(Outcome::Paused);
    }
    if let Some(dir) = job.dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    set_note(&note, "Starting");

    // YouTube, looked up ahead of time: fetch the files ourselves, many
    // connections each, the way IDM does. yt-dlp is the fallback.
    if !stream {
        match direct(client, &job, &progress, &note, &cancel).await {
            Ok(Some(done)) => return Ok(done),
            Ok(None) => {}
            Err(e) => {
                crate::log::line(&format!("direct download of {} failed, using yt-dlp: {e:#}", job.url));
                progress.downloaded.store(0, Ordering::Relaxed);
                set_note(&note, "Starting");
            }
        }
    }

    // First without a JavaScript runtime: yt-dlp gets going several seconds
    // sooner, and it nearly always works. If it fails, try once more with one.
    let (mut exit, mut stderr) = attempt(&tools, &job, &progress, &note, &cancel, stream, merges, false).await?;
    if exit == Some(false) && js_runtime().is_some() && !cancel.is_cancelled() {
        crate::log::line(&format!("yt-dlp failed without {}, retrying with it: {}", js_runtime().unwrap_or(""), last_error(&stderr).unwrap_or_default()));
        set_note(&note, "Starting");
        progress.downloaded.store(0, Ordering::Relaxed);
        (exit, stderr) = attempt(&tools, &job, &progress, &note, &cancel, stream, merges, true).await?;
    }
    progress.connections.store(0, Ordering::Relaxed);
    *note.lock().unwrap() = None;

    match exit {
        None => Ok(Outcome::Paused),
        Some(true) if job.dest.exists() => {
            let size = std::fs::metadata(&job.dest).map(|m| m.len()).unwrap_or(0);
            progress.total.store(size, Ordering::Relaxed);
            progress.downloaded.store(size, Ordering::Relaxed);
            Ok(Outcome::Completed)
        }
        Some(true) => Err(anyhow!("yt-dlp finished but the file wasn't created")),
        Some(false) => Err(anyhow!(last_error(&stderr).unwrap_or_else(|| "yt-dlp stopped with an error".into()))),
    }
}

/// The formats a quality choice means, picked from a saved lookup the same
/// way yt-dlp would: `(video, audio)`, either may be absent.
fn choose<'a>(info: &'a Value, format: &str) -> Option<(Option<&'a Value>, Option<&'a Value>)> {
    let formats = info["formats"].as_array()?;
    let plain = |f: &&Value| f["protocol"].as_str() == Some("https") && f["url"].is_string();
    let codec = |f: &Value, k: &str| f[k].as_str().is_some_and(|c| c != "none");
    let tbr = |f: &&&Value| f["tbr"].as_f64().unwrap_or(0.0);
    let audio = formats.iter().filter(plain).filter(|f| codec(f, "acodec") && !codec(f, "vcodec"));
    let best_m4a = audio.clone().filter(|f| f["ext"].as_str() == Some("m4a")).max_by(|a, b| tbr(&a).total_cmp(&tbr(&b)));
    let best_audio = best_m4a.or_else(|| audio.max_by(|a, b| tbr(&a).total_cmp(&tbr(&b))));
    if format.starts_with("ba") {
        return Some((None, Some(best_audio?)));
    }
    let h: u64 = format.split("height=").nth(1)?.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
    let at: Vec<&Value> = formats.iter().filter(plain).filter(|f| codec(f, "vcodec") && !codec(f, "acodec") && f["height"].as_u64() == Some(h)).collect();
    // Same preference as the format string: H.264 first, it plays everywhere.
    let avc = at.iter().copied().filter(|f| f["vcodec"].as_str().is_some_and(|c| c.starts_with("avc1"))).max_by(|a, b| tbr(&a).total_cmp(&tbr(&b)));
    let video = avc.or_else(|| at.iter().copied().max_by(|a, b| tbr(&a).total_cmp(&tbr(&b))))?;
    Some((Some(video), Some(best_audio?)))
}

/// YouTube holds each connection to roughly 0.5 MB/s; 16 at once doubled the
/// speed of 8 in testing, and 32 was no faster than 16.
const YOUTUBE_CONNECTIONS: usize = 16;

/// Downloads a YouTube video straight from its saved lookup with SwiftDL's
/// engine. `None` when there is no lookup or nothing in it fits, so yt-dlp
/// should do it instead.
async fn direct(client: &Client, job: &Job, progress: &Arc<Progress>, note: &Note, cancel: &CancellationToken) -> Result<Option<Outcome>> {
    if youtube_id(&job.url).is_none() {
        return Ok(None);
    }
    // Not looked up yet (a pasted link, say): do it now, then download directly.
    if wait_for_lookup(&job.url).await.is_none() {
        set_note(note, "Getting video details");
        prepare(client, &job.url, job.headers.clone()).await;
    }
    let Some(saved) = wait_for_lookup(&job.url).await else { return Ok(None) };
    let info: Value = serde_json::from_slice(&std::fs::read(&saved)?)?;
    let Some((video, audio)) = choose(&info, &job.format) else { return Ok(None) };
    let parts: Vec<(&Value, PathBuf)> = [(video, "video"), (audio, "audio")]
        .into_iter()
        .filter_map(|(f, tag)| Some((f?, job.dest.with_extension(format!("{tag}.{}", f?["ext"].as_str().unwrap_or("bin"))))))
        .collect();
    if parts.is_empty() {
        return Ok(None);
    }
    *note.lock().unwrap() = None;
    let total: u64 = parts.iter().map(|(f, _)| f["filesize"].as_u64().or(f["filesize_approx"].as_u64()).unwrap_or(0)).sum();
    progress.total.store(total, Ordering::Relaxed);

    let each: Vec<Arc<Progress>> = parts.iter().map(|_| Arc::new(Progress::default())).collect();
    let ticker = {
        let (each, progress) = (each.clone(), progress.clone());
        tokio::spawn(async move {
            loop {
                let got: u64 = each.iter().map(|p| p.downloaded.load(Ordering::Relaxed)).sum();
                let conns: usize = each.iter().map(|p| p.connections.load(Ordering::Relaxed)).sum();
                progress.downloaded.store(got, Ordering::Relaxed);
                progress.connections.store(conns, Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        })
    };
    let limiter = Arc::new(swiftdl_engine::SpeedLimiter::new(0));
    let started = std::time::Instant::now();
    let runs = parts.iter().zip(&each).map(|((f, path), p)| {
        let mut headers = swiftdl_engine::header::HeaderMap::new();
        for (k, v) in f["http_headers"].as_object().into_iter().flatten() {
            if let (Ok(k), Some(Ok(v))) = (k.parse::<swiftdl_engine::header::HeaderName>(), v.as_str().map(|v| v.parse())) {
                headers.insert(k, v);
            }
        }
        let req = swiftdl_engine::Request { url: f["url"].as_str().unwrap_or_default().to_string(), dest: path.clone(), connections: YOUTUBE_CONNECTIONS, headers, restart: false, mirrors: vec![] };
        let (p, cancel, limiter) = (p.clone(), cancel.clone(), limiter.clone());
        async move { swiftdl_engine::run(client, &req, p, cancel, limiter).await }
    });
    let results = futures_util::future::join_all(runs).await;
    ticker.abort();
    progress.connections.store(0, Ordering::Relaxed);
    for r in results {
        if r? == Outcome::Paused {
            return Ok(Some(Outcome::Paused));
        }
    }

    let fetched = started.elapsed();
    set_note(note, "Joining video and audio");
    let files: Vec<PathBuf> = parts.iter().map(|(_, p)| p.clone()).collect();
    if files.len() == 1 {
        std::fs::rename(&files[0], &job.dest)?;
    } else {
        let ff = ffmpeg_path().context("Joining video and audio needs ffmpeg.")?;
        let (files2, dest) = (files.clone(), job.dest.clone());
        let out = tauri::async_runtime::spawn_blocking(move || {
            let mut cmd = command(&ff);
            cmd.args(["-y", "-loglevel", "error", "-i"]).arg(&files2[0]).arg("-i").arg(&files2[1])
                .args(["-map", "0:v", "-map", "1:a", "-c", "copy", "-movflags", "+faststart"]).arg(&dest);
            cmd.output()
        })
        .await??;
        if !out.status.success() {
            bail!("couldn't join video and audio: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or(""));
        }
        for f in &files {
            let _ = std::fs::remove_file(f);
        }
    }
    *note.lock().unwrap() = None;
    let size = std::fs::metadata(&job.dest).map(|m| m.len()).unwrap_or(0);
    crate::log::line(&format!("direct download: {} MB fetched in {:.1}s, joined in {:.1}s", size / 1_000_000, fetched.as_secs_f64(), (started.elapsed() - fetched).as_secs_f64()));
    progress.total.store(size, Ordering::Relaxed);
    progress.downloaded.store(size, Ordering::Relaxed);
    Ok(Some(Outcome::Completed))
}

/// One yt-dlp run. Returns how it ended (None when cancelled) and its errors.
#[allow(clippy::too_many_arguments)]
async fn attempt(tools: &Tools, job: &Job, progress: &Arc<Progress>, note: &Note, cancel: &CancellationToken, stream: bool, merges: bool, with_js: bool) -> Result<(Option<bool>, String)> {
    let mut cmd = if with_js { ytdlp_command(&tools.ytdlp) } else { command(&tools.ytdlp) };
    cmd.args(["-f", &job.format, "--no-playlist", "--newline", "--no-colors", "--no-mtime", "-N", "4"])
        .args(["--progress-template", "download:SDL|%(progress.status)s|%(progress.downloaded_bytes)s|%(progress.total_bytes)s|%(progress.total_bytes_estimate)s"]);
    if merges {
        cmd.args(["--merge-output-format", "mp4"]);
    }
    if stream {
        // yt-dlp's own HLS downloader fetches -N pieces at once; ffmpeg's
        // would take them one by one.
        cmd.arg("--hls-prefer-native");
    }
    if let Some(dir) = &tools.ffmpeg {
        cmd.arg("--ffmpeg-location").arg(dir);
    }
    add_headers(&mut cmd, &headers_for(&job.url, &job.headers, with_js));
    // yt-dlp treats % in the output name as a template field.
    cmd.arg("-o").arg(job.dest.to_string_lossy().replace('%', "%%"));
    // A saved lookup skips yt-dlp's slowest step. (The retry with a JS
    // runtime looks the video up afresh, in case the saved one was the trouble.)
    match if with_js { None } else { wait_for_lookup(&job.url).await } {
        Some(saved) => { cmd.arg("--load-info-json").arg(saved); }
        None => { cmd.arg(&job.url); }
    }
    // Plain anonymous pipes; see ytdlp_json.
    let (stdout, out_w) = os_pipe::pipe()?;
    let (mut stderr, err_w) = os_pipe::pipe()?;
    let child = cmd.stdout(out_w).stderr(err_w).spawn().context("couldn't start yt-dlp")?;
    drop(cmd);

    let errors = Arc::new(Mutex::new(String::new()));
    {
        let errors = errors.clone();
        std::thread::spawn(move || {
            // Read bytes, not lines of UTF-8: stopping at the first byte that isn't
            // valid UTF-8 (yt-dlp writes the PC's own code page) would close the pipe
            // on yt-dlp, and its next write fails with "[Errno 22]".
            let mut bytes = vec![];
            let _ = stderr.read_to_end(&mut bytes);
            *errors.lock().unwrap() = String::from_utf8_lossy(&bytes).into_owned();
        });
    }
    let reader = {
        let (progress, note) = (progress.clone(), note.clone());
        std::thread::spawn(move || read_progress(stdout, &progress, &note))
    };

    let child = Arc::new(Mutex::new(child));
    let waiter = {
        let (child, cancel) = (child.clone(), cancel.clone());
        tauri::async_runtime::spawn_blocking(move || wait(&child, &cancel))
    };
    let exit = waiter.await??;
    let _ = reader.join();
    let stderr = errors.lock().unwrap().clone();
    Ok((exit, stderr))
}

/// Waits for yt-dlp. `None` when cancelled, else whether it succeeded.
fn wait(child: &Mutex<Child>, cancel: &CancellationToken) -> Result<Option<bool>> {
    loop {
        if cancel.is_cancelled() {
            let mut c = child.lock().unwrap();
            let _ = c.kill();
            let _ = c.wait();
            return Ok(None);
        }
        if let Some(status) = child.lock().unwrap().try_wait()? {
            return Ok(Some(status.success()));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Turns yt-dlp's output into progress. A video download is two files
/// (video, then audio), each reported from zero, so finished files are
/// added to a running base.
fn read_progress(stdout: impl std::io::Read, progress: &Progress, note: &Note) {
    let num = |s: &str| s.trim().parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0).map(|n| n as u64);
    let (mut base, mut cur) = (0u64, 0u64);
    let hint = progress.total.load(Ordering::Relaxed);
    // Lines as raw bytes, converted leniently: an odd byte must never end the
    // reading, or the pipe closes under yt-dlp (see attempt).
    let mut reader = BufReader::new(stdout);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if !matches!(reader.read_until(b'\n', &mut raw), Ok(n) if n > 0) {
            break;
        }
        let line = String::from_utf8_lossy(&raw);
        let line = line.trim_end();
        if let Some(rest) = line.strip_prefix("SDL|") {
            let f: Vec<&str> = rest.split('|').collect();
            if f.len() < 4 {
                continue;
            }
            let done = num(f[1]).unwrap_or(0);
            let total = num(f[2]).or_else(|| num(f[3])).unwrap_or(0);
            let cur_total;
            if f[0] == "finished" {
                base += total.max(done).max(cur);
                (cur, cur_total) = (0, 0);
            } else {
                (cur, cur_total) = (done, total);
                *note.lock().unwrap() = None;
                progress.connections.store(1, Ordering::Relaxed);
            }
            progress.downloaded.store(base + cur, Ordering::Relaxed);
            progress.total.store(hint.max(base + cur_total), Ordering::Relaxed);
        } else if line.starts_with("[Merger]") || line.starts_with("[VideoRemuxer]") {
            set_note(note, "Merging video and audio");
            progress.connections.store(0, Ordering::Relaxed);
        } else if line.starts_with("[Fixup") || line.starts_with("[ExtractAudio]") {
            set_note(note, "Finishing");
        }
    }
}

/// Removes yt-dlp's partial files for a download that was cancelled.
pub fn cleanup(dest: &Path) {
    let (Some(dir), Some(name)) = (dest.parent(), dest.file_name().map(|n| n.to_string_lossy().into_owned())) else { return };
    let stem = dest.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let partial = n.ends_with(".part") || n.ends_with(".ytdl") || n.contains(".part-Frag");
        if partial && (n.starts_with(&name) || n.starts_with(&format!("{stem}.f"))) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn youtube_cookies_only_with_a_runtime() {
        let h = vec![("Cookie".to_string(), "SID=1".to_string()), ("Referer".to_string(), "x".to_string())];
        let yt = "https://www.youtube.com/watch?v=M5OCLifZK1w";
        assert_eq!(super::headers_for(yt, &h, false).len(), 1);
        assert_eq!(super::headers_for(yt, &h, true).len(), 2);
        assert_eq!(super::headers_for("https://vimeo.com/1", &h, false).len(), 2);
    }

    #[test]
    fn sign_in_errors_are_put_plainly() {
        let vimeo = "ERROR: [vimeo] 1: The web client only works when logged-in. Use --cookies, --cookies-from-browser, --username and --password";
        assert!(super::last_error(vimeo).unwrap().starts_with("This site needs you to be signed in"));
        assert_eq!(super::last_error("ERROR: [x] 1: Video unavailable").unwrap(), "[x] 1: Video unavailable");
    }

    #[test]
    fn expired_stream_links_are_explained() {
        let e = "ERROR: [generic] 82134615: Unable to download webpage: HTTP Error 404: Not Found (caused by <HTTPError 404: Not Found>)";
        assert!(super::last_error(e).unwrap().starts_with("The site no longer gives out this stream link"));
    }

    #[test]
    fn finds_youtube_ids() {
        assert_eq!(super::youtube_id("https://www.youtube.com/watch?v=M5OCLifZK1w&list=RD").as_deref(), Some("M5OCLifZK1w"));
        assert_eq!(super::youtube_id("https://youtu.be/M5OCLifZK1w").as_deref(), Some("M5OCLifZK1w"));
        assert_eq!(super::youtube_id("https://www.youtube.com/shorts/abcDEF12345").as_deref(), Some("abcDEF12345"));
        assert_eq!(super::youtube_id("https://vimeo.com/123"), None);
    }

    #[test]
    fn recognises_streams() {
        assert!(is_stream_url("https://cdn.example.com/live/master.m3u8"));
        assert!(is_stream_url("https://cdn.example.com/v/index.M3U8?token=abc#t=5"));
        assert!(is_stream_url("https://cdn.example.com/dash/manifest.mpd"));
        assert!(is_media_url("https://cdn.example.com/live/master.m3u8"));
        assert!(!is_stream_url("https://cdn.example.com/video.mp4"));
        assert!(!is_stream_url("https://cdn.example.com/page?file=master.m3u8"));
        assert!(!is_stream_url("magnet:?xt=urn:btih:abc&dn=x.m3u8"));
    }

    #[test]
    fn recognises_video_sites() {
        assert!(is_media_url("https://www.youtube.com/watch?v=aqz-KE-bpKQ"));
        assert!(is_media_url("https://youtu.be/aqz-KE-bpKQ"));
        assert!(is_media_url("https://m.youtube.com/shorts/x"));
        assert!(!is_media_url("https://drive.usercontent.google.com/download?id=1"));
        assert!(!is_media_url("https://notyoutube.com/watch"));
    }

    #[test]
    fn lists_one_quality_per_height_then_audio() {
        let info = serde_json::json!({ "formats": [
            { "format_id": "137", "vcodec": "avc1", "acodec": "none", "height": 1080, "fps": 60, "tbr": 4000, "filesize": 400 },
            { "format_id": "248", "vcodec": "vp9", "acodec": "none", "height": 1080, "fps": 60, "tbr": 3000, "filesize": 300 },
            { "format_id": "136", "vcodec": "avc1", "acodec": "none", "height": 720, "fps": 30, "tbr": 2000, "filesize_approx": 200 },
            { "format_id": "140", "vcodec": "none", "acodec": "mp4a", "ext": "m4a", "tbr": 128, "filesize": 10 },
            { "format_id": "251", "vcodec": "none", "acodec": "opus", "ext": "webm", "tbr": 160, "filesize": 12 },
        ]});
        let q = qualities(&info);
        let labels: Vec<&str> = q.iter().map(|q| q.label.as_str()).collect();
        assert_eq!(labels, ["1080p60", "720p", "Audio only"]);
        assert_eq!(q[0].size, Some(400 + 12), "best video plus best audio");
        assert_eq!(q[2].ext, "m4a");
        assert_eq!(q[2].size, Some(10));
    }

    #[test]
    fn adds_up_progress_across_video_and_audio() {
        let out = "[download] Destination: a.f396.mp4\nSDL|downloading|1024|15298808|NA\nSDL|finished|15298808|15298808|NA\n\
                   [download] Destination: a.f251.webm\nSDL|downloading|5000000|10202210|NA\nSDL|finished|10202210|10202210|NA\n\
                   [Merger] Merging formats into \"a.mp4\"\n";
        let progress = Progress::default();
        progress.total.store(25_000_000, Ordering::Relaxed);
        let note = Note::default();
        read_progress(out.as_bytes(), &progress, &note);
        assert_eq!(progress.downloaded.load(Ordering::Relaxed), 15298808 + 10202210);
        assert_eq!(progress.total.load(Ordering::Relaxed), 15298808 + 10202210);
        assert_eq!(note.lock().unwrap().as_deref(), Some("Merging video and audio"));
    }
}

#[cfg(test)]
mod live {
    /// `cargo test --release live_youtube -- --ignored --nocapture` with
    /// SWIFTDL_YT_URL and SWIFTDL_YT_OUT set. Downloads 1080p for real.
    /// Runs the same lookup the app does, for any URL in SWIFTDL_PROBE_URL.
    #[test]
    #[ignore]
    fn live_probe() {
        let url = std::env::var("SWIFTDL_PROBE_URL").unwrap();
        let ytdlp = super::ytdlp_path();
        for with_js in [false, true] {
            let r = tauri::async_runtime::block_on(super::ytdlp_json(ytdlp.clone(), url.clone(), vec![], with_js));
            match r {
                Ok(j) => println!("with_js={with_js}: ok, {} bytes", j.len()),
                Err(e) => println!("with_js={with_js}: FAILED: {e:#}"),
            }
        }
    }

    /// Downloads the smallest quality of SWIFTDL_DL_URL to SWIFTDL_DL_OUT through
    /// the same code path as the app. Run it with PYTHONIOENCODING=gbk to act
    /// like a Chinese-language Windows.
    #[test]
    #[ignore]
    fn live_download() {
        let url = std::env::var("SWIFTDL_DL_URL").unwrap();
        let dest = std::path::PathBuf::from(std::env::var("SWIFTDL_DL_OUT").unwrap());
        let client = swiftdl_engine::build_client().unwrap();
        let job = super::Job { url, format: "wv*+wa/w".into(), dest: dest.clone(), headers: vec![] };
        let r = tauri::async_runtime::block_on(super::run(&client, job, Default::default(), Default::default(), Default::default()));
        match r {
            Ok(o) => println!("RESULT {o:?}, {} bytes", std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0)),
            Err(e) => println!("RESULT FAILED: {e:#}"),
        }
    }

    #[test]
    #[ignore]
    fn live_youtube() {
        let url = std::env::var("SWIFTDL_YT_URL").unwrap();
        let dest = std::path::PathBuf::from(std::env::var("SWIFTDL_YT_OUT").unwrap());
        let client = swiftdl_engine::build_client().unwrap();
        let format = "bv*[height=1080][vcodec^=avc1]+ba[ext=m4a]/bv*[height=1080]+ba[ext=m4a]/bv*[height=1080]+ba/b[height=1080]/bv*[height<=1080]+ba/b".to_string();
        tauri::async_runtime::block_on(async {
            let t = std::time::Instant::now();
            super::prepare(&client, &url, vec![]).await;
            println!("lookup: {:.1}s", t.elapsed().as_secs_f64());
            let t = std::time::Instant::now();
            let job = super::Job { url, format, dest: dest.clone(), headers: vec![] };
            let r = super::run(&client, job, Default::default(), Default::default(), Default::default()).await.unwrap();
            println!("{r:?}: download + join {:.1}s, {} bytes", t.elapsed().as_secs_f64(), std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0));
        });
    }
}
