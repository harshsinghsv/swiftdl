//! Downloads HLS (.m3u8) and DASH (.mpd) streams itself, the way IDM does:
//! read the playlist or manifest, fetch its pieces many at a time with the
//! browser's cookies, and join them. No yt-dlp, so nothing to wait for before
//! the first byte.
//!
//! Pieces are saved one file each in `<dest>.parts/` while downloading, so a
//! paused or interrupted download carries on from the pieces it already has.

use aes::cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use anyhow::{Context, Result, anyhow, bail};
use futures_util::{StreamExt, stream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use swiftdl_engine::{CancellationToken, Client, Outcome, Progress, header::HeaderMap};
use url::Url;

/// Pieces fetched at once.
const PARALLEL: usize = 8;

#[derive(Clone, Debug, PartialEq)]
enum Key {
    None,
    /// AES-128-CBC: key URL and IV (None: the piece's sequence number).
    Aes128 { uri: String, iv: Option<[u8; 16]> },
}

#[derive(Clone, Debug)]
struct Piece {
    url: String,
    /// `(length, offset)` from #EXT-X-BYTERANGE.
    range: Option<(u64, u64)>,
    key: Key,
    seq: u64,
}

#[derive(Debug, Default)]
struct Playlist {
    /// #EXT-X-MAP: the fMP4 header that goes before the pieces.
    init: Option<Piece>,
    pieces: Vec<Piece>,
}

fn join(base: &Url, uri: &str) -> Result<String> {
    Ok(base.join(uri.trim()).with_context(|| format!("bad address in playlist: {uri}"))?.to_string())
}

/// Reads `NAME=value` and `NAME="value"` pairs from a tag's attribute list.
fn attr(line: &str, name: &str) -> Option<String> {
    let mut rest = line.split_once(':')?.1;
    while !rest.is_empty() {
        let (k, after) = rest.split_once('=')?;
        let (v, tail) = if let Some(q) = after.strip_prefix('"') {
            let end = q.find('"')?;
            (&q[..end], q[end + 1..].trim_start_matches(','))
        } else {
            match after.split_once(',') {
                Some((v, t)) => (v, t),
                None => (after, ""),
            }
        };
        if k.trim().eq_ignore_ascii_case(name) {
            return Some(v.to_string());
        }
        rest = tail;
    }
    None
}

fn parse_iv(s: &str) -> Option<[u8; 16]> {
    let hex = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    let n = u128::from_str_radix(&format!("{hex:0>32}"), 16).ok()?;
    Some(n.to_be_bytes())
}

fn parse_range(s: &str, next: u64) -> Option<(u64, u64)> {
    let (len, off) = match s.split_once('@') {
        Some((l, o)) => (l.trim().parse().ok()?, o.trim().parse().ok()?),
        None => (s.trim().parse().ok()?, next),
    };
    Some((len, off))
}

/// A media playlist: the list of pieces. A master playlist (qualities) is an
/// error here; the extension picks the quality before sending it.
fn parse(text: &str, base: &Url) -> Result<Playlist> {
    if !text.trim_start().starts_with("#EXTM3U") {
        bail!("this isn't an HLS playlist");
    }
    if text.contains("#EXT-X-STREAM-INF") {
        // Fall back to the highest quality when given the master playlist.
        let mut best: Option<(u64, String)> = None;
        let mut bw = 0;
        for line in text.lines().map(str::trim) {
            if line.starts_with("#EXT-X-STREAM-INF") {
                bw = attr(line, "BANDWIDTH").and_then(|b| b.parse().ok()).unwrap_or(0);
            } else if !line.is_empty() && !line.starts_with('#') && best.as_ref().is_none_or(|(b, _)| bw > *b) {
                best = Some((bw, join(base, line)?));
            }
        }
        bail!("master:{}", best.map(|b| b.1).unwrap_or_default());
    }
    let mut out = Playlist::default();
    let mut key = Key::None;
    let mut seq: u64 = 0;
    let mut range: Option<(u64, u64)> = None;
    let mut next_off = 0;
    for line in text.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            seq = v.trim().parse().unwrap_or(0);
        } else if line.starts_with("#EXT-X-KEY") {
            let method = attr(line, "METHOD").unwrap_or_default();
            key = match method.as_str() {
                "NONE" => Key::None,
                "AES-128" => Key::Aes128 { uri: join(base, &attr(line, "URI").context("key without address")?)?, iv: attr(line, "IV").and_then(|v| parse_iv(&v)) },
                // SAMPLE-AES and DRM: the browser decrypts these, we can't.
                other => bail!("this video is protected ({other}) and can't be downloaded"),
            };
        } else if line.starts_with("#EXT-X-MAP") {
            let uri = attr(line, "URI").context("map without address")?;
            let r = attr(line, "BYTERANGE").and_then(|r| parse_range(&r, 0));
            out.init = Some(Piece { url: join(base, &uri)?, range: r, key: key.clone(), seq: 0 });
        } else if let Some(v) = line.strip_prefix("#EXT-X-BYTERANGE:") {
            range = parse_range(v, next_off);
        } else if !line.is_empty() && !line.starts_with('#') {
            if let Some((len, off)) = range {
                next_off = off + len;
            }
            out.pieces.push(Piece { url: join(base, line)?, range: range.take(), key: key.clone(), seq });
            seq += 1;
        }
    }
    if out.pieces.is_empty() {
        bail!("the playlist has no video in it");
    }
    Ok(out)
}

struct Fetcher {
    client: Client,
    headers: HeaderMap,
}

impl Fetcher {
    async fn get(&self, url: &str, range: Option<(u64, u64)>) -> Result<Vec<u8>> {
        let mut last = None;
        // Pieces fail now and then on busy CDNs; a retry nearly always works.
        for attempt in 0..4u64 {
            if attempt > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(400 * attempt)).await;
            }
            let mut req = self.client.get(url).headers(self.headers.clone());
            if let Some((len, off)) = range {
                req = req.header("Range", format!("bytes={off}-{}", off + len - 1));
            }
            match req.send().await.and_then(|r| r.error_for_status()) {
                Ok(r) => match r.bytes().await {
                    Ok(b) => return Ok(b.to_vec()),
                    Err(e) => last = Some(anyhow!(e)),
                },
                Err(e) => {
                    // A 403/404 won't change by asking again.
                    if e.status().is_some_and(|s| s.is_client_error()) {
                        return Err(anyhow!(e));
                    }
                    last = Some(anyhow!(e));
                }
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("download failed")))
    }

    async fn text(&self, url: &str) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.get(url, None).await?).into_owned())
    }
}

/// One stream download: the video playlist, and optionally a separate audio
/// playlist, saved to `dest` (an .mp4).
pub struct Job {
    pub video: String,
    pub audio: Option<String>,
    pub dest: PathBuf,
    pub headers: HeaderMap,
}

/// What a `format` string means for a stream picked in the extension, as
/// `(video, audio)` sources:
/// - `hls|<video playlist>|<audio playlist>`
/// - `dash|<manifest>|<video representation id>|<audio representation id>`
///
/// Any part may be empty. A DASH source is `dash:<manifest>#<id>`.
pub fn parse_format(format: &str) -> Option<(Option<String>, Option<String>)> {
    let part = |s: Option<&str>| s.filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(rest) = format.strip_prefix("dash|") {
        let mut it = rest.splitn(3, '|');
        let mpd = it.next()?.to_string();
        let src = |id: Option<String>| id.map(|id| format!("dash:{mpd}#{id}"));
        return Some((src(part(it.next())), src(part(it.next()))));
    }
    let rest = format.strip_prefix("hls")?;
    let mut it = rest.strip_prefix('|').unwrap_or("").splitn(2, '|');
    Some((part(it.next()), part(it.next())))
}

// ---------- DASH ----------

/// `PT1H2M3.5S` and the like, in seconds.
fn iso_duration(s: &str) -> Option<f64> {
    let t = s.trim().strip_prefix('P')?;
    let (days, time) = match t.split_once('T') {
        Some((d, t)) => (d, t),
        None => (t, ""),
    };
    let mut secs = 0.0;
    let mut num = String::new();
    for c in days.chars() {
        if c == 'D' { secs += num.parse::<f64>().ok()? * 86400.0; num.clear(); } else { num.push(c); }
    }
    num.clear();
    for c in time.chars() {
        match c {
            'H' => { secs += num.parse::<f64>().ok()? * 3600.0; num.clear(); }
            'M' => { secs += num.parse::<f64>().ok()? * 60.0; num.clear(); }
            'S' => { secs += num.parse::<f64>().ok()?; num.clear(); }
            _ => num.push(c),
        }
    }
    Some(secs)
}

/// Fills in `$RepresentationID$`, `$Number%05d$`, `$Time$` and `$Bandwidth$`.
fn fill(template: &str, id: &str, bandwidth: u64, number: u64, time: u64) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let Some(j) = after.find('$') else { out.push_str(&rest[i..]); return out };
        let tag = &after[..j];
        let (name, width) = match tag.split_once('%') {
            Some((n, f)) => (n, f.trim_start_matches('0').trim_end_matches('d').parse::<usize>().unwrap_or(0)),
            None => (tag, 0),
        };
        let num = |v: u64| format!("{v:0width$}");
        match name {
            "" => out.push('$'),
            "RepresentationID" => out.push_str(id),
            "Number" => out.push_str(&num(number)),
            "Time" => out.push_str(&num(time)),
            "Bandwidth" => out.push_str(&num(bandwidth)),
            _ => { out.push('$'); out.push_str(tag); out.push('$'); }
        }
        rest = &after[j + 1..];
    }
    out.push_str(rest);
    out
}

fn byte_range(r: &str) -> Option<(u64, u64)> {
    let (a, b) = r.split_once('-')?;
    let (a, b): (u64, u64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    Some((b - a + 1, a))
}

/// The first child element called `tag`.
fn el<'a, 'i>(n: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == tag)
}

/// The pieces of one representation of a DASH manifest. `Err("whole:<url>")`
/// means it is one plain file (SegmentBase), which is fetched in chunks.
fn parse_dash(text: &str, base: &Url, rep_id: &str) -> Result<Playlist> {
    let doc = roxmltree::Document::parse(text).context("the stream's manifest isn't valid XML")?;
    let root = doc.root_element();
    if root.attribute("type") == Some("dynamic") {
        bail!("live streams can't be downloaded yet");
    }
    let total = root.attribute("mediaPresentationDuration").and_then(iso_duration).unwrap_or(0.0);
    let rep = root
        .descendants()
        .find(|n| n.tag_name().name() == "Representation" && n.attribute("id") == Some(rep_id))
        .with_context(|| format!("the stream has no track {rep_id}"))?;
    let set = rep.parent().context("track outside an adaptation set")?;
    let period = set.parent().context("adaptation set outside a period")?;
    let period_secs = period.attribute("duration").and_then(iso_duration).unwrap_or(total);
    // BaseURLs stack from the manifest down to the track.
    let mut url = base.clone();
    for n in [root, period, set, rep] {
        if let Some(b) = el(n, "BaseURL").and_then(|b| b.text()) {
            url = url.join(b.trim())?;
        }
    }
    let id = rep.attribute("id").unwrap_or_default();
    let bw: u64 = rep.attribute("bandwidth").and_then(|b| b.parse().ok()).unwrap_or(0);
    let piece = |u: String, range: Option<(u64, u64)>, seq: u64| Piece { url: u, range, key: Key::None, seq };
    let mut out = Playlist::default();

    // Segment information lives on the track, or else on its adaptation set.
    let find = |tag: &str| el(rep, tag).or_else(|| el(set, tag)).or_else(|| el(period, tag));
    if let Some(t) = find("SegmentTemplate") {
        let attr = |k: &str| t.attribute(k).or_else(|| el(set, "SegmentTemplate").and_then(|s| s.attribute(k)));
        let timescale: u64 = attr("timescale").and_then(|v| v.parse().ok()).unwrap_or(1);
        let start: u64 = attr("startNumber").and_then(|v| v.parse().ok()).unwrap_or(1);
        let media = attr("media").context("segment template without media")?;
        if let Some(init) = attr("initialization") {
            out.init = Some(piece(url.join(&fill(init, id, bw, 0, 0))?.to_string(), None, 0));
        }
        let timeline = el(t, "SegmentTimeline").or_else(|| el(set, "SegmentTemplate").and_then(|s| el(s, "SegmentTimeline")));
        let mut number = start;
        if let Some(tl) = timeline {
            let mut time = 0u64;
            let end = (period_secs * timescale as f64) as u64;
            for s in tl.children().filter(|c| c.tag_name().name() == "S") {
                if let Some(t) = s.attribute("t").and_then(|v| v.parse().ok()) {
                    time = t;
                }
                let d: u64 = s.attribute("d").and_then(|v| v.parse().ok()).context("timeline entry without length")?;
                let r: i64 = s.attribute("r").and_then(|v| v.parse().ok()).unwrap_or(0);
                // r = -1: repeat until the end of the period.
                let repeats = if r < 0 { if d > 0 { end.saturating_sub(time).div_ceil(d) } else { 0 } } else { r as u64 + 1 };
                for _ in 0..repeats {
                    out.pieces.push(piece(url.join(&fill(media, id, bw, number, time))?.to_string(), None, number));
                    time += d;
                    number += 1;
                }
            }
        } else {
            let d: f64 = attr("duration").and_then(|v| v.parse().ok()).context("segment template without length")?;
            let count = ((period_secs * timescale as f64) / d).ceil() as u64;
            if count == 0 {
                bail!("the stream's length is unknown");
            }
            for _ in 0..count {
                out.pieces.push(piece(url.join(&fill(media, id, bw, number, 0))?.to_string(), None, number));
                number += 1;
            }
        }
    } else if let Some(list) = find("SegmentList") {
        if let Some(init) = el(list, "Initialization") {
            let u = init.attribute("sourceURL").map(|u| url.join(u)).transpose()?.unwrap_or_else(|| url.clone());
            out.init = Some(piece(u.to_string(), init.attribute("range").and_then(byte_range), 0));
        }
        for (i, s) in list.children().filter(|c| c.tag_name().name() == "SegmentURL").enumerate() {
            let u = s.attribute("media").map(|u| url.join(u)).transpose()?.unwrap_or_else(|| url.clone());
            out.pieces.push(piece(u.to_string(), s.attribute("mediaRange").and_then(byte_range), i as u64));
        }
    } else {
        // SegmentBase, or nothing: the track is one plain file.
        bail!("whole:{url}");
    }
    if out.pieces.is_empty() {
        bail!("the stream has no video in it");
    }
    Ok(out)
}

/// Chunk size for a plain file split across connections.
const CHUNK: u64 = 4 << 20;

/// A plain file as byte-range pieces, so it downloads many at a time too.
async fn chunks(f: &Fetcher, url: &str) -> Result<Playlist> {
    let r = f.client.get(url).headers(f.headers.clone()).header("Range", "bytes=0-0").send().await?.error_for_status()?;
    let total = r.headers().get("content-range").and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit('/').next()).and_then(|v| v.parse::<u64>().ok());
    let piece = |range| Piece { url: url.to_string(), range, key: Key::None, seq: 0 };
    let pieces = match total {
        Some(total) => (0..total.div_ceil(CHUNK)).map(|i| piece(Some((CHUNK.min(total - i * CHUNK), i * CHUNK)))).collect(),
        // No ranges: one piece, one connection.
        None => vec![piece(None)],
    };
    Ok(Playlist { init: None, pieces })
}

async fn load(f: &Fetcher, url: &str) -> Result<Playlist> {
    if let Some(src) = url.strip_prefix("dash:") {
        let (mpd, id) = src.rsplit_once('#').context("bad stream address")?;
        let text = f.text(mpd).await.context("couldn't read the stream's manifest")?;
        return match parse_dash(&text, &Url::parse(mpd)?, id) {
            Err(e) if e.to_string().starts_with("whole:") => chunks(f, &e.to_string()["whole:".len()..]).await,
            other => other,
        };
    }
    let mut url = url.to_string();
    for _ in 0..2 {
        let text = f.text(&url).await.context("couldn't read the stream's playlist")?;
        match parse(&text, &Url::parse(&url)?) {
            Err(e) if e.to_string().starts_with("master:") => url = e.to_string()["master:".len()..].to_string(),
            other => return other,
        }
    }
    bail!("couldn't find the video in the playlist")
}

pub async fn run(client: &Client, job: Job, progress: Arc<Progress>, note: crate::media::Note, cancel: CancellationToken) -> Result<Outcome> {
    let f = Arc::new(Fetcher { client: client.clone(), headers: job.headers.clone() });
    let set = |t: &str| *note.lock().unwrap() = Some(crate::i18n::note(t));
    // Joining needs ffmpeg; fetch it now (first time only) if it's missing.
    crate::media::ensure_ffmpeg(client, &note).await?;
    set("Starting");
    let video = load(&f, &job.video).await?;
    let audio = match &job.audio {
        Some(a) => Some(load(&f, a).await?),
        None => None,
    };
    *note.lock().unwrap() = None;

    let parts = job.dest.with_extension("parts");
    std::fs::create_dir_all(&parts)?;
    // Every piece of both playlists, numbered so they sort back into order.
    let mut all: Vec<(PathBuf, Piece)> = vec![];
    for (tag, list) in [("v", Some(&video)), ("a", audio.as_ref())] {
        let Some(list) = list else { continue };
        if let Some(init) = &list.init {
            all.push((parts.join(format!("{tag}-init")), init.clone()));
        }
        for (i, p) in list.pieces.iter().enumerate() {
            all.push((parts.join(format!("{tag}-{i:06}")), p.clone()));
        }
    }

    // Progress in bytes: done pieces exactly, the rest estimated from them.
    let count = all.len() as u64;
    let done_bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let done_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    for (path, _) in &all {
        if let Ok(m) = std::fs::metadata(path) {
            done_bytes.fetch_add(m.len(), Ordering::Relaxed);
            done_count.fetch_add(1, Ordering::Relaxed);
        }
    }
    let report = {
        let (progress, done_bytes, done_count) = (progress.clone(), done_bytes.clone(), done_count.clone());
        move || {
            let (b, n) = (done_bytes.load(Ordering::Relaxed), done_count.load(Ordering::Relaxed));
            progress.downloaded.store(b, Ordering::Relaxed);
            if n > 0 {
                progress.total.store((b as f64 / n as f64 * count as f64) as u64, Ordering::Relaxed);
            }
        }
    };
    report();
    progress.connections.store(PARALLEL, Ordering::Relaxed);

    let keys = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::<String, Vec<u8>>::new()));
    let todo: Vec<_> = all.iter().filter(|(p, _)| !p.exists()).cloned().collect();
    let mut tasks = stream::iter(todo.into_iter().map(|(path, piece)| {
        let (f, keys, cancel) = (f.clone(), keys.clone(), cancel.clone());
        async move {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            let mut data = f.get(&piece.url, piece.range).await.with_context(|| format!("couldn't download a piece of the video ({})", piece.url))?;
            if let Key::Aes128 { uri, iv } = &piece.key {
                let key = {
                    let mut k = keys.lock().await;
                    if !k.contains_key(uri) {
                        let got = f.get(uri, None).await.context("couldn't get the video's key")?;
                        k.insert(uri.clone(), got);
                    }
                    k[uri].clone()
                };
                let iv = iv.unwrap_or_else(|| (piece.seq as u128).to_be_bytes());
                let dec = cbc::Decryptor::<aes::Aes128>::new_from_slices(&key, &iv).map_err(|_| anyhow!("the video's key is invalid"))?;
                data = dec.decrypt_padded_vec_mut::<Pkcs7>(&data).map_err(|_| anyhow!("couldn't decrypt a piece of the video"))?;
            }
            // Writing can stall (antivirus scans each new file); keep it off
            // the downloads so the other pieces keep coming.
            let n = data.len() as u64;
            tauri::async_runtime::spawn_blocking(move || -> std::io::Result<()> {
                let tmp = path.with_extension("tmp");
                std::fs::write(&tmp, &data)?;
                std::fs::rename(&tmp, &path)
            })
            .await??;
            Ok::<_, anyhow::Error>(Some(n))
        }
    }))
    .buffer_unordered(PARALLEL);

    while let Some(r) = tasks.next().await {
        if let Some(n) = r? {
            done_bytes.fetch_add(n, Ordering::Relaxed);
            done_count.fetch_add(1, Ordering::Relaxed);
            report();
        }
        if cancel.is_cancelled() {
            break;
        }
    }
    drop(tasks);
    progress.connections.store(0, Ordering::Relaxed);
    if cancel.is_cancelled() {
        return Ok(Outcome::Paused);
    }

    set("Joining");
    let joined = |tag: &str| -> Result<PathBuf> {
        let out = parts.join(format!("{tag}.joined"));
        let mut w = std::io::BufWriter::new(std::fs::File::create(&out)?);
        for (path, _) in all.iter().filter(|(p, _)| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{tag}-")))) {
            std::io::copy(&mut std::fs::File::open(path)?, &mut w)?;
        }
        std::io::Write::flush(&mut w)?;
        Ok(out)
    };
    let v = joined("v")?;
    let a = if audio.is_some() { Some(joined("a")?) } else { None };
    finish(&v, a.as_deref(), &job.dest).await?;
    let _ = std::fs::remove_dir_all(&parts);
    *note.lock().unwrap() = None;
    let size = std::fs::metadata(&job.dest).map(|m| m.len()).unwrap_or(0);
    progress.total.store(size, Ordering::Relaxed);
    progress.downloaded.store(size, Ordering::Relaxed);
    Ok(Outcome::Completed)
}

/// Turns the joined pieces into one .mp4. ffmpeg only copies (no
/// re-encoding), so this takes seconds. Without ffmpeg, a video-only stream is
/// kept as it came (.ts plays in VLC and most players).
async fn finish(video: &Path, audio: Option<&Path>, dest: &Path) -> Result<()> {
    let ffmpeg = crate::media::ffmpeg_path();
    match ffmpeg {
        Some(ff) => {
            let (video, audio, dest) = (video.to_path_buf(), audio.map(Path::to_path_buf), dest.to_path_buf());
            let out = tauri::async_runtime::spawn_blocking(move || {
                let mut cmd = crate::media::quiet_command(&ff);
                cmd.args(["-y", "-loglevel", "error", "-i"]).arg(&video);
                if let Some(a) = &audio {
                    cmd.arg("-i").arg(a).args(["-map", "0:v?", "-map", "0:a?", "-map", "1:a"]);
                }
                cmd.args(["-c", "copy", "-bsf:a", "aac_adtstoasc", "-movflags", "+faststart"]).arg(&dest);
                cmd.output()
            })
            .await??;
            if !out.status.success() {
                bail!("couldn't join the video: {}", String::from_utf8_lossy(&out.stderr).lines().last().unwrap_or("ffmpeg failed"));
            }
        }
        None if audio.is_none() => {
            std::fs::rename(video, dest.with_extension("ts"))?;
        }
        None => bail!("Joining video and audio needs ffmpeg."),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_media_playlist() {
        let base = Url::parse("https://cdn.example/v/720/index.m3u8").unwrap();
        let p = parse(
            "#EXTM3U\n#EXT-X-MEDIA-SEQUENCE:5\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.bin\"\n#EXT-X-MAP:URI=\"init.mp4\"\n#EXTINF:4,\nseg0.m4s\n#EXTINF:4,\nhttps://other.example/seg1.m4s\n#EXT-X-ENDLIST\n",
            &base,
        )
        .unwrap();
        assert_eq!(p.init.unwrap().url, "https://cdn.example/v/720/init.mp4");
        assert_eq!(p.pieces.len(), 2);
        assert_eq!(p.pieces[0].seq, 5);
        assert_eq!(p.pieces[1].url, "https://other.example/seg1.m4s");
        assert_eq!(p.pieces[0].key, Key::Aes128 { uri: "https://cdn.example/v/720/k.bin".into(), iv: None });
    }

    #[test]
    fn master_points_to_best() {
        let base = Url::parse("https://cdn.example/master.m3u8").unwrap();
        let e = parse("#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=800000,RESOLUTION=640x360\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=3000000,RESOLUTION=1280x720\nhi.m3u8\n", &base).unwrap_err();
        assert_eq!(e.to_string(), "master:https://cdn.example/hi.m3u8");
    }

    #[test]
    fn reads_dash() {
        let base = Url::parse("https://cdn.example/v/manifest.mpd").unwrap();
        let mpd = r#"<MPD type="static" mediaPresentationDuration="PT0M10S"><Period>
          <AdaptationSet mimeType="video/mp4">
            <SegmentTemplate timescale="1000" duration="4000" startNumber="1" initialization="$RepresentationID$/init.mp4" media="$RepresentationID$/seg-$Number%03d$.m4s"/>
            <Representation id="720" bandwidth="2000000" height="720"/>
          </AdaptationSet>
          <AdaptationSet mimeType="audio/mp4">
            <Representation id="a1" bandwidth="128000"><SegmentTemplate timescale="10" initialization="a/init.mp4" media="a/$Time$.m4s"><SegmentTimeline><S t="0" d="40" r="1"/><S d="20"/></SegmentTimeline></SegmentTemplate></Representation>
          </AdaptationSet>
          <AdaptationSet mimeType="video/mp4"><Representation id="whole" bandwidth="1"><BaseURL>files/v.mp4</BaseURL><SegmentBase indexRange="0-99"/></Representation></AdaptationSet>
        </Period></MPD>"#;
        let v = parse_dash(mpd, &base, "720").unwrap();
        assert_eq!(v.init.unwrap().url, "https://cdn.example/v/720/init.mp4");
        assert_eq!(v.pieces.len(), 3);
        assert_eq!(v.pieces[2].url, "https://cdn.example/v/720/seg-003.m4s");
        let a = parse_dash(mpd, &base, "a1").unwrap();
        let urls: Vec<_> = a.pieces.iter().map(|p| p.url.rsplit('/').next().unwrap().to_string()).collect();
        assert_eq!(urls, ["0.m4s", "40.m4s", "80.m4s"]);
        assert_eq!(parse_dash(mpd, &base, "whole").unwrap_err().to_string(), "whole:https://cdn.example/v/files/v.mp4");
        assert_eq!(iso_duration("PT1H2M3.5S"), Some(3723.5));
        assert_eq!(parse_format("dash|https://x/m.mpd|720|a1"), Some((Some("dash:https://x/m.mpd#720".into()), Some("dash:https://x/m.mpd#a1".into()))));
    }

    #[test]
    fn reads_attributes() {
        assert_eq!(attr("#EXT-X-KEY:METHOD=AES-128,URI=\"a,b\",IV=0x1", "IV").as_deref(), Some("0x1"));
        assert_eq!(attr("#EXT-X-KEY:METHOD=AES-128,URI=\"a,b\",IV=0x1", "URI").as_deref(), Some("a,b"));
        assert_eq!(parse_format("hls|https://a/v.m3u8|https://a/a.m3u8"), Some((Some("https://a/v.m3u8".into()), Some("https://a/a.m3u8".into()))));
        assert_eq!(parse_format("hls|https://a/v.m3u8|"), Some((Some("https://a/v.m3u8".into()), None)));
        assert_eq!(parse_format("hls"), Some((None, None)));
        assert_eq!(parse_format("bv*+ba"), None);
    }
}

#[cfg(test)]
mod live {
    /// Downloads a real public stream: `cargo test live_hls -- --ignored --nocapture`
    /// with SWIFTDL_HLS_URL and SWIFTDL_HLS_OUT set.
    #[test]
    #[ignore]
    fn live_hls() {
        let url = std::env::var("SWIFTDL_HLS_URL").unwrap();
        let out = std::path::PathBuf::from(std::env::var("SWIFTDL_HLS_OUT").unwrap());
        let client = swiftdl_engine::build_client().unwrap();
        let progress = std::sync::Arc::new(swiftdl_engine::Progress::default());
        let start = std::time::Instant::now();
        let job = super::Job { video: url, audio: std::env::var("SWIFTDL_HLS_AUDIO").ok(), dest: out.clone(), headers: Default::default() };
        let r = tauri::async_runtime::block_on(super::run(&client, job, progress, Default::default(), Default::default())).unwrap();
        println!("{r:?} in {:.1}s, {} bytes", start.elapsed().as_secs_f64(), std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0));
    }
}
