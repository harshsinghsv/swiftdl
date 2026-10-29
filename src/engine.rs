//! The download engine: runs N connections in parallel, writes every chunk
//! straight to its final offset, and keeps every connection busy until the
//! very end by splitting the largest unfinished segment whenever one frees up.

use crate::probe::{RemoteFile, probe};
use crate::segment::{Segment, initial_segments};
use crate::state::SavedState;
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::{Client, StatusCode, header};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::task::JoinSet;

/// Never split a segment into halves smaller than this; a new connection's
/// handshake costs more than it saves below ~512 KiB.
const MIN_SPLIT_HALF: u64 = 512 * 1024;
const MAX_RETRIES: u32 = 8;
const SAVE_EVERY: Duration = Duration::from_secs(1);

pub struct Options {
    pub url: String,
    pub output: Option<PathBuf>,
    pub connections: usize,
    pub headers: header::HeaderMap,
    pub restart: bool,
}

pub enum Outcome {
    Completed(PathBuf),
    Paused,
}

struct Shared {
    client: Client,
    url: String,
    headers: header::HeaderMap,
    file: File,
    table: Mutex<Table>,
    active: AtomicUsize,
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

pub async fn download(opts: Options) -> Result<Outcome> {
    let client = build_client()?;
    let remote = probe(&client, &opts.url, &opts.headers).await?;
    let out = output_path(&opts, &remote);
    let part = append_ext(&out, "part");
    let state_path = append_ext(&out, "swiftdl");

    match (remote.resumable, remote.size) {
        (true, Some(size)) => {
            download_segmented(client, &opts, &remote, size, &out, &part, &state_path).await
        }
        _ => {
            eprintln!("Server does not support ranges: using one connection, no resume.");
            download_single(&client, &opts, &remote, &part).await?;
            finish(&part, &out)?;
            Ok(Outcome::Completed(out))
        }
    }
}

async fn download_segmented(
    client: Client,
    opts: &Options,
    remote: &RemoteFile,
    size: u64,
    out: &Path,
    part: &Path,
    state_path: &Path,
) -> Result<Outcome> {
    let saved = (!opts.restart && part.exists())
        .then(|| SavedState::load(state_path))
        .flatten()
        .filter(|s| s.matches(&opts.url, size, &remote.etag, &remote.last_modified));

    let (file, ranges) = match saved {
        Some(s) => {
            let file = OpenOptions::new().write(true).open(part)?;
            (file, s.segments)
        }
        None => {
            let file = OpenOptions::new().create(true).write(true).truncate(true).open(part)?;
            // Reserve the full size up front so segments can write anywhere
            // and there is no join step at the end.
            file.set_len(size).context("not enough disk space?")?;
            (file, initial_segments(size, opts.connections))
        }
    };

    let shared = Arc::new(Shared {
        client,
        url: remote.url.clone(),
        headers: opts.headers.clone(),
        file,
        table: Mutex::new(Table {
            segments: ranges.into_iter().map(|(p, e)| Arc::new(Segment::new(p, e))).collect(),
            next: 0,
        }),
        active: AtomicUsize::new(0),
    });

    let already = size - shared.remaining_bytes();
    print_header(remote, out, size, opts.connections, already);

    let save = {
        let meta = SavedState {
            url: opts.url.clone(),
            size,
            etag: remote.etag.clone(),
            last_modified: remote.last_modified.clone(),
            segments: vec![],
        };
        let shared = shared.clone();
        let state_path = state_path.to_path_buf();
        move || SavedState { segments: shared.remaining_ranges(), ..meta.clone() }.save(&state_path)
    };

    let saver = {
        let save = save.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(SAVE_EVERY);
            loop {
                tick.tick().await;
                let _ = save();
            }
        })
    };

    let bar = progress_bar(size, already);
    let ticker = {
        let shared = shared.clone();
        let bar = bar.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(200));
            loop {
                tick.tick().await;
                bar.set_position(size - shared.remaining_bytes());
                bar.set_message(format!("{} conns", shared.active.load(Ordering::Relaxed)));
            }
        })
    };

    let mut workers = JoinSet::new();
    for _ in 0..opts.connections.max(1) {
        workers.spawn(worker(shared.clone()));
    }

    let result: Result<bool> = tokio::select! {
        r = async {
            while let Some(res) = workers.join_next().await {
                res??;
            }
            Ok(true)
        } => r,
        _ = tokio::signal::ctrl_c() => Ok(false),
    };

    workers.shutdown().await;
    saver.abort();
    ticker.abort();
    let _ = saver.await;
    let _ = ticker.await;

    match result {
        Ok(true) if shared.remaining_bytes() == 0 => {
            bar.set_position(size);
            bar.finish_with_message("done");
            shared.file.sync_all()?;
            drop(shared);
            let _ = std::fs::remove_file(state_path);
            finish(part, out)?;
            Ok(Outcome::Completed(out.to_path_buf()))
        }
        Ok(true) => bail!("workers finished but data is missing (this is a bug)"),
        Ok(false) => {
            save()?;
            bar.abandon_with_message("paused");
            Ok(Outcome::Paused)
        }
        Err(e) => {
            save()?;
            bar.abandon_with_message("failed");
            Err(e.context("download stopped; progress saved, run the same command to resume"))
        }
    }
}

/// One connection's life: take a segment, download it, take another (or
/// steal half of someone else's), until there is nothing worth taking.
async fn worker(shared: Arc<Shared>) -> Result<()> {
    while let Some(seg) = shared.claim() {
        shared.active.fetch_add(1, Ordering::Relaxed);
        let result = download_segment(&shared, &seg).await;
        shared.active.fetch_sub(1, Ordering::Relaxed);
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
        if !seg.write(&shared.file, &chunk?)? {
            break; // segment complete (or its tail was stolen): hang up
        }
    }
    Ok(())
}

/// Fallback for servers without range support: one plain sequential stream.
async fn download_single(client: &Client, opts: &Options, remote: &RemoteFile, part: &Path) -> Result<()> {
    use std::io::Write;
    let resp = client.get(&remote.url).headers(opts.headers.clone()).send().await?.error_for_status()?;
    let mut file = std::io::BufWriter::new(File::create(part)?);
    let bar = match remote.size {
        Some(size) => progress_bar(size, 0),
        None => ProgressBar::new_spinner(),
    };
    let mut body = resp.bytes_stream();
    while let Some(chunk) = body.next().await {
        let chunk = chunk?;
        file.write_all(&chunk)?;
        bar.inc(chunk.len() as u64);
    }
    file.flush()?;
    bar.finish_with_message("done");
    Ok(())
}

fn finish(part: &Path, out: &Path) -> Result<()> {
    std::fs::rename(part, out).with_context(|| format!("could not rename to {}", out.display()))
}

/// Uses `-o` if given (a directory or a file path); otherwise the server's
/// file name in the current directory, adding " (1)", " (2)"... rather than
/// overwriting a finished file. An unfinished download keeps its name so it
/// can resume.
fn output_path(opts: &Options, remote: &RemoteFile) -> PathBuf {
    match &opts.output {
        Some(p) if p.is_dir() => unique(p.join(&remote.filename)),
        Some(p) => p.clone(),
        None => unique(PathBuf::from(&remote.filename)),
    }
}

fn unique(path: PathBuf) -> PathBuf {
    if !path.exists() || append_ext(&path, "part").exists() {
        return path;
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (1..)
        .map(|i| path.with_file_name(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

fn append_ext(path: &Path, ext: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".");
    s.push(ext);
    PathBuf::from(s)
}

fn progress_bar(size: u64, start: u64) -> ProgressBar {
    let bar = ProgressBar::new(size).with_position(start);
    bar.set_style(
        ProgressStyle::with_template(
            "{bar:40.cyan/blue} {bytes:>10}/{total_bytes} {binary_bytes_per_sec:>12}  ETA {eta:>4}  {msg}",
        )
        .unwrap()
        .progress_chars("█▉▊▋▌▍▎▏ "),
    );
    bar.enable_steady_tick(Duration::from_millis(200));
    bar
}

fn print_header(remote: &RemoteFile, out: &Path, size: u64, conns: usize, already: u64) {
    eprintln!("File:   {}", out.display());
    eprintln!("Size:   {} ({} bytes)", indicatif::HumanBytes(size), size);
    if already > 0 {
        eprintln!("Resume: {} already downloaded", indicatif::HumanBytes(already));
    }
    eprintln!("Conns:  {conns}, with dynamic segmentation");
    eprintln!("Source: {}", remote.url);
}
