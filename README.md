<p align="center"><img src="app/icon.svg" width="88" alt=""></p>
<h1 align="center">SwiftDL</h1>
<p align="center">A fast, resumable download manager with a browser extension.<br>Many connections per file, instant resume, free and open source.</p>

## Features

- **Faster downloads.** Each file is split across up to 32 connections. When one connection finishes, it takes half of the largest remaining part ("dynamic segmentation"), so every connection stays busy until the end.
- **Resume anything.** Pause, close the app, lose Wi-Fi or crash: progress is saved every second and picks up where it stopped.
- **Browser integration.** The extension catches downloads in Chrome, Edge, Brave and Vivaldi and sends them to SwiftDL with your cookies, so downloads that need a login still work. You can also right-click a link and choose *Download with SwiftDL*.
- **Live connection map.** See each connection filling its own part of the file in real time.
- **Queue and speed limit.** Choose how many downloads run at once and cap total bandwidth.
- **Tidy by default.** Files are sorted into Video, Music, Programs and similar folders.
- **No join step.** The file is pre-allocated and each connection writes straight into it, so there are no temp chunks and no waiting at 100%.

## Install

Download the latest release from the [Releases page](../../releases):

| Platform | File |
|---|---|
| Windows | `SwiftDL_x.y.z_x64-setup.exe` (no admin rights needed) |
| macOS | `SwiftDL_x.y.z_universal.dmg` |
| Linux | `.AppImage`, `.deb` or `.rpm` |

**Browser extension:** download `swiftdl-extension.zip`, unzip it, open `chrome://extensions` (or `edge://extensions`), turn on **Developer mode**, click **Load unpacked** and pick the folder. Open SwiftDL once so the extension can find it.

## How it works

```
Browser extension ──native messaging──▶ SwiftDL (same .exe, host mode)
                                              │ single-instance hand-off
                                              ▼
        UI (HTML/CSS/JS) ◀──events── Download manager ──▶ Engine (Rust, tokio)
                                      queue, settings,        probe → pre-allocate →
                                      persistence             N connections → resume file
```

| Path | What it is |
|---|---|
| `crates/engine` | The download engine: probing, segmentation, work stealing, resume, speed limiter |
| `crates/cli` | `swiftdl` command-line tool built on the engine |
| `app/src-tauri` | Desktop app (Tauri 2): download manager, native messaging host, browser registration |
| `app/ui` | The interface. Plain HTML/CSS/JS with no build step |
| `extension` | Chromium extension (Manifest V3) |
| `tools` | Throttled test server and a native-messaging test client |

### Benchmark

Local server limited to 2 MB/s **per connection**, as many real servers are. 64 MiB file:

| Connections | Time | Speed |
|---|---|---|
| 1 | 32.0 s | 2.0 MiB/s |
| 8 | 4.0 s | 15.9 MiB/s |
| 16 | 2.6 s | 25.1 MiB/s |

## Develop

```bash
cargo tauri dev            # run the desktop app (needs: cargo install tauri-cli)
cargo test --workspace     # engine tests
cargo tauri build          # build installers
```

**Working on the UI only?** Serve `app/ui` with any static server (for example `python -m http.server -d app/ui`) and open it in a browser. `mock.js` simulates downloads, so no Rust build is needed.

**Testing:** `python tools/throttle_server.py` serves a test file (and a page with a download link) at `http://127.0.0.1:8765` with per-connection throttling. `python tools/send_native_message.py <app.exe> ping` talks to the app the way the extension does.

### Windows toolchain notes

The release workflow builds with MSVC on GitHub's runners. Locally you can use either MSVC (Visual Studio Build Tools) or the `x86_64-pc-windows-gnu` toolchain with MinGW-w64 on `PATH`.

## Publishing the extension

`extension/manifest.json` contains a `key` that pins the extension ID to `knmfjbmojepkdiflnckpfjimfmjfcbgh`. The app only trusts IDs listed in `app/src-tauri/src/browser.rs`. When you publish to the Chrome Web Store, add the store's ID there too.

## License

MIT
