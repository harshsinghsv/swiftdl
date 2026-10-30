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
    url: String,
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
}

pub fn build_client() -> Result<Client> {
    Ok(Client::builder()
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

    let shared = Arc::new(Shared {
        client,
        url: remote.url.clone(),
        headers: req.headers.clone(),
        file,
        table: Mutex::new(Table {
            segments: ranges.into_iter().map(|(p, e)| Arc::new(Segment::new(p, e))).collect(),
            next: 0,
        }),
        progress: progress.clone(),
        limiter,
    });
    progress.downloaded.store(size - shared.remaining_bytes(), Ordering::Relaxed);

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
                shared.progress.downloaded.store(size - shared.remaining_bytes(), Ordering::Relaxed);
                *shared.progress.remaining.lock().unwrap() = shared.remaining_ranges();
                if n % (SAVE_EVERY.as_millis() as u64 / 250) == 0 {
                    let _ = save();
                }
            }
        })
    };

    let mut workers = JoinSet::new();
    for _ in 0..req.connections.max(1) {
        workers.spawn(worker(shared.clone()));
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
    progress.downloaded.store(size - shared.remaining_bytes(), Ordering::Relaxed);
    *progress.remaining.lock().unwrap() = shared.remaining_ranges();

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

/// One connection's life: take a segment, download it, take another (or
/// steal half of someone else's), until there is nothing worth taking.
async fn worker(shared: Arc<Shared>) -> Result<()> {
    while let Some(seg) = shared.claim() {
        shared.progress.connections.fetch_add(1, Ordering::Relaxed);
        let result = download_segment(&shared, &seg).await;
        shared.progress.connections.fetch_sub(1, Ordering::Relaxed);
        result?;
    }
    Ok(())
}

/// Downloads one segment, reconnecting from where it left off on errors.
/// The retry counter resets whenever an attempt makes progress, so only a
/// connection that is truly stuck gives up.
async fn download_segment(shared: &Shared, seg: &Segment) -> Result<()> {
    let mut failures = 0u32;
    loop {
        let (pos, end) = seg.range();
        if pos >= end {
            return Ok(());
        }
        let result = fetch_range(shared, seg, pos, end).await;
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

async fn fetch_range(shared: &Shared, seg: &Segment, pos: u64, end: u64) -> Result<()> {
    let resp = shared
        .client
        .get(&shared.url)
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
pub fn saved_remaining(dest: &Path) -> Option<Vec<(u64, u64)>> {
    SavedState::load(&append_ext(dest, "swiftdl")).map(|s| s.segments)
}

fn append_ext(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}
