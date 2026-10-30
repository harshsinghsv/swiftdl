// Shared by every page: release data from GitHub, OS detection, and the
// small interactions (header border, reveal on scroll, benchmark bars).

const REPO = "harshsinghsv/swiftdl";
const GITHUB = `https://github.com/${REPO}`;

// Used when the GitHub API can't be reached (offline, rate limited), so the
// download buttons still point at real files.
const FALLBACK = [{
  tag_name: "v0.2.0",
  name: "SwiftDL 0.2.0",
  published_at: "2026-10-30T06:30:24Z",
  html_url: `${GITHUB}/releases/tag/v0.2.0`,
  body: "",
  assets: [
    ["SwiftDL_0.2.0_x64-setup.exe", 2736030], ["SwiftDL_0.2.0_x64_en-US.msi", 4026368],
    ["SwiftDL_0.2.0_universal.dmg", 6726278], ["SwiftDL_universal.app.tar.gz", 6649292],
    ["SwiftDL_0.2.0_amd64.AppImage", 84892152], ["SwiftDL_0.2.0_amd64.deb", 4613202],
    ["SwiftDL-0.2.0-1.x86_64.rpm", 4623331], ["swiftdl-extension.zip", 20749],
  ].map(([name, size]) => ({ name, size, browser_download_url: `${GITHUB}/releases/download/v0.2.0/${name}` })),
}];

async function getReleases() {
  const KEY = "swiftdl-releases";
  try {
    const cached = JSON.parse(sessionStorage.getItem(KEY) || "null");
    if (cached && Date.now() - cached.at < 10 * 60 * 1000) return cached.data;
  } catch {}
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/releases?per_page=50`, { headers: { Accept: "application/vnd.github+json" } });
    if (!r.ok) throw new Error(r.status);
    const data = (await r.json()).filter((x) => !x.draft && !x.prerelease);
    if (!data.length) throw new Error("no releases");
    try { sessionStorage.setItem(KEY, JSON.stringify({ at: Date.now(), data })); } catch {}
    return data;
  } catch {
    return FALLBACK;
  }
}

function detectOS() {
  const p = (navigator.userAgentData?.platform || navigator.platform || navigator.userAgent).toLowerCase();
  if (p.includes("win")) return "windows";
  if (p.includes("mac") || /iphone|ipad/.test(p)) return "macos";
  if (p.includes("linux") || p.includes("x11") || p.includes("cros")) return "linux";
  return "windows";
}

const OS_NAMES = { windows: "Windows", macos: "macOS", linux: "Linux" };

// What each release file is, in the order it should be offered.
const KINDS = [
  { test: /-setup\.exe$/i, os: "windows", label: "Installer", ext: ".exe", note: "Recommended. Installs for your user, no admin rights needed.", primary: true },
  { test: /\.msi$/i, os: "windows", label: "MSI package", ext: ".msi", note: "For managed installs and IT deployment." },
  { test: /\.dmg$/i, os: "macos", label: "Disk image", ext: ".dmg", note: "Recommended. Universal: Apple silicon and Intel.", primary: true },
  { test: /\.app\.tar\.gz$/i, os: "macos", label: "App archive", ext: ".app.tar.gz", note: "The app bundle on its own, for scripted installs." },
  { test: /\.AppImage$/i, os: "linux", label: "AppImage", ext: ".AppImage", note: "Recommended. Runs on most distributions without installing.", primary: true },
  { test: /\.deb$/i, os: "linux", label: "Debian package", ext: ".deb", note: "Ubuntu, Debian, Mint, Pop!_OS." },
  { test: /\.rpm$/i, os: "linux", label: "RPM package", ext: ".rpm", note: "Fedora, RHEL, openSUSE." },
  { test: /extension\.zip$/i, os: "extension", label: "Browser extension", ext: ".zip", note: "Already included with the app. Only needed without it." },
];

function classify(asset) {
  const k = KINDS.find((k) => k.test.test(asset.name));
  return k ? { ...k, order: KINDS.indexOf(k), asset } : null;
}

const filesFor = (release, os) => release.assets.map(classify).filter((f) => f && f.os === os).sort((a, b) => a.order - b.order);

function fmtSize(n) {
  if (!n) return "";
  const u = ["B", "KB", "MB", "GB"]; let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n < 10 && i ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
}
const fmtDate = (s) => new Date(s).toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" });
const version = (r) => r.tag_name.replace(/^v/, "");
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

const ICONS = {
  download: '<path d="M12 4v11M7.5 10.5 12 15l4.5-4.5M5 20h14"/>',
  windows: '<path d="M4 5.5 11 4.5v7H4zM13 4.2 20 3v8.5h-7zM4 12.5h7v7l-7-1zM13 12.5h7V21l-7-1z"/>',
  macos: '<path d="M15.5 3.5c-.9.1-2 .7-2.6 1.5-.6.7-1 1.7-.9 2.7 1 .1 2-.5 2.6-1.3.6-.8 1-1.8.9-2.9zM18.7 16.6c-.5 1.1-.7 1.6-1.4 2.6-.9 1.4-2.2 3.1-3.8 3.1-1.4 0-1.8-.9-3.7-.9s-2.4.9-3.8.9c-1.6 0-2.8-1.6-3.7-3-2.5-3.9-2.8-8.5-1.2-10.9 1.1-1.7 2.9-2.7 4.6-2.7 1.7 0 2.8.9 4.2.9 1.4 0 2.2-.9 4.2-.9 1.5 0 3.1.8 4.2 2.2-3.7 2-3.1 7.3.4 8.7z"/>',
  linux: '<path d="M12 3c-2 0-3.2 1.7-3.2 4 0 1.4.4 2.3-.4 3.7C7.3 12.6 5.5 15 6 17.5c.3 1.5 1.5 2.5 3 2.5h6c1.5 0 2.7-1 3-2.5.5-2.5-1.3-4.9-2.4-6.8-.8-1.4-.4-2.3-.4-3.7 0-2.3-1.2-4-3.2-4z"/><path d="M10.5 7.5h.01M13.5 7.5h.01"/>',
  github: '<path d="M9 19c-4.3 1.4-4.3-2.5-6-3m12 5v-3.5c0-1 .1-1.4-.5-2 2.8-.3 5.5-1.4 5.5-6a4.6 4.6 0 0 0-1.3-3.2 4.2 4.2 0 0 0-.1-3.2s-1.1-.3-3.5 1.3a12.3 12.3 0 0 0-6.2 0C6.5 2.8 5.4 3.1 5.4 3.1a4.2 4.2 0 0 0-.1 3.2A4.6 4.6 0 0 0 4 9.5c0 4.6 2.7 5.7 5.5 6-.6.6-.6 1.2-.5 2V21"/>',
  arrow: '<path d="M5 12h14M13 6l6 6-6 6"/>',
  external: '<path d="M14 4h6v6M20 4l-9 9M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>',
  puzzle: '<path d="M9 4h3.5a1.5 1.5 0 0 1 3 0H19v3.5a1.5 1.5 0 0 1 0 3V14h-3.5a1.5 1.5 0 0 0-3 0H9v-3.5a1.5 1.5 0 0 0 0-3z"/><path d="M9 14v6H4v-6"/>',
};
const icon = (name, cls = "") => `<svg class="${cls}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[name]}</svg>`;

// Fills every [data-version] with the latest version, and every
// [data-download] button with the right installer for this visitor.
async function hydrateLatest() {
  const [latest] = await getReleases();
  const os = detectOS();
  document.querySelectorAll("[data-version]").forEach((el) => { el.textContent = `v${version(latest)}`; });
  const main = filesFor(latest, os).find((f) => f.primary);
  document.querySelectorAll("[data-download]").forEach((el) => {
    el.href = main ? main.asset.browser_download_url : "download.html";
    el.querySelector("[data-os]")?.replaceChildren(OS_NAMES[os]);
    const meta = el.parentElement.querySelector("[data-download-meta]");
    if (meta && main) meta.textContent = `v${version(latest)} · ${main.ext} · ${fmtSize(main.asset.size)}`;
  });
  document.querySelectorAll("[data-os-icon]").forEach((el) => { el.innerHTML = icon(os); });
  return latest;
}

function initChrome() {
  const header = document.querySelector(".site-header");
  if (header) {
    const onScroll = () => header.classList.toggle("scrolled", scrollY > 4);
    addEventListener("scroll", onScroll, { passive: true });
    onScroll();
  }
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      e.target.classList.add("in", "on");
      io.unobserve(e.target);
    }
  }, { threshold: 0.15 });
  document.querySelectorAll(".reveal, .bar").forEach((el) => io.observe(el));
  document.querySelectorAll("[data-icon]").forEach((el) => { el.innerHTML = icon(el.dataset.icon); });
  const y = document.querySelector("[data-year]");
  if (y) y.textContent = new Date().getFullYear();
}

document.addEventListener("DOMContentLoaded", initChrome);
