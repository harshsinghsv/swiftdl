//! Browser integration, all per-user (no admin rights):
//!
//! - Registers the app as a native messaging host so the extension can
//!   reach it. Runs on every launch, so it heals itself if the app moves.
//! - Offers the extension to installed Chromium browsers through their
//!   "external extensions" mechanism. Browsers never install extensions
//!   silently; on next start they show their own "New extension added"
//!   prompt and the user clicks Enable. This only works for extensions
//!   published on the Chrome Web Store (Edge and Brave use that store too).
//! - Detects which browsers are installed and when the extension last
//!   talked to the app, for the onboarding card.

use anyhow::Result;
use serde::Serialize;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Manager};

pub const HOST_NAME: &str = "com.swiftdl.host";

/// Extension IDs allowed to talk to the app. Fixed by the `key` in
/// `extension/manifest.json`; keep that key when publishing to the Chrome
/// Web Store (upload `extension-key.pem` once) so the ID stays the same.
pub const EXTENSION_IDS: &[&str] = &["knmfjbmojepkdiflnckpfjimfmjfcbgh"];
pub const STORE_UPDATE_URL: &str = "https://clients2.google.com/service/update2/crx";

/// Flip to `true` once the extension is live on the Chrome Web Store. Until
/// then the UI guides users through installing it from the bundled folder
/// instead of offering store buttons that would lead nowhere.
pub const STORE_PUBLISHED: bool = false;

pub fn store_url() -> String {
    format!("https://chromewebstore.google.com/detail/{}", EXTENSION_IDS[0])
}

/// Same folder Tauri uses for `app_data_dir`, computed without a running
/// app so the native-host process can use it too.
pub fn data_dir() -> PathBuf {
    let id = "com.swiftdl.app";
    #[cfg(windows)]
    let base = std::env::var_os("APPDATA").map(PathBuf::from);
    #[cfg(target_os = "macos")]
    let base = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    base.unwrap_or_default().join(id)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Called by the native host on every message from the extension.
pub fn mark_extension_seen() {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join("extension.json"), json!({ "lastSeen": now_ms() }).to_string());
}

pub fn extension_last_seen() -> Option<u64> {
    let text = std::fs::read_to_string(data_dir().join("extension.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&text).ok()?["lastSeen"].as_u64()
}

// ---------- native messaging host ----------

pub fn register(data_dir: &Path) -> Result<()> {
    let exe = std::env::current_exe()?;
    let manifest = json!({
        "name": HOST_NAME,
        "description": "SwiftDL download manager",
        "path": exe,
        "type": "stdio",
        "allowed_origins": EXTENSION_IDS.iter().map(|id| format!("chrome-extension://{id}/")).collect::<Vec<_>>(),
    });
    let body = serde_json::to_vec_pretty(&manifest)?;

    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let dir = data_dir.join("native-host");
        std::fs::create_dir_all(&dir)?;
        let file = dir.join(format!("{HOST_NAME}.json"));
        std::fs::write(&file, &body)?;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for b in BROWSERS {
            let (key, _) = hkcu.create_subkey(format!(r"{}\NativeMessagingHosts\{HOST_NAME}", b.reg_root))?;
            key.set_value("", &file.to_string_lossy().as_ref())?;
        }
    }

    #[cfg(not(windows))]
    {
        let _ = data_dir;
        for b in BROWSERS {
            let base = home().join(b.config_dir);
            if base.is_dir() {
                let dir = base.join("NativeMessagingHosts");
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join(format!("{HOST_NAME}.json")), &body)?;
            }
        }
    }
    Ok(())
}

// ---------- browsers ----------

struct Browser {
    id: &'static str,
    name: &'static str,
    /// Registry root under HKCU (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    reg_root: &'static str,
    /// Profile folder under $HOME (macOS / Linux).
    #[cfg_attr(windows, allow(dead_code))]
    config_dir: &'static str,
    /// Reads the external-extensions registration (Web Store extensions).
    external: bool,
}

#[cfg(windows)]
const BROWSERS: &[Browser] = &[
    Browser { id: "chrome", name: "Chrome", reg_root: r"Software\Google\Chrome", config_dir: "", external: true },
    Browser { id: "edge", name: "Edge", reg_root: r"Software\Microsoft\Edge", config_dir: "", external: true },
    Browser { id: "brave", name: "Brave", reg_root: r"Software\BraveSoftware\Brave-Browser", config_dir: "", external: true },
    Browser { id: "vivaldi", name: "Vivaldi", reg_root: r"Software\Vivaldi", config_dir: "", external: false },
    Browser { id: "chromium", name: "Chromium", reg_root: r"Software\Chromium", config_dir: "", external: false },
];
#[cfg(target_os = "macos")]
const BROWSERS: &[Browser] = &[
    Browser { id: "chrome", name: "Chrome", reg_root: "", config_dir: "Library/Application Support/Google/Chrome", external: true },
    Browser { id: "edge", name: "Edge", reg_root: "", config_dir: "Library/Application Support/Microsoft Edge", external: true },
    Browser { id: "brave", name: "Brave", reg_root: "", config_dir: "Library/Application Support/BraveSoftware/Brave-Browser", external: true },
    Browser { id: "vivaldi", name: "Vivaldi", reg_root: "", config_dir: "Library/Application Support/Vivaldi", external: false },
    Browser { id: "chromium", name: "Chromium", reg_root: "", config_dir: "Library/Application Support/Chromium", external: false },
];
#[cfg(all(unix, not(target_os = "macos")))]
const BROWSERS: &[Browser] = &[
    Browser { id: "chrome", name: "Chrome", reg_root: "", config_dir: ".config/google-chrome", external: false },
    Browser { id: "edge", name: "Edge", reg_root: "", config_dir: ".config/microsoft-edge", external: false },
    Browser { id: "brave", name: "Brave", reg_root: "", config_dir: ".config/BraveSoftware/Brave-Browser", external: false },
    Browser { id: "vivaldi", name: "Vivaldi", reg_root: "", config_dir: ".config/vivaldi", external: false },
    Browser { id: "chromium", name: "Chromium", reg_root: "", config_dir: ".config/chromium", external: false },
];

#[cfg(not(windows))]
fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

/// Where the browser's program lives, if it's installed.
fn browser_exe(id: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
        let pf = [env("ProgramFiles"), env("ProgramFiles(x86)"), env("LOCALAPPDATA")];
        let rel = match id {
            "chrome" => r"Google\Chrome\Application\chrome.exe",
            "edge" => r"Microsoft\Edge\Application\msedge.exe",
            "brave" => r"BraveSoftware\Brave-Browser\Application\brave.exe",
            "vivaldi" => r"Vivaldi\Application\vivaldi.exe",
            "chromium" => r"Chromium\Application\chrome.exe",
            _ => return None,
        };
        pf.into_iter().flatten().map(|b| b.join(rel)).find(|p| p.exists())
    }
    #[cfg(target_os = "macos")]
    {
        let app = match id {
            "chrome" => "Google Chrome.app",
            "edge" => "Microsoft Edge.app",
            "brave" => "Brave Browser.app",
            "vivaldi" => "Vivaldi.app",
            "chromium" => "Chromium.app",
            _ => return None,
        };
        [PathBuf::from("/Applications").join(app), home().join("Applications").join(app)].into_iter().find(|p| p.exists())
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let names: &[&str] = match id {
            "chrome" => &["google-chrome", "google-chrome-stable"],
            "edge" => &["microsoft-edge", "microsoft-edge-stable"],
            "brave" => &["brave-browser", "brave"],
            "vivaldi" => &["vivaldi", "vivaldi-stable"],
            "chromium" => &["chromium", "chromium-browser"],
            _ => return None,
        };
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).flat_map(|d| names.iter().map(move |n| d.join(n))).find(|p| p.exists())
    }
}

/// Adds (or, when `offer` is false, removes) the Web Store registration
/// that makes each installed browser prompt the user to enable the
/// extension the next time it starts.
pub fn offer_extension(offer: bool) -> Result<()> {
    let id = EXTENSION_IDS[0];
    #[cfg(windows)]
    {
        use winreg::RegKey;
        use winreg::enums::HKEY_CURRENT_USER;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        for b in BROWSERS.iter().filter(|b| b.external) {
            let path = format!(r"{}\Extensions\{id}", b.reg_root);
            if offer && browser_exe(b.id).is_some() {
                let (key, _) = hkcu.create_subkey(&path)?;
                key.set_value("update_url", &STORE_UPDATE_URL)?;
            } else if !offer {
                let _ = hkcu.delete_subkey_all(&path);
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        for b in BROWSERS.iter().filter(|b| b.external) {
            let dir = home().join(b.config_dir).join("External Extensions");
            let file = dir.join(format!("{id}.json"));
            if offer && browser_exe(b.id).is_some() {
                std::fs::create_dir_all(&dir)?;
                std::fs::write(&file, json!({ "external_update_url": STORE_UPDATE_URL }).to_string())?;
            } else if !offer {
                let _ = std::fs::remove_file(&file);
            }
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = (offer, id); // Linux only reads system-wide external extensions.
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    id: &'static str,
    name: &'static str,
    installed: bool,
    /// The browser will prompt to enable the extension from the Web Store.
    auto_offer: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationStatus {
    browsers: Vec<BrowserStatus>,
    /// Last time the extension talked to the app (ms since epoch).
    last_seen: Option<u64>,
    store_url: String,
    /// Whether the extension can be added from the Chrome Web Store yet.
    store_available: bool,
}

pub fn status() -> IntegrationStatus {
    IntegrationStatus {
        store_available: STORE_PUBLISHED,
        browsers: BROWSERS
            .iter()
            .map(|b| BrowserStatus { id: b.id, name: b.name, installed: browser_exe(b.id).is_some(), auto_offer: b.external })
            .collect(),
        last_seen: extension_last_seen(),
        store_url: store_url(),
    }
}

/// Opens the extension's store page in a specific browser.
pub fn open_store_in(app: &AppHandle, browser: &str) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let exe = browser_exe(browser).ok_or_else(|| anyhow::anyhow!("{browser} isn't installed"))?;
    #[cfg(target_os = "macos")]
    let with = exe.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    #[cfg(not(target_os = "macos"))]
    let with = exe.to_string_lossy().into_owned();
    app.opener().open_url(store_url(), Some(with))?;
    Ok(())
}

/// The unpacked extension shipped inside the installer, for "Load unpacked".
pub fn bundled_extension_dir(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().resource_dir().ok()?.join("extension");
    if dir.join("manifest.json").exists() {
        return Some(dir);
    }
    // Dev builds: use the repo's copy.
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extension");
    dev.join("manifest.json").exists().then(|| dev.canonicalize().unwrap_or(dev))
}
