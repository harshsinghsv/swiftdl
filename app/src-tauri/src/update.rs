//! Tells the user about a newer SwiftDL release on GitHub, and installs it.
//!
//! On Windows the installer is downloaded and run in place; elsewhere the
//! download opens in the browser, since a .dmg or Linux package needs the
//! user's hands anyway.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;
use swiftdl_engine::Client;

const LATEST: &str = "https://api.github.com/repos/harshsinghsv/swiftdl/releases/latest";

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Update {
    pub version: String,
    /// The release page, with the full notes.
    pub page: String,
    /// This platform's installer, when the release has one.
    pub asset: Option<String>,
}

fn parts(v: &str) -> Vec<u64> {
    v.trim().trim_start_matches('v').split(['.', '-', '+']).map_while(|p| p.parse().ok()).collect()
}

/// Whether `remote` is a later version than `local` (both like "0.3.1").
pub fn is_newer(remote: &str, local: &str) -> bool {
    let (r, l) = (parts(remote), parts(local));
    !r.is_empty() && r > l
}

/// This platform's installer among the release's assets.
fn pick_asset(names: &[(String, String)]) -> Option<String> {
    let suffix = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "aarch64") => "_arm64_setup.exe",
        ("windows", "x86_64") => "_x64-setup.exe",
        ("macos", _) => ".dmg",
        _ => return None, // Let the user choose on the release page.
    };
    names.iter().find(|(name, _)| name.ends_with(suffix)).map(|(_, url)| url.clone())
}

/// The latest release, if it is newer than this build.
pub async fn check(client: &Client) -> Result<Option<Update>> {
    let body = client
        .get(LATEST)
        .header("User-Agent", "SwiftDL")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let rel: Value = serde_json::from_str(&body).context("GitHub sent an unexpected reply")?;
    let tag = rel["tag_name"].as_str().unwrap_or_default();
    if rel["draft"].as_bool() == Some(true) || rel["prerelease"].as_bool() == Some(true) || !is_newer(tag, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let assets: Vec<(String, String)> = rel["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some((a["name"].as_str()?.to_string(), a["browser_download_url"].as_str()?.to_string())))
        .collect();
    Ok(Some(Update {
        version: tag.trim_start_matches('v').to_string(),
        page: rel["html_url"].as_str().unwrap_or("https://github.com/harshsinghsv/swiftdl/releases/latest").to_string(),
        asset: pick_asset(&assets),
    }))
}

/// Downloads the Windows installer and starts it. The caller then quits the
/// app so the installer can replace it; `/P` shows only a progress bar, `/UPDATE` keeps
/// the user's shortcuts, and `/R` starts SwiftDL again afterwards.
#[cfg(windows)]
pub async fn run_installer(client: &Client, url: &str) -> Result<()> {
    if !url.starts_with("https://github.com/harshsinghsv/swiftdl/releases/download/") {
        bail!("refusing to install from {url}");
    }
    let bytes = client.get(url).header("User-Agent", "SwiftDL").send().await?.error_for_status()?.bytes().await?;
    let path = std::env::temp_dir().join(url.rsplit('/').next().unwrap_or("SwiftDL-setup.exe"));
    std::fs::write(&path, &bytes).with_context(|| format!("could not save {}", path.display()))?;
    std::process::Command::new(&path).args(["/P", "/R", "/UPDATE"]).spawn().context("could not start the installer")?;
    Ok(())
}

#[cfg(not(windows))]
pub async fn run_installer(_client: &Client, _url: &str) -> Result<()> {
    bail!("installing in place is only supported on Windows")
}

#[cfg(test)]
mod tests {
    use super::is_newer;

    #[cfg(windows)]
    #[test]
    fn picks_only_the_current_windows_architecture() {
        use super::pick_asset;
        let arm64 = ("SwiftDL_0.4.0_arm64_setup.exe".to_string(), "arm64".to_string());
        let x64 = ("SwiftDL_0.4.0_x64-setup.exe".to_string(), "x64".to_string());
        assert!(x64.0.ends_with("-setup.exe"));
        assert!(!arm64.0.ends_with("-setup.exe"));

        let (current, other) = if cfg!(target_arch = "aarch64") { (arm64, x64) } else { (x64, arm64) };
        assert_eq!(pick_asset(&[other.clone(), current.clone()]), Some(current.1));
        assert_eq!(pick_asset(&[other]), None);
    }

    #[test]
    fn compares_versions() {
        assert!(is_newer("v0.3.2", "0.3.1"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.0"));
        assert!(!is_newer("v0.3.1", "0.3.1"));
        assert!(!is_newer("v0.3.0", "0.3.1"));
        assert!(!is_newer("nightly", "0.3.1"));
    }
}
