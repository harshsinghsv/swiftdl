//! The interface language for the parts Rust draws itself: the tray menu,
//! notifications, window titles and the live status notes the engine reports.
//!
//! The front end resolves "system" (it can read navigator.language on every
//! platform) and tells us the answer with `set_ui_language`. Until then, and
//! on the next start before the window is up, `system()` makes a best guess.

use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    ZhCn,
}

impl Lang {
    pub fn from_code(code: &str) -> Self {
        match code {
            "en" => Lang::En,
            "zh-CN" | "zh" => Lang::ZhCn,
            _ => Lang::system(),
        }
    }

    pub fn from_setting(setting: &str) -> Self {
        if setting == "system" || setting.is_empty() {
            Lang::system()
        } else {
            Lang::from_code(setting)
        }
    }

    /// The computer's language, used before the front end reports its own.
    pub fn system() -> Self {
        #[cfg(windows)]
        {
            use winreg::RegKey;
            use winreg::enums::HKEY_CURRENT_USER;
            if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Control Panel\\International")
                && let Ok(name) = key.get_value::<String, _>("LocaleName")
            {
                return if name.to_ascii_lowercase().starts_with("zh") { Lang::ZhCn } else { Lang::En };
            }
        }
        #[cfg(not(windows))]
        {
            if let Ok(value) = std::env::var("LC_ALL").or_else(|_| std::env::var("LANG"))
                && value.to_ascii_lowercase().starts_with("zh")
            {
                return Lang::ZhCn;
            }
        }
        Lang::En
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set(lang: Lang) {
    CURRENT.store(
        match lang {
            Lang::En => 0,
            Lang::ZhCn => 1,
        },
        Ordering::Relaxed,
    );
}

pub fn get() -> Lang {
    if CURRENT.load(Ordering::Relaxed) == 1 { Lang::ZhCn } else { Lang::En }
}

/// A fixed string: tray menu items, notification titles, window titles.
pub fn tr(en: &'static str) -> &'static str {
    match (get(), en) {
        (Lang::ZhCn, "Open SwiftDL") => "打开 SwiftDL",
        (Lang::ZhCn, "Quit SwiftDL") => "退出 SwiftDL",
        (Lang::ZhCn, "Ignore speed limit") => "忽略速度限制",
        (Lang::ZhCn, "New download") => "新建下载",
        (Lang::ZhCn, "Download complete") => "下载完成",
        (Lang::ZhCn, "Unpacked") => "已解压",
        (Lang::ZhCn, "Couldn't unpack") => "解压失败",
        (Lang::ZhCn, "Downloads finished") => "下载已全部完成",
        _ => en,
    }
}

/// A live status note from the engine ("Merging video and audio").
pub fn note(en: &str) -> String {
    if get() == Lang::En {
        return en.to_string();
    }
    match en {
        "Checking the file" => "正在校验文件",
        "Starting" => "正在开始",
        "Getting video details" => "正在获取视频详情",
        "Joining" => "正在合并",
        "Joining video and audio" => "正在合并音视频",
        "Merging video and audio" => "正在合并音视频",
        "Finishing" => "正在完成",
        "Getting yt-dlp" => "正在获取 yt-dlp",
        "Getting ffmpeg" => "正在获取 ffmpeg",
        "Unpacking ffmpeg" => "正在解压 ffmpeg",
        "Finding peers" => "正在寻找用户",
        "Looking for peers" => "正在寻找用户",
        "Downloading from 1 peer" => "正在从 1 个用户下载",
        "Checking existing pieces" => "正在检查已有分块",
        _ => en,
    }
    .to_string()
}

/// The torrent status line, which has numbers in it.
pub fn peers(n: u32, seeds: u32) -> String {
    match get() {
        Lang::ZhCn => format!("正在从 {n} 个用户下载 · {seeds} 个做种"),
        Lang::En => format!("Downloading from {n} peers · {seeds} seeding"),
    }
}

/// A short message from the backend that the user can act on.
pub fn text(en: &str) -> String {
    if get() == Lang::En {
        return en.to_string();
    }
    match en {
        "That isn't a web link" => "这不是一个网页链接",
        "Download not found" => "找不到该下载",
        "Only regular file downloads can change address" => "只有普通文件下载才能更换地址",
        "Pause the download first" => "请先暂停下载",
        "Only web, ftp and magnet links and .torrent files are supported" => "只支持网页、ftp、磁力链接和 .torrent 文件",
        "File name is empty" => "文件名为空",
        "not found" => "未找到",
        "extension folder not found" => "找不到扩展文件夹",
        "That's a large file, not a web page" => "这是一个大文件，不是网页",
        "Spotify's music is copy-protected, so SwiftDL can't download it." => "Spotify 的音乐受版权保护，SwiftDL 无法下载。",
        "that playlist has no videos SwiftDL can download" => "这个播放列表里没有 SwiftDL 能下载的视频",
        _ => en,
    }
    .to_string()
}

/// The language is process-wide, so tests that read it take this lock and
/// leave it as English when they finish.
#[cfg(test)]
pub fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chinese_strings_replace_english() {
        let _guard = test_lock();
        set(Lang::ZhCn);
        assert_eq!(tr("Open SwiftDL"), "打开 SwiftDL");
        assert_eq!(note("Merging video and audio"), "正在合并音视频");
        assert_eq!(peers(3, 2), "正在从 3 个用户下载 · 2 个做种");
        set(Lang::En);
        assert_eq!(tr("Open SwiftDL"), "Open SwiftDL");
        assert_eq!(note("Merging video and audio"), "Merging video and audio");
        assert_eq!(peers(3, 2), "Downloading from 3 peers · 2 seeding");
    }
}
