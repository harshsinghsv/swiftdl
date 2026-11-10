//! Real-Debrid: a paid service that fetches files from file-hosting sites at
//! full speed and hands back a direct link. With an API token set, links from
//! the hosts it supports are turned into direct links before downloading.
//! https://api.real-debrid.com/

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use swiftdl_engine::{Client, header};

const API: &str = "https://api.real-debrid.com/rest/1.0";

#[derive(Debug, PartialEq)]
pub struct Direct {
    pub url: String,
    pub filename: String,
    pub size: Option<u64>,
}

/// The hosts Real-Debrid supports, kept for six hours.
static DOMAINS: LazyLock<Mutex<Option<(Instant, Vec<String>)>>> = LazyLock::new(Default::default);

async fn domains(client: &Client) -> Result<Vec<String>> {
    if let Some((at, list)) = DOMAINS.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(6 * 3600) {
            return Ok(list.clone());
        }
    }
    let text = client.get(format!("{API}/hosts/domains")).send().await?.error_for_status()?.text().await?;
    let list: Vec<String> = serde_json::from_str(&text).context("Real-Debrid sent an unexpected list of hosts")?;
    *DOMAINS.lock().unwrap() = Some((Instant::now(), list.clone()));
    Ok(list)
}

/// Whether `host` is one of `domains` or a subdomain of one.
fn supported(host: &str, domains: &[String]) -> bool {
    let host = host.to_ascii_lowercase();
    domains.iter().any(|d| {
        let d = d.to_ascii_lowercase();
        host == d || host.ends_with(&format!(".{d}"))
    })
}

fn parse_direct(body: &str) -> Result<Direct> {
    let v: Value = serde_json::from_str(body).context("Real-Debrid sent an unexpected answer")?;
    if let Some(err) = v["error"].as_str() {
        bail!("Real-Debrid: {}", match err {
            "bad_token" | "bad_token_" => "the API token isn't valid. Check it in Settings.".to_string(),
            "hoster_unavailable" => "that host isn't available right now.".to_string(),
            "file_unavailable" => "the file is no longer available.".to_string(),
            "premium_only" | "account_locked" | "not_premium" => "the account isn't premium or is locked.".to_string(),
            other => other.replace('_', " "),
        });
    }
    Ok(Direct {
        url: v["download"].as_str().context("Real-Debrid gave no download link")?.to_string(),
        filename: v["filename"].as_str().unwrap_or("download").to_string(),
        size: v["filesize"].as_u64(),
    })
}

/// A direct link for `url` when it is on a supported host, else `None`
/// (the link is downloaded as it is).
pub async fn resolve(client: &Client, token: &str, url: &str) -> Result<Option<Direct>> {
    let Ok(parsed) = url::Url::parse(url) else { return Ok(None) };
    let Some(host) = parsed.host_str() else { return Ok(None) };
    if token.trim().is_empty() || !matches!(parsed.scheme(), "http" | "https") {
        return Ok(None);
    }
    // A host list that can't be fetched must not stop an ordinary download.
    let Ok(list) = domains(client).await else { return Ok(None) };
    if !supported(host, &list) {
        return Ok(None);
    }
    let body = format!("link={}", percent_encoding::utf8_percent_encode(url, percent_encoding::NON_ALPHANUMERIC));
    let text = client
        .post(format!("{API}/unrestrict/link"))
        .header(header::AUTHORIZATION, format!("Bearer {}", token.trim()))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .context("couldn't reach Real-Debrid")?
        .text()
        .await?;
    parse_direct(&text).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_hosts_and_subdomains() {
        let d = vec!["1fichier.com".to_string(), "Rapidgator.net".to_string()];
        assert!(supported("1fichier.com", &d) && supported("cdn.1fichier.com", &d) && supported("rapidgator.net", &d));
        assert!(!supported("notrapidgator.net", &d) && !supported("example.com", &d));
    }

    #[test]
    fn reads_answers_and_errors() {
        let ok = parse_direct(r#"{"id":"X","filename":"a.zip","filesize":123,"link":"https://1fichier.com/?x","download":"https://dl.example/a.zip","streamable":0}"#).unwrap();
        assert_eq!(ok, Direct { url: "https://dl.example/a.zip".into(), filename: "a.zip".into(), size: Some(123) });
        let bad = parse_direct(r#"{"error":"bad_token","error_code":8}"#).unwrap_err().to_string();
        assert!(bad.contains("API token"), "{bad}");
    }

    /// Real-Debrid's public list of hosts: `cargo test live_hosts -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_hosts() {
        let client = swiftdl_engine::build_client().unwrap();
        let list = tauri::async_runtime::block_on(domains(&client)).unwrap();
        println!("{} hosts, for example {:?}", list.len(), &list[..list.len().min(5)]);
        assert!(list.len() > 20);
    }
}
