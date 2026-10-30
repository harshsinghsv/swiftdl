# Security policy

SwiftDL handles download links, cookies passed from your browser, and files on
disk, so we take security reports seriously.

## Reporting a vulnerability

Please **don't open a public issue**. Use GitHub's private
[security advisory](../../security/advisories/new) form for this repository
instead. Include steps to reproduce, the affected version and the impact you
expect.

We aim to acknowledge reports within a few days and to ship a fix or
mitigation as soon as we can, crediting you if you want.

## Supported versions

Security fixes go into the latest release.

## Scope notes

- The browser extension talks to the app only through native messaging, and
  the app accepts messages only from the extension IDs listed in
  `app/src-tauri/src/browser.rs`.
- Cookies captured for a download are stored with that download so it can
  resume, and are cleared when it completes.
