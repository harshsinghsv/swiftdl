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

pub fn is_native_host_launch(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a.starts_with("chrome-extension://"))
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
    match v["type"].as_str() {
        Some("ping") => json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }),
        Some("download") if v["url"].is_string() => match launch_app(&["--add", &URL_SAFE_NO_PAD.encode(msg)]) {
            Ok(()) => json!({ "ok": true }),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        },
        _ => json!({ "ok": false, "error": "unknown message" }),
    }
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

/// Pulls the `--add <payload>` the host passed on, if any.
pub fn parse_add_arg(args: &[String]) -> Option<crate::manager::Incoming> {
    let i = args.iter().position(|a| a == "--add")?;
    let raw = URL_SAFE_NO_PAD.decode(args.get(i + 1)?).ok()?;
    serde_json::from_slice(&raw).ok()
}
