// SwiftDL browser integration.
//
// 1. Catches downloads the browser starts and hands them to the SwiftDL app
//    (through native messaging), with the cookies and referrer the site
//    expects, so downloads behind a login keep working. If the app isn't
//    installed or refuses, the browser simply finishes the download itself.
// 2. Watches each tab's network traffic for video and audio files so the
//    page button and the popup can offer them for download.

const HOST = "com.swiftdl.host";
const DEFAULTS = { enabled: true, minSizeKB: 0, disabledSites: [] };

const getSettings = () => chrome.storage.sync.get(DEFAULTS);
const hostOf = (url) => { try { return new URL(url).hostname.replace(/^www\./, ""); } catch { return ""; } };

// Last few events, kept for troubleshooting (service worker console or
// chrome.storage.local "log").
async function log(event) {
  console.log("SwiftDL:", event);
  const { log: entries = [] } = await chrome.storage.local.get("log");
  entries.push(`${new Date().toISOString()} ${event}`);
  await chrome.storage.local.set({ log: entries.slice(-30) });
}

// ---------- talking to the app ----------

async function cookieHeader(url) {
  try {
    const cookies = await chrome.cookies.getAll({ url });
    return cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  } catch {
    return "";
  }
}

async function sendToApp({ url, referrer, filename, qualities, pick }) {
  const message = {
    type: "download",
    url,
    referrer: referrer || "",
    filename: filename ? filename.split(/[\\/]/).pop() : "",
    cookies: await cookieHeader(url),
    userAgent: navigator.userAgent,
    // A video's qualities, read from the page, so the app needn't look them up.
    ...(qualities ? { qualities, pick } : {}),
  };
  // The browser starts SwiftDL for this message even when the app is
  // closed. A start that fails once (the computer is busy, an update is
  // still being written) usually works a moment later, so try twice.
  let reply;
  try {
    reply = await chrome.runtime.sendNativeMessage(HOST, message);
  } catch (e) {
    await log(`native host failed, retrying: ${e?.message || e}`);
    await new Promise((r) => setTimeout(r, 800));
    reply = await chrome.runtime.sendNativeMessage(HOST, message);
  }
  if (!reply?.ok) throw new Error(reply?.error || "SwiftDL did not accept the download");
}

const ping = () =>
  chrome.runtime.sendNativeMessage(HOST, { type: "ping" })
    .then((r) => ({ ok: !!r?.ok, version: r?.version }))
    .catch((e) => ({ ok: false, error: String(e?.message || e) }));

function flashBadge(text, color, tabId) {
  chrome.action.setBadgeBackgroundColor({ color });
  chrome.action.setBadgeText({ text, ...(tabId ? { tabId } : {}) });
  setTimeout(() => refreshBadge(tabId), 2500);
}

// ---------- catching browser downloads ----------

// Hold Alt while clicking a link to let the browser download it instead.
let bypassUntil = 0;

async function siteDisabled(url) {
  const { disabledSites } = await getSettings();
  const host = hostOf(url);
  return !!host && disabledSites.includes(host);
}

// "Download in browser" in the app opens the link with #swiftdl-browser on the
// end. The browser then keeps that one download, with its own session.
const browserOnce = new Map();
const markBrowser = (u) => { if (typeof u === "string" && u.includes("#swiftdl-browser")) browserOnce.set(u.split("#")[0], Date.now() + 60000); };
chrome.tabs.onCreated.addListener((t) => markBrowser(t.pendingUrl || t.url));
chrome.tabs.onUpdated.addListener((_id, info) => markBrowser(info.url));

chrome.downloads.onCreated.addListener(async (item) => {
  const settings = await getSettings();
  const url = item.finalUrl || item.url;
  await log(`created #${item.id} state=${item.state} ${url}`);
  if (!settings.enabled || item.state !== "in_progress") return;
  // blob:, data: and file: URLs only exist inside the browser.
  if (!/^https?:/i.test(url)) return;
  if (Date.now() < bypassUntil) { bypassUntil = 0; await log(`#${item.id} Alt held: left to the browser`); return; }
  if ([item.url, item.finalUrl, url].some((u) => u && (browserOnce.get(String(u).split("#")[0]) || 0) > Date.now())) { await log(`#${item.id} kept by the browser, as asked in SwiftDL`); return; }
  if (await siteDisabled(item.referrer || url)) return;
  if (settings.minSizeKB > 0 && item.totalBytes > 0 && item.totalBytes < settings.minSizeKB * 1024) return;

  // Pause first so the browser doesn't keep downloading while we ask the app.
  await chrome.downloads.pause(item.id).catch(() => {});
  try {
    await sendToApp({ url, referrer: item.referrer, filename: item.filename });
    await chrome.downloads.cancel(item.id).catch(() => {});
    await chrome.downloads.erase({ id: item.id }).catch(() => {});
    await log(`#${item.id} handed to SwiftDL`);
    flashBadge("✓", "#2f9e6a");
  } catch (err) {
    await log(`#${item.id} left to the browser: ${err?.message || err}`);
    await chrome.downloads.resume(item.id).catch(() => {});
    flashBadge("!", "#dc4b4b");
  }
});

// ---------- media detection ----------
// Per tab, the video/audio files and streams the page has loaded. Kept in
// session storage because the service worker can be stopped at any time.

const MEDIA_EXT = /\.(mp4|m4v|webm|mkv|mov|avi|flv|wmv|mp3|m4a|aac|ogg|oga|opus|flac|wav)(?:$|[?#])/i;
const STREAM_EXT = /\.(m3u8|mpd)(?:$|[?#])/i;
const SEGMENT = /\.(ts|m4s|aac)(?:$|[?#])|[?&](range|bytestart)=/i;
const MIN_FILE_BYTES = 512 * 1024;
const key = (tabId) => `media:${tabId}`;

function classify(url, mime, size) {
  if (/mpegurl/i.test(mime) || /\.m3u8(?:$|[?#])/i.test(url)) return "hls";
  if (/dash\+xml/i.test(mime) || /\.mpd(?:$|[?#])/i.test(url)) return "dash";
  const isMedia = /^(video|audio)\//i.test(mime) || MEDIA_EXT.test(url);
  if (!isMedia || /mp2t/i.test(mime) || SEGMENT.test(url)) return null;
  if (size && size < MIN_FILE_BYTES) return null;
  return "file";
}

function header(headers, name) {
  return headers?.find((h) => h.name.toLowerCase() === name)?.value || "";
}

chrome.webRequest.onResponseStarted.addListener(
  async (d) => {
    if (d.tabId < 0 || d.statusCode >= 400) return;
    const mime = header(d.responseHeaders, "content-type").split(";")[0].trim().toLowerCase();
    const range = header(d.responseHeaders, "content-range");
    const size = Number(range.split("/")[1]) || Number(header(d.responseHeaders, "content-length")) || 0;
    const kind = classify(d.url, mime, size);
    if (!kind) return;
    const url = d.url.replace(/([?&])(range|rn|rbuf)=[^&]*/gi, "$1").replace(/[?&]$/, "");
    const { [key(d.tabId)]: list = [] } = await chrome.storage.session.get(key(d.tabId));
    if (list.some((m) => m.url === url)) return;
    list.unshift({ url, kind, mime, size, at: Date.now() });
    await chrome.storage.session.set({ [key(d.tabId)]: list.slice(0, 25) });
    refreshBadge(d.tabId);
    // Read a stream's qualities now, while the video plays, so the menu has
    // them the moment it opens. (Our own fetch has no tab, so it isn't caught.)
    if (kind === "hls" || kind === "dash") {
      streamQualities(url).then(async (qualities) => {
        if (!qualities) return;
        const { [key(d.tabId)]: now = [] } = await chrome.storage.session.get(key(d.tabId));
        const item = now.find((m) => m.url === url);
        if (!item) return;
        item.qualities = qualities;
        await chrome.storage.session.set({ [key(d.tabId)]: now });
      }).catch(() => {});
    }
  },
  { urls: ["http://*/*", "https://*/*"], types: ["media", "xmlhttprequest", "other", "object"] },
  ["responseHeaders"],
);

// ---------- reading a stream's qualities (HLS) ----------
// The player already loaded this playlist; reading it again takes a moment
// and tells us every quality, so the menu can show them at once. Each choice
// carries the exact playlists for the app's own downloader.

const hlsAttr = (line, name) => {
  const m = new RegExp(`(?:^|[:,])${name}=("([^"]*)"|[^,]*)`, "i").exec(line);
  return m ? (m[2] ?? m[1]) : null;
};

async function streamQualities(url) {
  const get = async (u) => {
    const r = await fetch(u, { credentials: "include" });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    return r.text();
  };
  const text = await get(url);
  if (/<MPD[\s>]/.test(text)) return dashQualities(text, url);
  if (!text.trimStart().startsWith("#EXTM3U")) return null;
  const lines = text.split(/\r?\n/).map((l) => l.trim());
  const abs = (u) => new URL(u, url).href;
  // One playlist, no choices: download it as it is.
  if (!lines.some((l) => l.startsWith("#EXT-X-STREAM-INF"))) {
    return [{ label: "Original", format: `hls|${url}|`, ext: "mp4", size: null }];
  }
  const audio = {};
  for (const l of lines.filter((l) => l.startsWith("#EXT-X-MEDIA") && /TYPE=AUDIO/i.test(l))) {
    const g = hlsAttr(l, "GROUP-ID"), u = hlsAttr(l, "URI");
    if (g && u && (!audio[g] || /DEFAULT=YES/i.test(l))) audio[g] = abs(u);
  }
  const variants = [];
  lines.forEach((l, i) => {
    if (!l.startsWith("#EXT-X-STREAM-INF")) return;
    const uri = lines.slice(i + 1).find((x) => x && !x.startsWith("#"));
    if (!uri) return;
    const res = hlsAttr(l, "RESOLUTION");
    const height = res ? Number(res.split("x")[1]) || 0 : 0;
    const bw = Number(hlsAttr(l, "AVERAGE-BANDWIDTH") || hlsAttr(l, "BANDWIDTH")) || 0;
    const fps = Number(hlsAttr(l, "FRAME-RATE")) || 0;
    const group = hlsAttr(l, "AUDIO");
    variants.push({ height, bw, fps, video: abs(uri), audio: group ? audio[group] || "" : "" });
  });
  // Best first; one entry per height (the highest bitrate at that height).
  variants.sort((a, b) => b.height - a.height || b.bw - a.bw);
  const seen = new Set();
  const picked = variants.filter((v) => { const k = v.height || v.bw; if (seen.has(k)) return false; seen.add(k); return true; });
  if (!picked.length) return null;
  // Length of the video, from one of its playlists, to estimate sizes.
  let secs = 0;
  try {
    secs = (await get(picked[picked.length - 1].video)).split(/\r?\n/)
      .filter((l) => l.startsWith("#EXTINF:")).reduce((s, l) => s + (parseFloat(l.slice(8)) || 0), 0);
  } catch { secs = 0; }
  return picked.map((v) => ({
    label: v.height ? `${v.height}p${v.fps > 30.5 ? Math.round(v.fps) : ""}` : `${Math.round(v.bw / 1000)} kbps`,
    format: `hls|${v.video}|${v.audio}`,
    ext: "mp4",
    size: secs && v.bw ? Math.round((v.bw * secs) / 8) : null,
  }));
}

// A DASH manifest's qualities. No XML parser in a service worker, but the
// attributes needed here are plain enough to read directly.
function dashQualities(xml, url) {
  if (/<MPD[^>]*type="dynamic"/.test(xml)) return null; // live
  const iso = (s) => { const m = /P(?:(\d+)D)?T?(?:([\d.]+)H)?(?:([\d.]+)M)?(?:([\d.]+)S)?/.exec(s || ""); return m ? (+m[1] || 0) * 86400 + (+m[2] || 0) * 3600 + (+m[3] || 0) * 60 + (+m[4] || 0) : 0; };
  const secs = iso((/mediaPresentationDuration="([^"]+)"/.exec(xml) || [])[1]);
  const at = (tag, name) => { const m = new RegExp(`\\s${name}="([^"]*)"`).exec(tag); return m ? m[1] : ""; };
  // Frame rates may be fractions: "30000/1001", "12800/512".
  const rate = (s) => { const [n, d] = String(s || "").split("/").map(Number); return n ? n / (d || 1) : 0; };
  const video = [], audio = [];
  // Each adaptation set, then its tracks; a track inherits its set's type.
  for (const set of xml.split(/<AdaptationSet\b/).slice(1)) {
    const head = set.slice(0, set.indexOf(">") + 1);
    const setType = at(head, "contentType") || at(head, "mimeType");
    for (const m of set.matchAll(/<Representation\b[^>]*>/g)) {
      const r = m[0], kind = `${at(r, "mimeType") || setType} ${at(r, "codecs") || at(head, "codecs")}`;
      const t = { id: at(r, "id"), bw: +at(r, "bandwidth") || 0, height: +at(r, "height") || +at(head, "height") || 0, fps: rate(at(r, "frameRate") || at(head, "frameRate")) };
      if (!t.id || /text|wvtt|stpp|ttml|image/i.test(kind)) continue;
      if (/audio|mp4a|opus|ac-3|ec-3/i.test(kind)) audio.push(t); else if (/video|avc|hvc|hev|vp0?9|av01/i.test(kind) || t.height) video.push(t);
    }
  }
  const a = audio.sort((x, y) => y.bw - x.bw)[0];
  video.sort((x, y) => y.height - x.height || y.bw - x.bw);
  const seen = new Set();
  const list = video.filter((v) => !seen.has(v.height) && seen.add(v.height)).map((v) => ({
    label: v.height ? `${v.height}p${v.fps > 30.5 ? Math.round(v.fps) : ""}` : `${Math.round(v.bw / 1000)} kbps`,
    format: `dash|${url}|${v.id}|${a ? a.id : ""}`,
    ext: "mp4",
    size: secs ? Math.round(((v.bw + (a?.bw || 0)) * secs) / 8) : null,
  }));
  if (a) list.push({ label: "Audio only", format: `dash|${url}||${a.id}`, ext: "m4a", size: secs ? Math.round((a.bw * secs) / 8) : null });
  return list.length ? list : null;
}

async function mediaFor(tabId) {
  const { [key(tabId)]: list = [] } = await chrome.storage.session.get(key(tabId));
  return list;
}

async function refreshBadge(tabId) {
  if (tabId == null) { chrome.action.setBadgeText({ text: "" }); return; }
  // Files, plus one for a stream (its playlists are one video).
  const media = await mediaFor(tabId);
  const count = media.filter((m) => m.kind === "file").length + (media.some((m) => m.kind !== "file") ? 1 : 0);
  chrome.action.setBadgeBackgroundColor({ color: "#0a84ff", tabId });
  chrome.action.setBadgeText({ text: count ? String(count) : "", tabId });
}

// A new page in the tab starts a fresh list.
chrome.tabs.onUpdated.addListener((tabId, info) => {
  if (info.status === "loading" && info.url) chrome.storage.session.remove(key(tabId)).then(() => refreshBadge(tabId));
});
chrome.tabs.onRemoved.addListener((tabId) => chrome.storage.session.remove(key(tabId)));

// ---------- right-click menu ----------

function createMenus() {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({ id: "swiftdl-link", title: "Download with SwiftDL", contexts: ["link"] });
    chrome.contextMenus.create({ id: "swiftdl-media", title: "Download with SwiftDL", contexts: ["video", "audio", "image"] });
  });
}
chrome.runtime.onInstalled.addListener(() => { createMenus(); ping(); });
chrome.runtime.onStartup.addListener(() => { createMenus(); ping(); });
// Also check in whenever the extension loads (added, reloaded or turned back
// on), so the app notices the connection right away. Session storage is
// cleared when the extension is reloaded or turned off, so this runs once per load.
chrome.storage.session.get("checkedIn").then(({ checkedIn }) => {
  if (!checkedIn) chrome.storage.session.set({ checkedIn: true }).then(ping);
}).catch(() => {});

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const url = info.linkUrl || info.srcUrl;
  if (!url || !/^https?:/i.test(url)) return flashBadge("!", "#dc4b4b", tab?.id);
  try {
    await sendToApp({ url, referrer: info.pageUrl || tab?.url });
    flashBadge("✓", "#2f9e6a", tab?.id);
  } catch (err) {
    await log(`context menu: could not reach the app: ${err?.message || err}`);
    flashBadge("!", "#dc4b4b", tab?.id);
  }
});

// ---------- messages from the page button and the popup ----------

chrome.runtime.onMessage.addListener((msg, sender, respond) => {
  const tabId = msg.tabId ?? sender.tab?.id;
  const pageUrl = msg.pageUrl || sender.tab?.url || "";
  (async () => {
    switch (msg?.type) {
      case "ping": return ping();
      case "bypass": bypassUntil = Date.now() + 4000; return { ok: true };
      case "media": return { items: await mediaFor(tabId), disabled: await siteDisabled(pageUrl) };
      case "prepare":
        try {
          await chrome.runtime.sendNativeMessage(HOST, { type: "prepare", url: msg.url, referrer: pageUrl, cookies: await cookieHeader(msg.url), userAgent: navigator.userAgent });
        } catch { /* only a head start; the download works without it */ }
        return { ok: true };
      case "streamQualities":
        try { return { qualities: await streamQualities(msg.url) }; } catch (e) { return { qualities: null, error: String(e?.message || e) }; }
      case "grab":
        try {
          await sendToApp({ url: msg.url, referrer: pageUrl, filename: msg.filename, qualities: msg.qualities, pick: msg.pick });
          return { ok: true };
        } catch (e) {
          return { ok: false, error: String(e?.message || e) };
        }
      case "magnet": {
        const { enabled } = await getSettings();
        if (!enabled || (await siteDisabled(pageUrl))) return { ok: false };
        try {
          await sendToApp({ url: msg.url, referrer: pageUrl });
          flashBadge("✓", "#2f9e6a", tabId);
          return { ok: true };
        } catch (e) {
          return { ok: false, error: String(e?.message || e) };
        }
      }
      case "show":
        return chrome.runtime.sendNativeMessage(HOST, { type: "show" }).catch((e) => ({ ok: false, error: String(e) }));
      case "toggle-site": {
        const { disabledSites } = await getSettings();
        const host = hostOf(pageUrl);
        const next = disabledSites.includes(host) ? disabledSites.filter((h) => h !== host) : [...disabledSites, host];
        await chrome.storage.sync.set({ disabledSites: next });
        return { disabled: next.includes(host) };
      }
    }
  })().then(respond);
  return true; // async response
});
