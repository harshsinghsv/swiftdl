//! Native messaging host mode.
//!
//! Browsers can only talk to a local program by launching it with a
//! `chrome-extension://...` argument and exchanging length-prefixed JSON on
//! stdin/stdout. Rather than ship a second binary, the app itself plays that
//! role: it answers the message, hands downloads to the running app window
//! (starting it if needed, via the single-instance plugin), and exits.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};

/// Chromium browsers pass the caller's origin; Firefox passes the path of
/// the host manifest and the add-on's ID.
pub fn is_native_host_launch(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a.starts_with("chrome-extension://") || a == crate::browser::FIREFOX_ID)
}

pub fn run() {
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    loop {
        let mut len = [0u8; 4];
        if stdin.read_exact(&mut len).is_err() {
            break; // browser closed the pipe
        }
        let len = u32::from_ne_bytes(len) as usize;
        if len > 4 * 1024 * 1024 {
            break;
        }
        let mut buf = vec![0u8; len];
        if stdin.read_exact(&mut buf).is_err() {
            break;
        }
        let reply = handle(&buf).to_string();
        let ok = stdout
            .write_all(&(reply.len() as u32).to_ne_bytes())
            .and_then(|_| stdout.write_all(reply.as_bytes()))
            .and_then(|_| stdout.flush());
        if ok.is_err() {
            break;
        }
    }
}

fn handle(msg: &[u8]) -> Value {
    let Ok(v) = serde_json::from_slice::<Value>(msg) else {
        return json!({ "ok": false, "error": "invalid JSON" });
    };
    crate::browser::mark_extension_seen(caller_browser());
    let reply = |r: io::Result<()>| match r {
        Ok(()) => json!({ "ok": true }),
        Err(e) => json!({ "ok": false, "error": e.to_string() }),
    };
    match v["type"].as_str() {
        Some("ping") => json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }),
        Some("download") if v["url"].is_string() => match refused(&v) {
            // The site won't give SwiftDL this file: say so, and the extension
            // lets the browser keep (and finish) its own download.
            Some(why) => json!({ "ok": false, "error": why }),
            None => reply(launch_app(&["--add", &URL_SAFE_NO_PAD.encode(msg)])),
        },
        Some("show") => reply(launch_app(&[])),
        // The quality menu opened on a video: start looking it up now.
        Some("prepare") if v["url"].is_string() => reply(launch_app(&["--prepare", &URL_SAFE_NO_PAD.encode(msg)])),
        _ => json!({ "ok": false, "error": "unknown message" }),
    }
}

/// Asks the site for the file the way SwiftDL would. Returns why not, when the
/// site refuses (403, a sign-in page...): then the browser should keep the
/// download. Network trouble and slow answers are not refusals; the app deals
/// with those.
fn refused(v: &Value) -> Option<String> {
    let inc: crate::manager::Incoming = serde_json::from_value(v.clone()).ok()?;
    let plain = inc.url.starts_with("http") && !crate::media::is_media_url(&inc.url) && !crate::torrent::is_torrent(&inc.url, inc.filename.as_deref());
    if !plain {
        return None;
    }
    let headers = crate::manager::to_header_map(&inc.headers());
    let probe = async {
        let client = swiftdl_engine::build_client().ok()?;
        tokio::time::timeout(std::time::Duration::from_secs(8), swiftdl_engine::probe(&client, &inc.url, &headers)).await.ok()
    };
    let result = tauri::async_runtime::block_on(probe)?;
    let err = format!("{:#}", result.err()?);
    (err.starts_with("server returned") || err.starts_with("This link opens a web page")).then(|| format!("{err}. The browser keeps this download."))
}

/// Starts the app with `args`. If it is already open, the single-instance
/// plugin forwards the args to that window and this new process exits.
fn launch_app(args: &[&str]) -> io::Result<()> {
    let mut cmd = Command::new(std::env::current_exe()?);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // The browser's pipe handles are inheritable. If the app inherited
        // them, the browser would never see our stdout close.
        unsafe { windows_handles::stop_inheriting_std_handles() };
        // The browser started this host, so it may come to the front; pass
        // that on, or the app's New download window would only flash in the
        // taskbar instead of appearing over the browser.
        unsafe { windows_handles::allow_foreground() };
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        // Chrome runs hosts in a job that is killed with the host; break
        // away so the app window survives. Fall back if the job forbids it.
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        if cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB).spawn().is_ok() {
            return Ok(());
        }
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    cmd.spawn().map(|_| ())
}

#[cfg(windows)]
mod windows_handles {
    use std::ffi::c_void;

    const STD_HANDLES: [u32; 3] = [-10i32 as u32, -11i32 as u32, -12i32 as u32];
    const HANDLE_FLAG_INHERIT: u32 = 1;

    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
        fn AllowSetForegroundWindow(process_id: u32) -> i32;
    }

    pub unsafe fn allow_foreground() {
        const ASFW_ANY: u32 = u32::MAX;
        unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    }

    pub unsafe fn stop_inheriting_std_handles() {
        for which in STD_HANDLES {
            unsafe {
                let h = GetStdHandle(which);
                if !h.is_null() && h as isize != -1 {
                    SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }
}

/// The browser that launched this host, found by walking up the process tree.
/// On Windows the browser starts hosts through `cmd.exe`, so it is usually
/// the grandparent rather than the parent.
fn caller_browser() -> Option<&'static str> {
    let mut pid = std::process::id();
    for _ in 0..4 {
        pid = parent_pid(pid)?;
        if let Some(b) = process_path(pid).as_deref().and_then(crate::browser::browser_from_exe) {
            return Some(b);
        }
    }
    None
}

#[cfg(windows)]
fn parent_pid(pid: u32) -> Option<u32> {
    process_tree::entries().into_iter().find(|e| e.0 == pid).map(|e| e.1).filter(|&p| p != 0)
}

#[cfg(windows)]
fn process_path(pid: u32) -> Option<String> {
    process_tree::image_path(pid)
}

#[cfg(unix)]
fn ps(pid: u32, field: &str) -> Option<String> {
    let out = Command::new("ps").args(["-o", field, "-p", &pid.to_string()]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(unix)]
fn parent_pid(pid: u32) -> Option<u32> {
    ps(pid, "ppid=")?.parse().ok().filter(|&p| p > 1)
}

#[cfg(unix)]
fn process_path(pid: u32) -> Option<String> {
    std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| ps(pid, "comm="))
}

#[cfg(windows)]
mod process_tree {
    use std::ffi::c_void;

    #[repr(C)]
    struct ProcessEntry32W {
        size: u32,
        usage: u32,
        pid: u32,
        heap_id: usize,
        module_id: u32,
        threads: u32,
        parent_pid: u32,
        pri_class_base: i32,
        flags: u32,
        exe_file: [u16; 260],
    }

    const TH32CS_SNAPPROCESS: u32 = 0x2;
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    unsafe extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> *mut c_void;
        fn Process32FirstW(snap: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snap: *mut c_void, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn QueryFullProcessImageNameW(process: *mut c_void, flags: u32, name: *mut u16, size: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// (pid, parent pid) for every running process.
    pub fn entries() -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() || snap as isize == -1 {
                return out;
            }
            let mut e: ProcessEntry32W = std::mem::zeroed();
            e.size = std::mem::size_of::<ProcessEntry32W>() as u32;
            let mut ok = Process32FirstW(snap, &mut e);
            while ok != 0 {
                out.push((e.pid, e.parent_pid));
                ok = Process32NextW(snap, &mut e);
            }
            CloseHandle(snap);
        }
        out
    }

    pub fn image_path(pid: u32) -> Option<String> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut len);
            CloseHandle(h);
            (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
        }
    }
}

/// Pulls the `--prepare <payload>` the host passed on, if any.
pub fn parse_prepare_arg(args: &[String]) -> Option<crate::manager::Incoming> {
    let i = args.iter().position(|a| a == "--prepare")?;
    let raw = URL_SAFE_NO_PAD.decode(args.get(i + 1)?).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Pulls the `--add <payload>` the host passed on, if any.
pub fn parse_add_arg(args: &[String]) -> Option<crate::manager::Incoming> {
    let i = args.iter().position(|a| a == "--add")?;
    let raw = URL_SAFE_NO_PAD.decode(args.get(i + 1)?).ok()?;
    serde_json::from_slice(&raw).ok()
}

#[cfg(test)]
mod refusal_tests {
    use super::refused;
    use std::io::{Read, Write};

    /// A server that answers every request with `reply`.
    fn serve(reply: &'static str) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 2048];
                let _ = s.read(&mut buf);
                let _ = s.write_all(reply.as_bytes());
            }
        });
        format!("http://127.0.0.1:{port}/file.zip")
    }

    fn ask(url: &str) -> Option<String> {
        refused(&serde_json::json!({ "type": "download", "url": url }))
    }

    #[test]
    fn the_browser_keeps_what_the_site_refuses() {
        let forbidden = serve("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        assert!(ask(&forbidden).unwrap().contains("403"));
        let page = serve("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi");
        assert!(ask(&page).unwrap().contains("web page"));
        let file = serve("HTTP/1.1 206 Partial Content\r\nContent-Type: application/zip\r\nContent-Range: bytes 0-0/1000\r\nContent-Length: 1\r\nConnection: close\r\n\r\nx");
        assert_eq!(ask(&file), None, "a working link is taken over");
        assert_eq!(ask("http://127.0.0.1:1/unreachable.zip"), None, "no answer is not a refusal");
        assert_eq!(ask("https://www.youtube.com/watch?v=abc"), None, "video pages go through yt-dlp");
    }
}

#[cfg(test)]
mod bind_tests {
    use std::io::{Read, Write};

    #[test]
    fn downloads_go_out_from_the_chosen_address_only() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut s in listener.incoming().flatten() {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf);
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi");
            }
        });
        let url = format!("http://127.0.0.1:{port}/");
        let get = |bind: &str| {
            let client = swiftdl_engine::build_client_opts("", bind).unwrap();
            tauri::async_runtime::block_on(async { client.get(&url).send().await.map(|r| r.status().as_u16()) })
        };
        assert_eq!(get("127.0.0.1").unwrap(), 200, "an address this PC has works");
        assert!(get("192.0.2.1").is_err(), "an address this PC doesn't have fails, like a dropped VPN");
        assert_eq!(get("").unwrap(), 200, "empty means any");
    }
}
