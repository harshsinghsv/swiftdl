//! The resume file (`<name>.swiftdl`) that sits next to the `.part` file.
//! It records which byte ranges are still missing, so a crash, Ctrl+C or
//! dropped network never costs more than the last second of progress.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedState {
    pub url: String,
    pub size: u64,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    /// Remaining `[pos, end)` ranges. Everything outside them is on disk.
    pub segments: Vec<(u64, u64)>,
}

impl SavedState {
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Writes to a temp file first, then renames, so the state file is never
    /// left half-written.
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("swiftdl.tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// True when this state describes the same remote file version.
    pub fn matches(&self, url: &str, size: u64, etag: &Option<String>, last_modified: &Option<String>) -> bool {
        self.url == url
            && self.size == size
            && (self.etag.is_none() || etag.is_none() || &self.etag == etag)
            && (self.last_modified.is_none() || last_modified.is_none() || &self.last_modified == last_modified)
    }
}
