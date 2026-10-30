# Contributing to SwiftDL

Thanks for helping. Bug reports, ideas and pull requests are all welcome.

## Getting set up

You need Rust (stable) and, for the desktop app, the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/) for your OS.

```bash
cargo install tauri-cli --version "^2" --locked
cargo test --workspace          # engine tests
cargo tauri dev                 # run the app (from app/src-tauri)
```

**UI only:** serve `app/ui` with any static server and open it in a browser.
`mock.js` simulates downloads, so you don't need to build Rust:

```bash
python -m http.server 5179 -d app/ui
```

**Extension:** load `extension/` with *Load unpacked* on `chrome://extensions`.
`python tools/throttle_server.py` serves test files, a page with a download link
(`/`) and a page with a video (`/video`).

## Project layout

| Path | What it is |
|---|---|
| `crates/engine` | Download engine (no UI): probing, segmentation, resume, speed limit |
| `crates/cli` | Command-line front end |
| `app/src-tauri` | Desktop app backend: download manager, windows, tray, browser registration |
| `app/ui` | Interface, plain HTML/CSS/JS with no build step |
| `extension` | Chromium extension (Manifest V3) |
| `tools` | Test server and native-messaging client |

## Making changes

- Keep pull requests focused: one fix or feature each.
- Match the surrounding code style. Run `cargo fmt` and `cargo clippy --workspace`.
- Add or update tests for engine changes.
- For UI changes, check both light and dark mode and include a screenshot.
- Update `CHANGELOG.md` under **Unreleased**.

## Commit messages

We use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(extension): add a download button over video players
fix(engine): retry a segment after a dropped connection
docs: explain how to publish the extension
```

Common types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`.
Scopes: `engine`, `cli`, `app`, `ui`, `extension`, `release`.

## Reporting bugs

Open an issue with the bug report template. Include your OS, SwiftDL version,
browser (for extension issues) and, if possible, a link that reproduces it.
