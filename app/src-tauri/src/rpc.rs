//! A local, aria2-compatible JSON-RPC API, so tools that already talk to aria2
//! (AriaNg, scripts, automation) can control SwiftDL.
//!
//! It is off by default. It listens only on this computer, needs the secret
//! token on every call, and refuses requests whose Host isn't local (which
//! stops a web page from reaching it by DNS rebinding).

use crate::manager::{AddRequest, DownloadManager, Item, Status};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// What the API needs from the app. A fake stands in for it in tests.
pub trait Backend: Send + Sync {
    fn items(&self) -> Vec<Item>;
    fn add(&self, url: &str, dir: Option<&str>, out: Option<&str>) -> Result<u64, String>;
    fn pause(&self, id: u64);
    fn resume(&self, id: u64);
    fn remove(&self, id: u64);
}

impl Backend for Arc<DownloadManager> {
    fn items(&self) -> Vec<Item> {
        self.snapshot()
    }

    fn add(&self, url: &str, dir: Option<&str>, out: Option<&str>) -> Result<u64, String> {
        let name = out.map(str::to_string).unwrap_or_else(|| {
            let last = url.split(['?', '#']).next().unwrap_or("").rsplit('/').find(|p| !p.is_empty()).unwrap_or("download");
            percent_encoding::percent_decode_str(last).decode_utf8_lossy().into_owned()
        });
        let dir = dir.map(PathBuf::from).unwrap_or_else(|| self.default_dir(crate::manager::category_for(&name)));
        Ok(DownloadManager::add(
            self,
            AddRequest {
                url: url.to_string(),
                filename: name,
                dir,
                headers: vec![],
                start: true,
                window: None,
                format: None,
                size: None,
                torrent_files: None,
                checksum: None,
                mirrors: vec![],
            },
        ))
    }

    fn pause(&self, id: u64) {
        DownloadManager::pause(self, &[id]);
    }

    fn resume(&self, id: u64) {
        DownloadManager::resume(self, &[id]);
    }

    fn remove(&self, id: u64) {
        DownloadManager::remove(self, &[id], false);
    }
}

const METHODS: [&str; 20] = [
    "aria2.addUri", "aria2.remove", "aria2.forceRemove", "aria2.pause", "aria2.pauseAll", "aria2.forcePause", "aria2.forcePauseAll",
    "aria2.unpause", "aria2.unpauseAll", "aria2.tellStatus", "aria2.tellActive", "aria2.tellWaiting", "aria2.tellStopped",
    "aria2.getGlobalStat", "aria2.getVersion", "aria2.getGlobalOption", "aria2.getOption", "aria2.changeGlobalOption",
    "aria2.removeDownloadResult", "system.multicall",
];

fn gid(id: u64) -> String {
    format!("{id:016x}")
}

fn parse_gid(v: &Value) -> Result<u64, (i64, String)> {
    v.as_str().and_then(|s| u64::from_str_radix(s, 16).ok()).ok_or((1, "GID is not valid".to_string()))
}

fn aria_status(s: Status) -> &'static str {
    match s {
        Status::Downloading => "active",
        Status::Queued => "waiting",
        Status::Paused => "paused",
        Status::Completed => "complete",
        Status::Error => "error",
    }
}

/// One download in aria2's shape (all numbers as strings), cut down to `keys`.
fn status_json(item: &Item, keys: Option<&Value>) -> Value {
    let size = item.size.unwrap_or(0);
    let dir = item.path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let mut v = json!({
        "gid": gid(item.id),
        "status": aria_status(item.status),
        "totalLength": size.to_string(),
        "completedLength": item.downloaded.to_string(),
        "uploadLength": "0",
        "downloadSpeed": (item.speed.max(0.0) as u64).to_string(),
        "uploadSpeed": "0",
        "connections": item.connections.to_string(),
        "dir": dir,
        "errorCode": if item.status == Status::Error { "1" } else { "0" },
        "errorMessage": item.error.clone().unwrap_or_default(),
        "files": [{
            "index": "1",
            "path": item.path.to_string_lossy(),
            "length": size.to_string(),
            "completedLength": item.downloaded.to_string(),
            "selected": "true",
            "uris": [{ "uri": item.url, "status": "used" }],
        }],
    });
    if let (Some(keys), Some(obj)) = (keys.and_then(Value::as_array), v.as_object_mut()) {
        let wanted: Vec<&str> = keys.iter().filter_map(Value::as_str).collect();
        if !wanted.is_empty() {
            obj.retain(|k, _| wanted.contains(&k.as_str()));
        }
    }
    v
}

/// `offset` and `num` as aria2 takes them: a window over the list.
fn window<'a>(items: Vec<&'a Item>, offset: Option<&Value>, num: Option<&Value>) -> Vec<&'a Item> {
    let offset = offset.and_then(Value::as_u64).unwrap_or(0) as usize;
    let num = num.and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
    items.into_iter().skip(offset).take(num).collect()
}

type Failure = (i64, String);

fn call(method: &str, params: &[Value], be: &dyn Backend, secret: &str) -> Result<Value, Failure> {
    // Every call carries "token:SECRET" as its first parameter.
    let mut params = params.to_vec();
    // (A multicall carries none itself; each call inside it does.)
    if !matches!(method, "system.multicall" | "system.listMethods") {
        let token = params.first().and_then(Value::as_str).and_then(|s| s.strip_prefix("token:")).map(str::to_string);
        match token {
            Some(t) if !secret.is_empty() && t == secret => {
                params.remove(0);
            }
            _ => return Err((1, "Unauthorized".into())),
        }
    }
    let items = be.items();
    let of = |wanted: &[Status]| items.iter().filter(|i| wanted.contains(&i.status)).collect::<Vec<_>>();
    let keys = |n: usize| params.get(n).cloned();
    Ok(match method {
        "aria2.getVersion" => json!({ "version": env!("CARGO_PKG_VERSION"), "enabledFeatures": ["HTTP/HTTPS", "FTP", "BitTorrent", "Metalink"] }),
        "aria2.getGlobalStat" => {
            let speed: f64 = items.iter().map(|i| i.speed.max(0.0)).sum();
            let stopped = of(&[Status::Completed, Status::Error]).len();
            json!({
                "downloadSpeed": (speed as u64).to_string(), "uploadSpeed": "0",
                "numActive": of(&[Status::Downloading]).len().to_string(),
                "numWaiting": of(&[Status::Queued, Status::Paused]).len().to_string(),
                "numStopped": stopped.to_string(), "numStoppedTotal": stopped.to_string(),
            })
        }
        "aria2.addUri" => {
            let url = params.first().and_then(Value::as_array).and_then(|a| a.first()).and_then(Value::as_str).ok_or((1, "No address given".to_string()))?;
            if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("ftp://") || url.starts_with("magnet:")) {
                return Err((1, "Only http, https, ftp and magnet addresses are supported".into()));
            }
            let opts = params.get(1).and_then(Value::as_object);
            let text = |k: &str| opts.and_then(|o| o.get(k)).and_then(Value::as_str);
            Value::String(gid(be.add(url, text("dir"), text("out")).map_err(|e| (1, e))?))
        }
        "aria2.tellStatus" => {
            let id = parse_gid(params.first().unwrap_or(&Value::Null))?;
            let item = items.iter().find(|i| i.id == id).ok_or_else(|| (1, format!("GID {} is not found", gid(id))))?;
            status_json(item, keys(1).as_ref())
        }
        "aria2.tellActive" => Value::Array(of(&[Status::Downloading]).into_iter().map(|i| status_json(i, keys(0).as_ref())).collect()),
        "aria2.tellWaiting" => Value::Array(window(of(&[Status::Queued, Status::Paused]), params.first(), params.get(1)).into_iter().map(|i| status_json(i, keys(2).as_ref())).collect()),
        "aria2.tellStopped" => Value::Array(window(of(&[Status::Completed, Status::Error]), params.first(), params.get(1)).into_iter().map(|i| status_json(i, keys(2).as_ref())).collect()),
        "aria2.pause" | "aria2.forcePause" | "aria2.unpause" | "aria2.remove" | "aria2.forceRemove" | "aria2.removeDownloadResult" => {
            let id = parse_gid(params.first().unwrap_or(&Value::Null))?;
            if !items.iter().any(|i| i.id == id) {
                return Err((1, format!("GID {} is not found", gid(id))));
            }
            match method {
                "aria2.unpause" => be.resume(id),
                "aria2.pause" | "aria2.forcePause" => be.pause(id),
                _ => be.remove(id),
            }
            Value::String(gid(id))
        }
        "aria2.pauseAll" | "aria2.forcePauseAll" => {
            of(&[Status::Downloading, Status::Queued]).into_iter().for_each(|i| be.pause(i.id));
            json!("OK")
        }
        "aria2.unpauseAll" => {
            of(&[Status::Paused]).into_iter().for_each(|i| be.resume(i.id));
            json!("OK")
        }
        "aria2.getGlobalOption" | "aria2.getOption" => json!({}),
        "aria2.changeGlobalOption" => json!("OK"),
        "system.listMethods" => json!(METHODS),
        "system.multicall" => {
            // Each entry is {methodName, params}; the answers are one-element lists.
            let calls = params.first().and_then(Value::as_array).cloned().unwrap_or_default();
            Value::Array(calls.iter().map(|c| {
                let name = c["methodName"].as_str().unwrap_or("");
                let inner: Vec<Value> = c["params"].as_array().cloned().unwrap_or_default();
                match call(name, &inner, be, secret) {
                    Ok(v) => json!([v]),
                    Err((code, message)) => json!({ "code": code, "message": message }),
                }
            }).collect())
        }
        _ => return Err((1, format!("Method not found: {method}"))),
    })
}

/// One JSON-RPC request, or a batch of them, to its answer.
pub fn dispatch(req: &Value, be: &dyn Backend, secret: &str) -> Value {
    if let Some(batch) = req.as_array() {
        return Value::Array(batch.iter().map(|r| dispatch(r, be, secret)).collect());
    }
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let params: Vec<Value> = req["params"].as_array().cloned().unwrap_or_default();
    match call(req["method"].as_str().unwrap_or(""), &params, be, secret) {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    }
}

fn local_host(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    let name = h.rsplit_once(':').filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit())).map(|(n, _)| n).unwrap_or(&h);
    matches!(name, "127.0.0.1" | "localhost" | "[::1]")
}

fn header(name: &str, value: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(name.as_bytes(), value.as_bytes()).unwrap()
}

fn answer(req: tiny_http::Request, backend: &dyn Backend, secret: &str) {
    use std::io::Read;
    let cors = |r: tiny_http::Response<std::io::Cursor<Vec<u8>>>| {
        r.with_header(header("Access-Control-Allow-Origin", "*"))
            .with_header(header("Access-Control-Allow-Headers", "Content-Type"))
            .with_header(header("Access-Control-Allow-Methods", "POST, OPTIONS"))
    };
    let host = req.headers().iter().find(|h| h.field.equiv("Host")).map(|h| h.value.as_str().to_string()).unwrap_or_default();
    if !local_host(&host) {
        let _ = req.respond(tiny_http::Response::from_string("Forbidden").with_status_code(403));
        return;
    }
    if *req.method() == tiny_http::Method::Options {
        let _ = req.respond(cors(tiny_http::Response::from_data(Vec::new()).with_status_code(204)));
        return;
    }
    let path = req.url().split('?').next().unwrap_or("").to_string();
    if *req.method() != tiny_http::Method::Post || !(path == "/jsonrpc" || path == "/") {
        let _ = req.respond(tiny_http::Response::from_string("Not found").with_status_code(404));
        return;
    }
    let mut req = req;
    let mut body = String::new();
    let _ = req.as_reader().take(1 << 20).read_to_string(&mut body);
    let reply = match serde_json::from_str::<Value>(&body) {
        Ok(v) => dispatch(&v, backend, secret),
        Err(_) => json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": "Parse error" } }),
    };
    let data = reply.to_string().into_bytes();
    let _ = req.respond(cors(tiny_http::Response::from_data(data).with_header(header("Content-Type", "application/json"))));
}

/// Starts the API on `addr` (like "127.0.0.1:6800") and returns the port it
/// got. It runs on its own thread for as long as the app does.
pub fn serve(addr: &str, backend: Arc<dyn Backend>, secret: String) -> Result<u16, String> {
    let server = tiny_http::Server::http(addr).map_err(|e| e.to_string())?;
    let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
    std::thread::spawn(move || {
        for req in server.incoming_requests() {
            answer(req, &*backend, &secret);
        }
    });
    Ok(port)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::sync::Mutex;

    fn item(id: u64, status: &str, downloaded: u64) -> Item {
        serde_json::from_value(json!({
            "id": id, "url": format!("https://x.example/f{id}.zip"), "filename": format!("f{id}.zip"), "path": format!("C:/D/f{id}.zip"),
            "size": 1000, "downloaded": downloaded, "status": status, "category": "Compressed", "addedAt": 1, "resumable": true,
        }))
        .unwrap()
    }

    #[derive(Default)]
    struct Fake {
        items: Mutex<Vec<Item>>,
        log: Mutex<Vec<String>>,
    }

    impl Backend for Fake {
        fn items(&self) -> Vec<Item> {
            self.items.lock().unwrap().clone()
        }
        fn add(&self, url: &str, dir: Option<&str>, out: Option<&str>) -> Result<u64, String> {
            self.log.lock().unwrap().push(format!("add {url} {dir:?} {out:?}"));
            Ok(99)
        }
        fn pause(&self, id: u64) {
            self.log.lock().unwrap().push(format!("pause {id}"));
        }
        fn resume(&self, id: u64) {
            self.log.lock().unwrap().push(format!("resume {id}"));
        }
        fn remove(&self, id: u64) {
            self.log.lock().unwrap().push(format!("remove {id}"));
        }
    }

    fn fake() -> Fake {
        let f = Fake::default();
        *f.items.lock().unwrap() = vec![item(1, "downloading", 400), item(2, "queued", 0), item(3, "paused", 10), item(4, "completed", 1000), item(5, "error", 5)];
        f
    }

    fn rpc(f: &Fake, method: &str, params: Value) -> Value {
        dispatch(&json!({ "jsonrpc": "2.0", "id": "q", "method": method, "params": params }), f, "s3cret")
    }

    #[test]
    fn needs_the_token() {
        let f = fake();
        for params in [json!([]), json!(["token:wrong"]), json!(["nope"])] {
            assert_eq!(rpc(&f, "aria2.getVersion", params)["error"]["message"], "Unauthorized");
        }
        assert!(rpc(&f, "aria2.getVersion", json!(["token:s3cret"]))["result"]["version"].is_string());
        // No secret set on the server side: nothing is allowed.
        assert!(dispatch(&json!({ "id": 1, "method": "aria2.getVersion", "params": ["token:"] }), &f, "")["error"].is_object());
    }

    #[test]
    fn lists_downloads_the_aria2_way() {
        let f = fake();
        let t = "token:s3cret";
        let stat = &rpc(&f, "aria2.getGlobalStat", json!([t]))["result"];
        assert_eq!((stat["numActive"].as_str(), stat["numWaiting"].as_str(), stat["numStopped"].as_str()), (Some("1"), Some("2"), Some("2")));
        let active = rpc(&f, "aria2.tellActive", json!([t, ["gid", "status", "completedLength"]]));
        assert_eq!(active["result"][0], json!({ "gid": "0000000000000001", "status": "active", "completedLength": "400" }));
        let waiting = rpc(&f, "aria2.tellWaiting", json!([t, 0, 10]));
        assert_eq!(waiting["result"].as_array().unwrap().len(), 2);
        assert_eq!(rpc(&f, "aria2.tellWaiting", json!([t, 1, 1]))["result"][0]["status"], "paused");
        let stopped = rpc(&f, "aria2.tellStopped", json!([t, 0, 10, ["status", "errorCode"]]));
        assert_eq!(stopped["result"], json!([{ "status": "complete", "errorCode": "0" }, { "status": "error", "errorCode": "1" }]));
        let one = &rpc(&f, "aria2.tellStatus", json!([t, "0000000000000004"]))["result"];
        assert_eq!((one["totalLength"].as_str(), one["files"][0]["path"].as_str()), (Some("1000"), Some("C:/D/f4.zip")));
        assert!(rpc(&f, "aria2.tellStatus", json!([t, "00000000000000ff"]))["error"]["message"].as_str().unwrap().contains("not found"));
    }

    #[test]
    fn controls_downloads() {
        let f = fake();
        let t = "token:s3cret";
        let added = rpc(&f, "aria2.addUri", json!([t, ["https://a.example/x.iso"], { "dir": "D:/Get", "out": "x.iso" }]));
        assert_eq!(added["result"], "0000000000000063");
        assert!(rpc(&f, "aria2.addUri", json!([t, ["file:///etc/passwd"]]))["error"].is_object());
        rpc(&f, "aria2.pause", json!([t, "0000000000000001"]));
        rpc(&f, "aria2.unpause", json!([t, "0000000000000003"]));
        rpc(&f, "aria2.remove", json!([t, "0000000000000004"]));
        rpc(&f, "aria2.pauseAll", json!([t]));
        let log = f.log.lock().unwrap().clone();
        assert_eq!(log[0], "add https://a.example/x.iso Some(\"D:/Get\") Some(\"x.iso\")");
        assert_eq!(&log[1..4], ["pause 1", "resume 3", "remove 4"]);
        assert_eq!(&log[4..], ["pause 1", "pause 2"], "pauseAll stops what runs or waits");
    }

    #[test]
    fn answers_batches_and_multicalls() {
        let f = fake();
        let t = "token:s3cret";
        let batch = dispatch(&json!([{ "id": 1, "method": "aria2.getVersion", "params": [t] }, { "id": 2, "method": "nope", "params": [t] }]), &f, "s3cret");
        assert!(batch[0]["result"].is_object() && batch[1]["error"]["message"].as_str().unwrap().contains("Method not found"));
        let multi = rpc(&f, "system.multicall", json!([[{ "methodName": "aria2.getGlobalStat", "params": [t] }, { "methodName": "aria2.getVersion", "params": [] }]]));
        assert!(multi["result"][0][0]["numActive"].is_string());
        assert_eq!(multi["result"][1]["message"], "Unauthorized");
    }

    /// Plain HTTP to a real server on a free port.
    fn http(port: u16, host: &str, request_line: &str, body: &str) -> String {
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        write!(s, "{request_line} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    #[test]
    fn serves_over_http_to_local_callers_only() {
        let port = serve("127.0.0.1:0", Arc::new(fake()), "s3cret".into()).unwrap();
        let ok = http(port, &format!("127.0.0.1:{port}"), "POST /jsonrpc", r#"{"jsonrpc":"2.0","id":"a","method":"aria2.getVersion","params":["token:s3cret"]}"#);
        assert!(ok.starts_with("HTTP/1.1 200") && ok.contains("\"version\"") && ok.to_lowercase().contains("access-control-allow-origin"), "{ok}");
        let wrong = http(port, &format!("localhost:{port}"), "POST /jsonrpc", r#"{"id":1,"method":"aria2.getVersion","params":["token:no"]}"#);
        assert!(wrong.contains("Unauthorized"), "{wrong}");
        let rebound = http(port, &format!("evil.example:{port}"), "POST /jsonrpc", r#"{"id":1,"method":"aria2.getVersion","params":["token:s3cret"]}"#);
        assert!(rebound.starts_with("HTTP/1.1 403"), "{rebound}");
        let preflight = http(port, &format!("127.0.0.1:{port}"), "OPTIONS /jsonrpc", "");
        assert!(preflight.starts_with("HTTP/1.1 204"), "{preflight}");
        let garbage = http(port, &format!("127.0.0.1:{port}"), "POST /jsonrpc", "not json");
        assert!(garbage.contains("Parse error"), "{garbage}");
    }
}
