//! The download engine: runs N connections in parallel, writes every chunk
//! straight to its final offset, and keeps every connection busy until the
//! very end by splitting the largest unfinished segment whenever one frees up.

use crate::limiter::SpeedLimiter;
use crate::probe::{RemoteFile, probe};
use crate::segment::{Segment, initial_segments};
use crate::state::SavedState;
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode, header};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

/// Never split a segment into halves smaller than this; a new connection's
/// handshake costs more than it saves below ~512 KiB.
const MIN_SPLIT_HALF: u64 = 512 * 1024;
const MAX_RETRIES: u32 = 8;
const SAVE_EVERY: Duration = Duration::from_secs(1);

pub struct Request {
    pub url: String,
    /// Final file path. `<dest>.part` and `<dest>.swiftdl` live beside it
    /// until the download completes.
    pub dest: PathBuf,
    pub connections: usize,
    pub headers: header::HeaderMap,
    /// Ignore any saved progress and start over.
    pub restart: bool,
    /// Other addresses of the same file. Each is checked (same size, byte
    /// ranges allowed); the ones that pass share the connections, and a
    /// connection whose server fails moves on to another.
    pub mirrors: Vec<String>,
}

/// Live numbers a front end can poll from any thread.
#[derive(Default)]
pub struct Progress {
    /// Total size in bytes; 0 while unknown.
    pub total: AtomicU64,
    pub downloaded: AtomicU64,
    pub connections: AtomicUsize,
    /// False when the server cannot resume (no range support).
    pub resumable: std::sync::atomic::AtomicBool,
    /// Byte ranges `[pos, end)` still missing; each one's `pos` is where a
    /// connection is writing. Lets a UI draw the connection map.
    pub remaining: Mutex<Vec<(u64, u64)>>,
    /// Every segment as `(start, pos, end)`, in file order, for a
    /// per-connection view.
    pub segments: Mutex<Vec<(u64, u64, u64)>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    /// Cancelled through the token. Progress is saved if the server
    /// supports resuming; otherwise the next run starts over.
    Paused,
}

struct Shared {
    client: Client,
    /// The file's address, then any mirrors that passed the check.
    urls: Vec<String>,
    headers: header::HeaderMap,
    file: File,
    table: Mutex<Table>,
    progress: Arc<Progress>,
    limiter: Arc<SpeedLimiter>,
}

struct Table {
    segments: Vec<Arc<Segment>>,
    /// Segments before this index have been handed to a worker.
    next: usize,
}

impl Shared {
    /// Hands a worker its next job: an unclaimed segment if any are left,
    /// otherwise the back half of whichever segment has the most left.
    fn claim(&self) -> Option<Arc<Segment>> {
        let mut t = self.table.lock().unwrap();
        while t.next < t.segments.len() {
            let seg = t.segments[t.next].clone();
            t.next += 1;
            if seg.remaining() > 0 {
                return Some(seg);
            }
        }
        let victim = t.segments.iter().max_by_key(|s| s.remaining())?.clone();
        let stolen = Arc::new(victim.split(MIN_SPLIT_HALF)?);
        t.segments.push(stolen.clone());
        t.next = t.segments.len();
        Some(stolen)
    }

    fn remaining_ranges(&self) -> Vec<(u64, u64)> {
        let t = self.table.lock().unwrap();
        t.segments.iter().map(|s| s.range()).filter(|(p, e)| p < e).collect()
    }

    fn remaining_bytes(&self) -> u64 {
        let t = self.table.lock().unwrap();
        t.segments.iter().map(|s| s.remaining()).sum()
    }

    fn spans(&self) -> Vec<(u64, u64, u64)> {
        let t = self.table.lock().unwrap();
        let mut v: Vec<_> = t.segments.iter().map(|s| s.span()).collect();
        v.sort_by_key(|s| s.0);
        v
    }

    fn publish(&self, size: u64) {
        self.progress.downloaded.store(size - self.remaining_bytes(), Ordering::Relaxed);
        *self.progress.remaining.lock().unwrap() = self.remaining_ranges();
        *self.progress.segments.lock().unwrap() = self.spans();
    }
}

pub fn build_client() -> Result<Client> {
    build_client_with("")
}

/// `proxy` is an address like `http://127.0.0.1:8080`; empty follows the
/// system's proxy settings.
pub fn build_client_with(proxy: &str) -> Result<Client> {
    build_client_opts(proxy, "")
}

/// As `build_client_with`, and `bind` is a local address (like a VPN adapter's)
/// that every connection must go out from. If that address disappears, for
/// example the VPN drops, connections fail rather than use another route.
pub fn build_client_opts(proxy: &str, bind: &str) -> Result<Client> {
    let mut builder = Client::builder();
    if let Ok(ip) = bind.trim().parse::<std::net::IpAddr>() {
        builder = builder.local_address(ip);
    }
    let proxy = proxy.trim();
    if !proxy.is_empty() {
        // An address reqwest can't use (say, a SOCKS one) is ignored: the
        // system's proxy settings apply instead.
        if let Ok(p) = reqwest::Proxy::all(proxy) {
            builder = builder.proxy(p);
        }
    }
    Ok(builder
        // HTTP/2 would multiplex every range over ONE connection and share
        // one congestion window, which defeats the point. Force HTTP/1.1 so
        // each segment gets its own TCP connection.
        .http1_only()
        .user_agent(concat!("swiftdl/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()?)
}

/// Downloads `req` until it completes, fails, or `cancel` fires.
pub async fn run(
    client: &Client,
    req: &Request,
    progress: Arc<Progress>,
    cancel: CancellationToken,
    limiter: Arc<SpeedLimiter>,
) -> Result<Outcome> {
    let remote = tokio::select! {
        r = probe(client, &req.url, &req.headers) => r?,
        _ = cancel.cancelled() => return Ok(Outcome::Paused),
    };
    if let Some(dir) = req.dest.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    }
    progress.total.store(remote.size.unwrap_or(0), Ordering::Relaxed);
    progress.resumable.store(remote.resumable, Ordering::Relaxed);

    let part = part_path(&req.dest);
    let state_path = append_ext(&req.dest, "swiftdl");
    let outcome = match (remote.resumable, remote.size) {
        (true, Some(size)) => {
            download_segmented(client.clone(), req, &remote, size, &part, &state_path, progress, cancel, limiter)
                .await?
        }
        _ => download_single(client, req, &remote, &part, progress, cancel, limiter).await?,
    };
    if outcome == Outcome::Completed {
        let _ = std::fs::remove_file(&state_path);
        if req.dest.exists() {
            std::fs::remove_file(&req.dest)?;
        }
        std::fs::rename(&part, &req.dest).with_context(|| format!("could not rename to {}", req.dest.display()))?;
    }
    Ok(outcome)
}

#[allow(clippy::too_many_arguments)]
async fn download_segmented(
    client: Client,
    req: &Request,
    remote: &RemoteFile,
    size: u64,
    part: &Path,
    state_path: &Path,
    progress: Arc<Progress>,
    cancel: CancellationToken,
    limiter: Arc<SpeedLimiter>,
) -> Result<Outcome> {
    let saved = (!req.restart && part.exists())
        .then(|| SavedState::load(state_path))
        .flatten()
        .filter(|s| s.matches(&req.url, size, &remote.etag, &remote.last_modified));

    let (file, ranges) = match saved {
        Some(s) => (OpenOptions::new().write(true).open(part)?, s.segments),
        None => {
            let file = OpenOptions::new().create(true).write(true).truncate(true).open(part)?;
            // Reserve the full size up front so segments can write anywhere
            // and there is no join step at the end.
            file.set_len(size).context("not enough disk space?")?;
            (file, initial_segments(size, req.connections))
        }
    };

    let mut urls = vec![remote.url.clone()];
    urls.extend(usable_mirrors(&client, req, size).await);
    let shared = Arc::new(Shared {
        client,
        urls,
        headers: req.headers.clone(),
        file,
        table: Mutex::new(Table {
            segments: ranges.into_iter().map(|(p, e)| Arc::new(Segment::new(p, e))).collect(),
            next: 0,
        }),
        progress: progress.clone(),
        limiter,
    });
    shared.publish(size);

    let save = {
        let meta = SavedState {
            url: req.url.clone(),
            size,
            etag: remote.etag.clone(),
            last_modified: remote.last_modified.clone(),
            segments: vec![],
        };
        let shared = shared.clone();
        let state_path = state_path.to_path_buf();
        move || SavedState { segments: shared.remaining_ranges(), ..meta.clone() }.save(&state_path)
    };

    let background = {
        let save = save.clone();
        let shared = shared.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(250));
            for n in 0u64.. {
                tick.tick().await;
                shared.publish(size);
                if n % (SAVE_EVERY.as_millis() as u64 / 250) == 0 {
                    let _ = save();
                }
            }
        })
    };

    let mut workers = JoinSet::new();
    for id in 0..req.connections.max(1) {
        workers.spawn(worker(shared.clone(), id));
    }

    let result: Result<bool> = tokio::select! {
        r = async {
            while let Some(res) = workers.join_next().await {
                res??;
            }
            Ok(true)
        } => r,
        _ = cancel.cancelled() => Ok(false),
    };

    workers.shutdown().await;
    background.abort();
    let _ = background.await;
    progress.connections.store(0, Ordering::Relaxed);
    shared.publish(size);

    match result {
        Ok(true) if shared.remaining_bytes() == 0 => {
            shared.file.sync_all()?;
            Ok(Outcome::Completed)
        }
        Ok(true) => bail!("workers finished but data is missing (this is a bug)"),
        Ok(false) => {
            save()?;
            Ok(Outcome::Paused)
        }
        Err(e) => {
            save()?;
            Err(e)
        }
    }
}

/// The mirrors that really hold this file: they answer, allow byte ranges and
/// report the same size.
async fn usable_mirrors(client: &Client, req: &Request, size: u64) -> Vec<String> {
    let checks = req.mirrors.iter().map(|m| async move {
        let r = tokio::time::timeout(Duration::from_secs(10), probe(client, m, &req.headers)).await.ok()?.ok()?;
        (r.resumable && r.size == Some(size)).then_some(r.url)
    });
    futures_util::future::join_all(checks).await.into_iter().flatten().collect()
}

/// One connection's life: take a segment, download it, take another (or
/// steal half of someone else's), until there is nothing worth taking.
async fn worker(shared: Arc<Shared>, id: usize) -> Result<()> {
    while let Some(seg) = shared.claim() {
        shared.progress.connections.fetch_add(1, Ordering::Relaxed);
        let result = download_segment(&shared, &seg, id).await;
        shared.progress.connections.fetch_sub(1, Ordering::Relaxed);
        result?;
    }
    Ok(())
}

/// Downloads one segment, reconnecting from where it left off on errors.
/// The retry counter resets whenever an attempt makes progress, so only a
/// connection that is truly stuck gives up.
async fn download_segment(shared: &Shared, seg: &Segment, id: usize) -> Result<()> {
    let mut failures = 0u32;
    loop {
        let (pos, end) = seg.range();
        if pos >= end {
            return Ok(());
        }
        // Each connection has its own server; after a failure, the next one.
        let url = &shared.urls[(id + failures as usize) % shared.urls.len()];
        let result = fetch_range(shared, url, seg, pos, end).await;
        let progressed = seg.range().0 > pos;
        if progressed {
            failures = 0;
        }
        match result {
            Ok(()) if progressed => continue,
            // Server closed the stream early without sending anything.
            Ok(()) => {
                failures += 1;
                if failures > MAX_RETRIES {
                    bail!("server keeps closing the connection at byte {pos}");
                }
            }
            Err(e) => {
                failures += 1;
                if failures > MAX_RETRIES {
                    return Err(e);
                }
            }
        }
        let backoff = Duration::from_millis(250 * 2u64.pow(failures.min(5)));
        tokio::time::sleep(backoff).await;
    }
}

async fn fetch_range(shared: &Shared, url: &str, seg: &Segment, pos: u64, end: u64) -> Result<()> {
    let resp = shared
        .client
        .get(url)
        .headers(shared.headers.clone())
        .header(header::RANGE, format!("bytes={}-{}", pos, end - 1))
        .send()
        .await?;
    if resp.status() != StatusCode::PARTIAL_CONTENT {
        bail!("expected 206 for a range request, got {}", resp.status());
    }
    let mut body = resp.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        shared.limiter.take(chunk.len()).await;
        if !seg.write(&shared.file, &chunk)? {
            break; // segment complete (or its tail was stolen): hang up
        }
    }
    Ok(())
}

/// Fallback for servers without range support: one plain sequential stream.
/// Pausing it loses progress, because the server cannot resume.
async fn download_single(
    client: &Client,
    req: &Request,
    remote: &RemoteFile,
    part: &Path,
    progress: Arc<Progress>,
    cancel: CancellationToken,
    limiter: Arc<SpeedLimiter>,
) -> Result<Outcome> {
    use std::io::Write;
    let resp = client.get(&remote.url).headers(req.headers.clone()).send().await?.error_for_status()?;
    let mut file = std::io::BufWriter::new(File::create(part)?);
    let mut body = resp.bytes_stream();
    progress.downloaded.store(0, Ordering::Relaxed);
    progress.connections.store(1, Ordering::Relaxed);
    let result = async {
        loop {
            let chunk = tokio::select! {
                c = body.next() => c,
                _ = cancel.cancelled() => return Ok(Outcome::Paused),
            };
            let Some(chunk) = chunk else { break };
            let chunk = chunk?;
            limiter.take(chunk.len()).await;
            file.write_all(&chunk)?;
            progress.downloaded.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        file.flush()?;
        Ok(Outcome::Completed)
    }
    .await;
    progress.connections.store(0, Ordering::Relaxed);
    result
}

/// Picks a free path for `filename` inside `dir`, adding " (1)", " (2)"...
/// rather than overwriting an existing file. A path with an unfinished
/// `.part` beside it is reused so the download can resume.
pub fn resolve_dest(dir: &Path, filename: &str) -> PathBuf {
    let path = dir.join(filename);
    if !path.exists() || part_path(&path).exists() {
        return path;
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|i| path.with_file_name(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists() && !part_path(p).exists())
        .unwrap()
}

pub fn part_path(dest: &Path) -> PathBuf {
    append_ext(dest, "part")
}

/// The still-missing ranges of a paused download, from its resume file.
/// Points a paused download's saved progress at a new address for the same
/// file (a link that expired and was replaced). The size and the server's
/// validators are still checked when it resumes, so a different file starts over.
pub fn rebase_saved(dest: &Path, new_url: &str) -> bool {
    let path = append_ext(dest, "swiftdl");
    let Some(mut s) = SavedState::load(&path) else { return false };
    s.url = new_url.to_string();
    s.save(&path).is_ok()
}

pub fn saved_remaining(dest: &Path) -> Option<Vec<(u64, u64)>> {
    SavedState::load(&append_ext(dest, "swiftdl")).map(|s| s.segments)
}

fn append_ext(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

#[cfg(test)]
mod rebase_tests {
    use super::*;

    #[test]
    fn a_refreshed_link_keeps_its_progress() {
        let dest = std::env::temp_dir().join("swiftdl-rebase-test.bin");
        let state = SavedState { url: "https://x/f?token=old".into(), size: 100, etag: Some("e".into()), last_modified: None, segments: vec![(40, 100)] };
        state.save(&append_ext(&dest, "swiftdl")).unwrap();
        assert!(!state.matches("https://x/f?token=new", 100, &Some("e".into()), &None), "a different address would start over");
        assert!(rebase_saved(&dest, "https://x/f?token=new"));
        let moved = SavedState::load(&append_ext(&dest, "swiftdl")).unwrap();
        assert!(moved.matches("https://x/f?token=new", 100, &Some("e".into()), &None));
        assert!(!moved.matches("https://x/f?token=new", 101, &Some("e".into()), &None), "a different size still starts over");
        assert_eq!(moved.segments, vec![(40, 100)]);
        let _ = std::fs::remove_file(append_ext(&dest, "swiftdl"));
        assert!(!rebase_saved(&dest, "https://x/f"), "nothing saved, nothing to move");
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::atomic::AtomicUsize;

    /// A small server for one file. `total` is the size it claims; `flaky`
    /// makes it fail every request except the first byte (which passes the check).
    fn serve(data: Arc<Vec<u8>>, total: usize, flaky: bool, hits: Arc<AtomicUsize>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (data, hits) = (data.clone(), hits.clone());
                std::thread::spawn(move || {
                    let mut s = stream;
                    let mut buf = [0u8; 2048];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    hits.fetch_add(1, Ordering::Relaxed);
                    let range = head.lines().find_map(|l| l.to_lowercase().strip_prefix("range: bytes=").map(str::to_string)).unwrap_or_default();
                    let (a, b) = range.trim().split_once('-').unwrap_or(("0", "0"));
                    let (a, b): (usize, usize) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
                    if flaky && a != 0 {
                        let _ = s.write_all(b"HTTP/1.1 500 Oops\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        return;
                    }
                    let end = b.min(data.len() - 1);
                    let body = &data[a.min(end)..=end];
                    let _ = s.write_all(format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{end}/{total}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes());
                    let _ = s.write_all(body);
                });
            }
        });
        format!("http://127.0.0.1:{port}/file.bin")
    }

    async fn download(urls: Vec<String>, name: &str) -> Vec<u8> {
        let dest = std::env::temp_dir().join(format!("swiftdl-mirror-{name}-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&dest);
        let req = Request { url: urls[0].clone(), dest: dest.clone(), connections: 4, headers: Default::default(), restart: true, mirrors: urls[1..].to_vec() };
        let client = build_client().unwrap();
        let out = run(&client, &req, Arc::new(Progress::default()), CancellationToken::new(), Arc::new(SpeedLimiter::new(0))).await.unwrap();
        assert_eq!(out, Outcome::Completed);
        let got = std::fs::read(&dest).unwrap();
        let _ = std::fs::remove_file(&dest);
        got
    }

    #[tokio::test]
    async fn mirrors_share_the_download_and_cover_for_a_failing_one() {
        let data: Arc<Vec<u8>> = Arc::new((0..2_000_000u32).map(|i| (i % 251) as u8).collect());
        let (ha, hb, hc, hd) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let a = serve(data.clone(), data.len(), false, ha.clone());
        let b = serve(data.clone(), data.len(), false, hb.clone());
        // Same bytes but it claims another size: must be left out.
        let wrong = serve(data.clone(), data.len() + 5, false, hc.clone());
        // Passes the check, then fails every real request.
        let flaky = serve(data.clone(), data.len(), true, hd.clone());

        let got = download(vec![a, b, wrong, flaky], "all").await;
        assert!(got == *data, "the file must come out byte for byte");
        assert!(ha.load(Ordering::Relaxed) > 1 && hb.load(Ordering::Relaxed) > 1, "both good mirrors carried parts");
        assert_eq!(hc.load(Ordering::Relaxed), 1, "a mirror with the wrong size is only asked once, for the check");
    }
}

#[cfg(test)]
mod socks_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::sync::atomic::AtomicUsize;

    fn web() -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi");
            }
        });
        port
    }

    /// A bare SOCKS5 server (no login, CONNECT only) that counts what it carries.
    fn socks(hits: Arc<AtomicUsize>) -> u16 {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut client in listener.incoming().flatten() {
                let hits = hits.clone();
                std::thread::spawn(move || {
                    let mut b = [0u8; 300];
                    client.read_exact(&mut b[..2]).unwrap();
                    let n = b[1] as usize;
                    client.read_exact(&mut b[..n]).unwrap();
                    client.write_all(&[5, 0]).unwrap();
                    client.read_exact(&mut b[..4]).unwrap();
                    let host = match b[3] {
                        1 => {
                            client.read_exact(&mut b[..4]).unwrap();
                            format!("{}.{}.{}.{}", b[0], b[1], b[2], b[3])
                        }
                        3 => {
                            client.read_exact(&mut b[..1]).unwrap();
                            let len = b[0] as usize;
                            client.read_exact(&mut b[..len]).unwrap();
                            String::from_utf8_lossy(&b[..len]).into_owned()
                        }
                        _ => return,
                    };
                    client.read_exact(&mut b[..2]).unwrap();
                    let port = u16::from_be_bytes([b[0], b[1]]);
                    let Ok(target) = TcpStream::connect((host.as_str(), port)) else { return };
                    client.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
                    hits.fetch_add(1, Ordering::Relaxed);
                    let (mut c2, mut t2) = (client.try_clone().unwrap(), target.try_clone().unwrap());
                    std::thread::spawn(move || { let _ = std::io::copy(&mut c2, &mut t2); });
                    let mut target = target;
                    let _ = std::io::copy(&mut target, &mut client);
                });
            }
        });
        port
    }

    #[tokio::test]
    async fn downloads_through_a_socks5_proxy() {
        let target = web();
        let hits = Arc::new(AtomicUsize::new(0));
        let proxy = socks(hits.clone());
        let client = build_client_opts(&format!("socks5h://127.0.0.1:{proxy}"), "").unwrap();
        let body = client.get(format!("http://localhost:{target}/")).send().await.unwrap().text().await.unwrap();
        assert_eq!(body, "hi");
        assert!(hits.load(Ordering::Relaxed) >= 1, "the request went through the proxy");
    }
}
