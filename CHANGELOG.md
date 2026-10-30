# Changelog

All notable changes to SwiftDL are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **YouTube and other video sites.** The video button, the popup and the New
  download window offer each quality, from 4K down to audio only. These sites
  go through yt-dlp, with ffmpeg joining video and audio; both are fetched on
  first use (ffmpeg comes from the system on macOS and Linux) and yt-dlp is
  checked for updates daily. The download window shows yt-dlp's progress and
  steps such as merging.

### Changed
- **Adding the extension is a guided step.** Until the extension is on the
  Chrome Web Store, *Add the extension* opens a four-step guide that points at
  the copy SwiftDL installs, with a button to open that folder, instead of
  store buttons that led nowhere. The automatic-offer switch is hidden until
  the listing exists.
- The README now points at the installed extension folder rather than the zip.

### Fixed
- Settings → Browser showed every browser as needing the extension even when
  it was connected. SwiftDL now records which browser the extension talked to
  from, and marks that browser as connected.

## [0.2.0] - 2026-10-30

### Added
- **A window for every download.** A file caught from the browser opens a
  small *New download* window (address, category, save location, size) instead
  of the main window. Starting it turns the same window into live progress, with
  each connection's start position and progress, and then a *Download complete*
  summary with Open and Show in folder.
- **Download video button.** The extension detects video and audio files a page
  loads and shows a button over video players that lists them. The popup lists
  them too, and the toolbar icon shows how many were found.
- **Automatic extension offer.** SwiftDL registers its extension with installed
  Chrome, Edge and Brave, which show a one-time *New extension added* prompt
  (requires the Chrome Web Store listing). A *Connect your browser* bar and a
  Browser settings page show which browsers were found and whether the
  extension has connected.
- **Alt+click bypass**, a per-site switch and a small-file threshold in the
  extension.
- **System tray.** Closing the main window keeps downloads and browser
  integration running; the tray menu opens or quits SwiftDL.
- The engine reports each connection's start, position and end.
- The uninstaller removes SwiftDL's browser registrations.
- Test pages for the extension in `tools/throttle_server.py`.

### Changed
- Calmer visual design: neutral buttons, one system-blue accent for progress
  and focus, no gradients, label/value details instead of boxed stat cards.
- New graphite logo.
- Redesigned extension popup in the app's style.
- Connections are shown as one rounded slot per connection, in both the main
  window and the download window.
- Softer file-type tiles throughout the app.

### Fixed
- Frameless windows no longer show the light Windows 11 border, and download
  windows no longer show white edges around their frame.

## [0.1.0] - 2026-10-30

### Added
- Segmented, resumable download engine: multiple connections per file, dynamic
  segmentation (idle connections take half of the largest remaining part),
  pre-allocated output written in place, resume file saved every second,
  retries with backoff, and a global speed limiter.
- `swiftdl` command-line tool.
- Desktop app (Tauri 2) with a queue, categories, settings, notifications,
  command palette, inspector with speed chart, and light and dark themes.
- Chromium extension (Manifest V3) that hands browser downloads to the app with
  cookies, referrer and user agent, plus a *Download with SwiftDL* context menu.
- The app doubles as the native messaging host and registers itself per user.
- GitHub Actions for CI and tagged releases (Windows, macOS, Linux and the
  extension zip).

[Unreleased]: https://github.com/harshsinghsv/swiftdl/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/harshsinghsv/swiftdl/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/harshsinghsv/swiftdl/releases/tag/v0.1.0
