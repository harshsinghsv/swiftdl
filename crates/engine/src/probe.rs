//! Probing: one small request that tells us how big the file is, whether the
//! server supports byte ranges (i.e. we can split it), and what to call it.

use anyhow::{Context, Result, bail};
use percent_encoding::percent_decode_str;
use reqwest::{Client, StatusCode, header};

#[derive(Debug, Clone)]
pub struct RemoteFile {
    /// URL after following redirects; segment requests go straight here.
    pub url: String,
    pub size: Option<u64>,
    /// True when the server honours `Range` requests and reports a total size.
    pub resumable: bool,
    pub filename: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// Google Drive's normal share links answer with a "can't scan for viruses"
/// web page, not the file. This address serves the file itself.
fn direct_url(url: &str) -> String {
    let Ok(u) = reqwest::Url::parse(url) else { return url.to_string() };
    if u.host_str() != Some("drive.google.com") {
        return url.to_string();
    }
    let id = u.query_pairs().find(|(k, _)| k == "id").map(|(_, v)| v.into_owned()).or_else(|| {
        let mut parts = u.path_segments()?;
        (parts.next()? == "file" && parts.next()? == "d").then(|| parts.next().map(str::to_string)).flatten()
    });
    match id.filter(|i| !i.is_empty() && i.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')) {
        Some(id) => format!("https://drive.usercontent.google.com/download?id={id}&export=download&confirm=t"),
        None => url.to_string(),
    }
}

/// Asks for byte 0 only. A `206` with `Content-Range: bytes 0-0/N` proves
/// range support and gives the size. A `200` means the server ignored the
/// range, so we must download in a single stream.
pub async fn probe(client: &Client, url: &str, headers: &header::HeaderMap) -> Result<RemoteFile> {
    let url = &direct_url(url);
    let resp = client
        .get(url)
        .headers(headers.clone())
        .header(header::RANGE, "bytes=0-0")
        .send()
        .await
        .with_context(|| format!("could not connect to {url}"))?;

    let status = resp.status();
    if !status.is_success() {
        bail!("server returned {status}");
    }

    let h = resp.headers();
    let (size, resumable) = if status == StatusCode::PARTIAL_CONTENT {
        let total = header_str(h, header::CONTENT_RANGE)
            .and_then(|s| s.rsplit('/').next())
            .and_then(|s| s.trim().parse::<u64>().ok());
        (total, total.is_some())
    } else {
        (resp.content_length(), false)
    };

    let final_url = resp.url().clone();
    let filename = header_str(h, header::CONTENT_DISPOSITION)
        .and_then(filename_from_disposition)
        .or_else(|| filename_from_url(&final_url))
        .map(|n| sanitize(&n))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "download.bin".to_string());

    // A web page where a file should be: a sign-in or "are you sure" page.
    // Saving it as the download would only leave a small broken file.
    let ctype = header_str(h, header::CONTENT_TYPE).unwrap_or("").to_ascii_lowercase();
    let attachment = header_str(h, header::CONTENT_DISPOSITION).is_some_and(|d| d.to_ascii_lowercase().contains("attachment"));
    let wants_html = [".html", ".htm", ".xhtml"].iter().any(|e| filename.to_ascii_lowercase().ends_with(e));
    if ctype.starts_with("text/html") && !attachment && !wants_html {
        bail!("This link opens a web page, not a file. The site may need you to sign in or confirm first.");
    }

    Ok(RemoteFile {
        url: final_url.to_string(),
        size,
        resumable,
        filename,
        etag: header_str(h, header::ETAG).map(str::to_string),
        last_modified: header_str(h, header::LAST_MODIFIED).map(str::to_string),
    })
}

fn header_str(h: &header::HeaderMap, name: header::HeaderName) -> Option<&str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// Handles both `filename*=UTF-8''na%20me.zip` (RFC 6266/5987) and
/// `filename="name.zip"`, preferring the encoded form.
fn filename_from_disposition(value: &str) -> Option<String> {
    let params: Vec<&str> = value.split(';').map(str::trim).collect();
    for p in &params {
        if p.to_ascii_lowercase().starts_with("filename*=") {
            let raw = &p["filename*=".len()..];
            let encoded = raw.splitn(3, '\'').nth(2).unwrap_or(raw);
            return Some(percent_decode_str(encoded.trim_matches('"')).decode_utf8_lossy().into_owned());
        }
    }
    for p in &params {
        if p.to_ascii_lowercase().starts_with("filename=") {
            return Some(p["filename=".len()..].trim_matches('"').to_string());
        }
    }
    None
}

fn filename_from_url(url: &reqwest::Url) -> Option<String> {
    let last = url.path_segments()?.filter(|s| !s.is_empty()).last()?;
    Some(percent_decode_str(last).decode_utf8_lossy().into_owned())
}

/// Strips characters Windows forbids in file names, plus any path parts.
fn sanitize(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.chars()
        .map(|c| if "<>:\"|?*".contains(c) || c.is_control() { '_' } else { c })
        .collect::<String>()
        .trim()
        .trim_end_matches('.')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_links_go_to_the_file() {
        let want = "https://drive.usercontent.google.com/download?id=AbC_1-x&export=download&confirm=t";
        assert_eq!(direct_url("https://drive.google.com/uc?id=AbC_1-x&export=download"), want);
        assert_eq!(direct_url("https://drive.google.com/file/d/AbC_1-x/view?usp=sharing"), want);
        assert_eq!(direct_url("https://drive.google.com/open?id=AbC_1-x"), want);
        assert_eq!(direct_url("https://example.com/uc?id=AbC_1-x"), "https://example.com/uc?id=AbC_1-x");
        assert_eq!(direct_url("https://drive.google.com/drive/folders/zzz"), "https://drive.google.com/drive/folders/zzz");
    }

    #[test]
    fn parses_disposition() {
        assert_eq!(filename_from_disposition(r#"attachment; filename="a b.zip""#).as_deref(), Some("a b.zip"));
        assert_eq!(
            filename_from_disposition("attachment; filename=x.bin; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf").as_deref(),
            Some("résumé.pdf")
        );
        assert_eq!(filename_from_disposition("inline"), None);
    }

    #[test]
    fn sanitizes() {
        assert_eq!(sanitize("../evil/na:me?.txt"), "na_me_.txt");
        assert_eq!(sanitize("trailing..."), "trailing");
    }
}
