//! ftp:// downloads: one connection, resumable (the server is asked to carry
//! on from the byte it stopped at). Plain FTP; user and password may be in the
//! address (ftp://user:pass@host/file), otherwise it logs in anonymously.

use anyhow::{Context, Result, bail};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use suppaftp::{FtpStream, types::FileType};
use swiftdl_engine::{CancellationToken, Outcome, Progress};

pub fn is_ftp(url: &str) -> bool {
    url.len() > 6 && url[..6].eq_ignore_ascii_case("ftp://")
}

struct Target {
    addr: String,
    user: String,
    pass: String,
    path: String,
    name: String,
}

fn parse(url: &str) -> Result<Target> {
    let u = url::Url::parse(url).context("not a valid ftp:// address")?;
    let host = u.host_str().context("the address has no server")?;
    let decode = |s: &str| percent_encoding::percent_decode_str(s).decode_utf8_lossy().into_owned();
    let path = decode(u.path());
    let name = path.rsplit('/').find(|p| !p.is_empty()).unwrap_or("").to_string();
    if name.is_empty() {
        bail!("the address doesn't point to a file");
    }
    Ok(Target {
        addr: format!("{host}:{}", u.port().unwrap_or(21)),
        user: if u.username().is_empty() { "anonymous".into() } else { decode(u.username()) },
        pass: u.password().map(decode).unwrap_or_else(|| "swiftdl@".into()),
        path,
        name,
    })
}

fn connect(t: &Target) -> Result<FtpStream> {
    use std::net::ToSocketAddrs;
    let addr = t.addr.to_socket_addrs().with_context(|| format!("couldn't find the server {}", t.addr))?.next().context("the server has no address")?;
    let mut ftp = FtpStream::connect_timeout(addr, Duration::from_secs(15)).with_context(|| format!("couldn't connect to {}", t.addr))?;
    ftp.login(&t.user, &t.pass).context("the server refused the login")?;
    // Extended passive (EPSV) works through more firewalls and on IPv6; the
    // library falls back to plain passive for old servers.
    ftp.set_mode(suppaftp::Mode::ExtendedPassive);
    ftp.transfer_type(FileType::Binary)?;
    Ok(ftp)
}

/// File name and size, for the New download window.
pub fn probe(url: &str) -> Result<(String, Option<u64>)> {
    let t = parse(url)?;
    let mut ftp = connect(&t)?;
    let size = ftp.size(&t.path).ok().map(|s| s as u64);
    let _ = ftp.quit();
    Ok((t.name, size))
}

pub async fn run(url: String, dest: PathBuf, progress: Arc<Progress>, cancel: CancellationToken) -> Result<Outcome> {
    tauri::async_runtime::spawn_blocking(move || download(&url, &dest, &progress, &cancel)).await?
}

fn part_of(dest: &Path) -> PathBuf {
    swiftdl_engine::part_path(dest)
}

fn download(url: &str, dest: &Path, progress: &Progress, cancel: &CancellationToken) -> Result<Outcome> {
    let t = parse(url)?;
    let mut ftp = connect(&t)?;
    let total = ftp.size(&t.path).context("the server doesn't have that file")? as u64;
    progress.total.store(total, Ordering::Relaxed);
    progress.resumable.store(true, Ordering::Relaxed);
    if let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_of(dest);
    // Carry on from what is already on disk.
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let offset = if have > 0 && have < total { have } else { 0 };
    let mut file = if offset > 0 { std::fs::OpenOptions::new().append(true).open(&part)? } else { std::fs::File::create(&part)? };
    if offset > 0 {
        ftp.resume_transfer(offset as usize).context("the server can't resume")?;
    }
    progress.downloaded.store(offset, Ordering::Relaxed);
    progress.connections.store(1, Ordering::Relaxed);
    let mut data = ftp.retr_as_stream(&t.path).context("the server wouldn't send the file")?;
    let mut buf = vec![0u8; 256 * 1024];
    let paused = loop {
        if cancel.is_cancelled() {
            break true;
        }
        let n = data.read(&mut buf).context("the connection broke")?;
        if n == 0 {
            break false;
        }
        file.write_all(&buf[..n])?;
        progress.downloaded.fetch_add(n as u64, Ordering::Relaxed);
    };
    file.flush()?;
    progress.connections.store(0, Ordering::Relaxed);
    if paused {
        drop(data); // hang up; the next run asks to carry on from here
        return Ok(Outcome::Paused);
    }
    let _ = ftp.finalize_retr_stream(data);
    let _ = ftp.quit();
    drop(file);
    let got = std::fs::metadata(&part)?.len();
    if got != total {
        bail!("the server stopped early ({got} of {total} bytes). Retry to carry on.");
    }
    std::fs::rename(&part, dest)?;
    Ok(Outcome::Completed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_addresses() {
        assert!(is_ftp("ftp://x/y") && is_ftp("FTP://x/y") && !is_ftp("https://x/y"));
        let t = parse("ftp://bob:p%40ss@files.example.com:2121/pub/My%20File.zip").unwrap();
        assert_eq!((t.addr.as_str(), t.user.as_str(), t.pass.as_str()), ("files.example.com:2121", "bob", "p@ss"));
        assert_eq!((t.path.as_str(), t.name.as_str()), ("/pub/My File.zip", "My File.zip"));
        let a = parse("ftp://files.example.com/a.iso").unwrap();
        assert_eq!((a.addr.as_str(), a.user.as_str()), ("files.example.com:21", "anonymous"));
        assert!(parse("ftp://files.example.com/").is_err(), "a folder isn't a file");
    }

    /// `SWIFTDL_FTP_URL=ftp://... SWIFTDL_FTP_OUT=... cargo test live_ftp -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_ftp() {
        let url = std::env::var("SWIFTDL_FTP_URL").unwrap();
        let out = PathBuf::from(std::env::var("SWIFTDL_FTP_OUT").unwrap());
        println!("probe: {:?}", probe(&url).unwrap());
        let progress = Arc::new(Progress::default());
        // Stop after a moment, then carry on: proves resuming.
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let p2 = progress.clone();
        std::thread::spawn(move || {
            while p2.downloaded.load(Ordering::Relaxed) < 100_000 {
                std::thread::sleep(Duration::from_millis(20));
            }
            c2.cancel();
        });
        let first = download(&url, &out, &progress, &cancel).unwrap();
        println!("first run: {first:?} with {} bytes", progress.downloaded.load(Ordering::Relaxed));
        let second = download(&url, &out, &Progress::default(), &CancellationToken::new()).unwrap();
        println!("second run: {second:?}, file {} bytes", std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0));
    }
}
