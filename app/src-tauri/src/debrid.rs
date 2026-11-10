//! Debrid services: paid accounts that fetch files from file-hosting sites at
//! full speed and hand back a direct link. With a service and its API token
//! set in Settings, links on the hosts it supports are turned into direct links
//! before downloading. Real-Debrid, AllDebrid and Premiumize are supported.

use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use swiftdl_engine::{Client, header};

const REAL: &str = "https://api.real-debrid.com/rest/1.0";
const ALL: &str = "https://api.alldebrid.com/v4";
const PREMIUMIZE: &str = "https://www.premiumize.me/api";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Provider {
    Real,
    All,
    Premiumize,
}

impl Provider {
    /// From the setting's value; anything unknown means Real-Debrid.
    pub fn parse(s: &str) -> Self {
        match s {
            "alldebrid" => Self::All,
            "premiumize" => Self::Premiumize,
            _ => Self::Real,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Real => "realdebrid",
            Self::All => "alldebrid",
            Self::Premiumize => "premiumize",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Real => "Real-Debrid",
            Self::All => "AllDebrid",
            Self::Premiumize => "Premiumize",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Direct {
    pub url: String,
    pub filename: String,
    pub size: Option<u64>,
}

/// Each service's supported hosts, kept for six hours.
static DOMAINS: LazyLock<Mutex<HashMap<&'static str, (Instant, Vec<String>)>>> = LazyLock::new(Default::default);

fn strings(v: &Value) -> impl Iterator<Item = String> + '_ {
    v.as_array().into_iter().flatten().filter_map(|s| s.as_str().map(str::to_string))
}

fn parse_domains(p: Provider, body: &str) -> Result<Vec<String>> {
    let v: Value = serde_json::from_str(body).with_context(|| format!("{} sent an unexpected list of hosts", p.name()))?;
    let list: Vec<String> = match p {
        Provider::Real => strings(&v).collect(),
        Provider::All => strings(&v["data"]["hosts"]).chain(strings(&v["data"]["redirectors"])).collect(),
        Provider::Premiumize => strings(&v["directdl"]).collect(),
    };
    if list.is_empty() {
        bail!("{} sent an empty list of hosts", p.name());
    }
    Ok(list)
}

async fn domains(client: &Client, p: Provider) -> Result<Vec<String>> {
    if let Some((at, list)) = DOMAINS.lock().unwrap().get(p.key()) {
        if at.elapsed() < Duration::from_secs(6 * 3600) {
            return Ok(list.clone());
        }
    }
    let url = match p {
        Provider::Real => format!("{REAL}/hosts/domains"),
        Provider::All => format!("{ALL}/hosts/domains?agent=swiftdl"),
        Provider::Premiumize => format!("{PREMIUMIZE}/services/list"),
    };
    let text = client.get(url).send().await?.error_for_status()?.text().await?;
    let list = parse_domains(p, &text)?;
    DOMAINS.lock().unwrap().insert(p.key(), (Instant::now(), list.clone()));
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

fn last_part(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn parse_direct(p: Provider, body: &str) -> Result<Direct> {
    let v: Value = serde_json::from_str(body).with_context(|| format!("{} sent an unexpected answer", p.name()))?;
    match p {
        Provider::Real => {
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
        Provider::All => {
            if v["status"].as_str() != Some("success") {
                let code = v["error"]["code"].as_str().unwrap_or("");
                let msg = v["error"]["message"].as_str().unwrap_or("it refused the link");
                bail!("AllDebrid: {}", if code.starts_with("AUTH_") { "the API key isn't valid. Check it in Settings.".to_string() } else { msg.to_string() });
            }
            let d = &v["data"];
            Ok(Direct {
                url: d["link"].as_str().context("AllDebrid gave no download link")?.to_string(),
                filename: d["filename"].as_str().unwrap_or("download").to_string(),
                size: d["filesize"].as_u64(),
            })
        }
        Provider::Premiumize => {
            if v["status"].as_str() != Some("success") {
                let msg = v["message"].as_str().unwrap_or("it refused the link");
                bail!("Premiumize: {}", if v["code"].as_str() == Some("authentication_failed") { "the API key isn't valid. Check it in Settings.".to_string() } else { msg.to_string() });
            }
            let first = &v["content"][0];
            let url = first["link"].as_str().or_else(|| v["location"].as_str()).context("Premiumize gave no download link")?.to_string();
            let name = first["path"].as_str().map(last_part).or_else(|| v["filename"].as_str()).unwrap_or("download").to_string();
            Ok(Direct { url, filename: name, size: first["size"].as_u64().or_else(|| v["filesize"].as_u64()) })
        }
    }
}

/// A direct link for `url` when it is on a host the service supports, else
/// `None` (the link is downloaded as it is).
pub async fn resolve(client: &Client, provider: &str, token: &str, url: &str) -> Result<Option<Direct>> {
    let Ok(parsed) = url::Url::parse(url) else { return Ok(None) };
    let Some(host) = parsed.host_str() else { return Ok(None) };
    if token.trim().is_empty() || !matches!(parsed.scheme(), "http" | "https") {
        return Ok(None);
    }
    let (p, token) = (Provider::parse(provider), token.trim());
    // A host list that can't be fetched must not stop an ordinary download.
    let Ok(list) = domains(client, p).await else { return Ok(None) };
    if !supported(host, &list) {
        return Ok(None);
    }
    let enc = |s: &str| percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string();
    let request = match p {
        Provider::Real => client
            .post(format!("{REAL}/unrestrict/link"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(format!("link={}", enc(url))),
        Provider::All => client
            .get(format!("{ALL}/link/unlock?agent=swiftdl&link={}", enc(url)))
            .header(header::AUTHORIZATION, format!("Bearer {token}")),
        Provider::Premiumize => client
            .post(format!("{PREMIUMIZE}/transfer/directdl"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(format!("apikey={}&src={}", enc(token), enc(url))),
    };
    let text = request.send().await.with_context(|| format!("couldn't reach {}", p.name()))?.text().await?;
    parse_direct(p, &text).map(Some)
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
    fn reads_real_debrid() {
        let ok = parse_direct(Provider::Real, r#"{"id":"X","filename":"a.zip","filesize":123,"link":"https://1fichier.com/?x","download":"https://dl.example/a.zip"}"#).unwrap();
        assert_eq!(ok, Direct { url: "https://dl.example/a.zip".into(), filename: "a.zip".into(), size: Some(123) });
        assert!(parse_direct(Provider::Real, r#"{"error":"bad_token","error_code":8}"#).unwrap_err().to_string().contains("API token"));
    }

    #[test]
    fn reads_alldebrid() {
        let ok = parse_direct(Provider::All, r#"{"status":"success","data":{"link":"https://dl.example/b.iso","filename":"b.iso","filesize":456,"host":"1fichier"}}"#).unwrap();
        assert_eq!(ok, Direct { url: "https://dl.example/b.iso".into(), filename: "b.iso".into(), size: Some(456) });
        let bad = parse_direct(Provider::All, r#"{"status":"error","error":{"code":"AUTH_BAD_APIKEY","message":"The auth apikey is invalid"}}"#).unwrap_err().to_string();
        assert!(bad.contains("API key"), "{bad}");
        let other = parse_direct(Provider::All, r#"{"status":"error","error":{"code":"LINK_DOWN","message":"Link is down"}}"#).unwrap_err().to_string();
        assert!(other.contains("Link is down"), "{other}");
    }

    #[test]
    fn reads_premiumize() {
        let ok = parse_direct(Provider::Premiumize, r#"{"status":"success","location":"https://dl.example/c.zip","content":[{"path":"folder/c.zip","size":789,"link":"https://dl.example/c.zip"}]}"#).unwrap();
        assert_eq!(ok, Direct { url: "https://dl.example/c.zip".into(), filename: "c.zip".into(), size: Some(789) });
        let bad = parse_direct(Provider::Premiumize, r#"{"status":"error","message":"Not logged in","code":"authentication_failed"}"#).unwrap_err().to_string();
        assert!(bad.contains("API key"), "{bad}");
    }

    #[test]
    fn reads_host_lists() {
        assert_eq!(parse_domains(Provider::Real, r#"["a.com","b.com"]"#).unwrap().len(), 2);
        assert_eq!(parse_domains(Provider::All, r#"{"status":"success","data":{"hosts":["a.com"],"streams":["x"],"redirectors":["r.com"]}}"#).unwrap(), ["a.com", "r.com"]);
        assert_eq!(parse_domains(Provider::Premiumize, r#"{"status":"success","cache":["c.com"],"directdl":["d.com"]}"#).unwrap(), ["d.com"]);
        assert!(parse_domains(Provider::Premiumize, r#"{"status":"success"}"#).is_err());
    }

    /// The services' public host lists: `cargo test live_hosts -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_hosts() {
        let client = swiftdl_engine::build_client().unwrap();
        for p in [Provider::Real, Provider::All, Provider::Premiumize] {
            let list = tauri::async_runtime::block_on(domains(&client, p)).unwrap();
            println!("{}: {} hosts, for example {:?}", p.name(), list.len(), &list[..list.len().min(3)]);
            assert!(list.len() > 10);
        }
    }
}
