<p align="center"><img src="app/icon.svg" width="88" alt=""></p>
<h1 align="center">SwiftDL</h1>
<p align="center">A fast, resumable download manager with a browser extension.<br>Many connections per file, instant resume, free and open source.</p>

## Features

- **Faster downloads.** Each file is split across up to 32 connections. When one connection finishes, it takes half of the largest remaining part ("dynamic segmentation"), so every connection stays busy until the end.
- **Resume anything.** Pause, close the app, lose Wi-Fi or crash: progress is saved every second and picks up where it stopped.
- **Browser integration.** The extension catches downloads in Chrome, Edge, Brave and Vivaldi and sends them to SwiftDL with your cookies, so downloads that need a login still work. Right-click any link for *Download with SwiftDL*, or hold <kbd>Alt</kbd> while clicking to let the browser handle it.
- **Download video button.** A button appears over video players and lists the video and audio files the page has loaded.
- **A window for every download.** When the browser hands over a file, a small *New download* window asks where to save it. It then shows live progress, each connection's start position and progress, and finally a *Download complete* summary.
- **Live connection map.** See each connection filling its own part of the file in real time.
- **Lives in the tray.** Closing the window keeps downloads and browser integration running.
- **Queue and speed limit.** Choose how many downloads run at once and cap total bandwidth.
- **Tidy by default.** Files are sorted into Video, Music, Programs and similar folders.
- **No join step.** The file is pre-allocated and each connection writes straight into it, so there are no temp chunks and no waiting at 100%.

## Install

Download the latest release from the [Releases page](https://github.com/harshsinghsv/swiftdl/releases):

| Platform | File |
|---|---|
| Windows | `SwiftDL_x.y.z_x64-setup.exe` (no admin rights needed) |
| macOS | `SwiftDL_x.y.z_universal.dmg` |
| Linux | `.AppImage`, `.deb` or `.rpm` |

**Browser extension.** After you install SwiftDL and open it once, Chrome, Edge and Brave show a *New extension added* prompt the next time they start; click **Enable**. You can also use the **Add to Chrome / Add to Edge** buttons in the app. No browser allows an app to install an extension silently, and this prompt is the same route IDM uses.

To install it by hand instead, download `swiftdl-extension.zip` from the release, unzip it, open `chrome://extensions`, turn on **Developer mode**, click **Load unpacked** and choose the folder.

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

**Testing:** `python tools/throttle_server.py` serves a test file at `http://127.0.0.1:8765` with per-connection throttling, a page with a download link at `/`, and a page with a video player at `/video`. `python tools/send_native_message.py <app.exe> ping` talks to the app the way the extension does.

### Windows toolchain notes

The release workflow builds with MSVC on GitHub's runners. Locally you can use either MSVC (Visual Studio Build Tools) or the `x86_64-pc-windows-gnu` toolchain with MinGW-w64 on `PATH`.

## Publishing the extension

The automatic browser prompt only works once the extension is on the Chrome Web Store (Edge and Brave install from it too).

1. `extension/manifest.json` has a `key` that pins the extension ID to `knmfjbmojepkdiflnckpfjimfmjfcbgh`. The app trusts that ID (see `app/src-tauri/src/browser.rs`) and registers it with installed browsers.
2. For the first upload to the Web Store, include the private key as `key.pem` in the zip so the store keeps the same ID. The key isn't in the repository (`extension-key.pem` is in `.gitignore`), so keep it safe.

## Releasing

1. Update the version in `Cargo.toml`, `app/src-tauri/tauri.conf.json` and `extension/manifest.json`, and add a section to [CHANGELOG.md](CHANGELOG.md).
2. Commit with `chore(release): x.y.z` and tag it: `git tag -a vx.y.z -m "SwiftDL x.y.z"`.
3. `git push --follow-tags`. The release workflow builds installers for Windows, macOS and Linux plus the extension zip, and drafts a GitHub release.

## Contributing

Bug reports, ideas and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md). To report a security issue privately, see [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
