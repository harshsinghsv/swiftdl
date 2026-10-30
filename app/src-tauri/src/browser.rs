//! Registers the app as a native messaging host so the browser extension
//! can reach it. Runs on every launch (cheap and self-healing if the app
//! moved). Per-user only: no admin rights needed.

use anyhow::Result;
use serde_json::json;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};

pub const HOST_NAME: &str = "com.swiftdl.host";

/// Extension IDs allowed to talk to the app. The first is fixed by the
/// `key` in `extension/manifest.json`; add the Web Store ID once published.
pub const EXTENSION_IDS: &[&str] = &["knmfjbmojepkdiflnckpfjimfmjfcbgh"];

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
        for browser in [
            r"Software\Google\Chrome",
            r"Software\Microsoft\Edge",
            r"Software\BraveSoftware\Brave-Browser",
            r"Software\Chromium",
            r"Software\Vivaldi",
        ] {
            let (key, _) = hkcu.create_subkey(format!(r"{browser}\NativeMessagingHosts\{HOST_NAME}"))?;
            key.set_value("", &file.to_string_lossy().as_ref())?;
        }
    }

    #[cfg(not(windows))]
    {
        let _ = data_dir;
        let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
        #[cfg(target_os = "macos")]
        let bases = [
            "Library/Application Support/Google/Chrome",
            "Library/Application Support/Chromium",
            "Library/Application Support/Microsoft Edge",
            "Library/Application Support/BraveSoftware/Brave-Browser",
            "Library/Application Support/Vivaldi",
        ];
        #[cfg(not(target_os = "macos"))]
        let bases =
            [".config/google-chrome", ".config/chromium", ".config/microsoft-edge", ".config/BraveSoftware/Brave-Browser", ".config/vivaldi"];
        for base in bases {
            let base = home.join(base);
            if base.is_dir() {
                let dir = base.join("NativeMessagingHosts");
                std::fs::create_dir_all(&dir)?;
                std::fs::write(dir.join(format!("{HOST_NAME}.json")), &body)?;
            }
        }
    }
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
