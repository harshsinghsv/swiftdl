// SwiftDL page helper.
// - Shows a "Download video" button over video players. Clicking it lists
//   the media files the page loaded and sends the chosen one to SwiftDL.
// - Alt+click on a link tells the background to let the browser handle
//   that one download itself.
// Everything lives in a closed shadow root so page styles can't touch it.

(() => {
  if (window.__swiftdlLoaded) return;
  window.__swiftdlLoaded = true;

  // Alt+click: skip SwiftDL for this download.
  document.addEventListener("click", (e) => {
    if (e.altKey && e.target.closest?.("a[href]")) chrome.runtime.sendMessage({ type: "bypass" }).catch(() => {});
  }, true);

  // Magnet links aren't downloads, so the browser would hand them to another
  // app. Send them to SwiftDL; if it can't take them, open the link as usual.
  document.addEventListener("click", async (e) => {
    const a = e.target.closest?.('a[href^="magnet:"]');
    if (!a || e.altKey || e.button !== 0 || e.defaultPrevented) return;
    e.preventDefault();
    e.stopPropagation();
    let res;
    try { res = await chrome.runtime.sendMessage({ type: "magnet", url: a.href }); } catch { res = null; }
    if (!res?.ok) location.href = a.href;
  }, true);

  const MIN_W = 200, MIN_H = 112;
  // Video sites SwiftDL downloads through yt-dlp: the page itself is sent,
  // and the quality is chosen in SwiftDL.
  const VIDEO_SITE = /(^|\.)(youtube\.com|youtube-nocookie\.com|vimeo\.com|x\.com|twitter\.com|reddit\.com|instagram\.com|tiktok\.com|facebook\.com|twitch\.tv|dailymotion\.com|bilibili\.com)$/i;
  const onVideoSite = () => VIDEO_SITE.test(location.hostname);
  const LOGO = `<svg viewBox="0 0 64 64" width="16" height="16" aria-hidden="true"><rect width="64" height="64" rx="14.5" fill="#24262b"/><g fill="#fff" transform="translate(13 13) scale(1.1875)"><rect x="4.5" y="3" width="4.4" height="12" rx="2.2"/><rect x="10.9" y="3" width="4.4" height="17" rx="2.2"/><rect x="16.7" y="3" width="4.4" height="17" rx="2.2"/><rect x="23.1" y="3" width="4.4" height="12" rx="2.2"/><path d="M6 21.5 16 28l10-6.5" fill="none" stroke="#fff" stroke-width="4.2" stroke-linecap="round" stroke-linejoin="round"/></g></svg>`;

  let host, root, wrap, btn, panel, video = null, hideTimer = 0, raf = 0, open = false;

  function build() {
    host = document.createElement("swiftdl-overlay");
    host.style.cssText = "all:initial;position:fixed;inset:0 auto auto 0;width:0;height:0;z-index:2147483647;";
    root = host.attachShadow({ mode: "closed" });
    root.innerHTML = `<style>
      :host { all: initial; }
      * { box-sizing: border-box; font-family: "Segoe UI Variable Text", "Segoe UI", -apple-system, system-ui, sans-serif; }
      /* max-content: anchored at the video's right edge, the wrap would
         otherwise only get the space left before the window edge, and on a
         large video "Download video" broke onto two lines. */
      .wrap { position: fixed; width: max-content; display: flex; flex-direction: column; align-items: flex-end; gap: 6px; transform: translateX(-100%); opacity: 0; transition: opacity .15s; pointer-events: none; }
      .wrap.on { opacity: 1; pointer-events: auto; }
      .btn { display: inline-flex; align-items: center; gap: 7px; height: 30px; padding: 0 12px 0 8px; white-space: nowrap; border: 0; border-radius: 9px;
        background: rgba(20, 21, 26, .86); color: #fff; font-size: 12.5px; font-weight: 600; letter-spacing: -.005em; cursor: pointer;
        box-shadow: 0 0 0 1px rgba(255,255,255,.12), 0 6px 20px rgba(0,0,0,.35); backdrop-filter: blur(14px) saturate(1.5); }
      .btn:hover { background: rgba(30, 31, 38, .94); }
      .btn .chev { opacity: .6; margin-left: 2px; transition: transform .15s; }
      .wrap.open .btn .chev { transform: rotate(180deg); }
      .panel { width: 320px; padding: 6px; border-radius: 12px; background: rgba(22, 23, 28, .94); color: #eef0f3;
        box-shadow: 0 0 0 1px rgba(255,255,255,.1), 0 16px 48px rgba(0,0,0,.5); backdrop-filter: blur(18px) saturate(1.5); display: none; }
      .wrap.open .panel { display: block; animation: pop .14s ease-out; }
      @keyframes pop { from { opacity: 0; transform: translateY(-4px); } }
      .head { padding: 6px 8px 6px; font-size: 11.5px; font-weight: 600; color: #8a8e98; }
      .item { display: flex; align-items: center; gap: 10px; width: 100%; padding: 8px; border: 0; border-radius: 8px; background: none; color: inherit; text-align: left; cursor: pointer; }
      .item:hover { background: rgba(255,255,255,.07); }
      .item[disabled] { cursor: default; opacity: .55; }
      .item[disabled]:hover { background: none; }
      .tag { flex: none; min-width: 42px; height: 20px; padding: 0 6px; border-radius: 5px; display: grid; place-items: center; font-size: 10.5px; font-weight: 700; letter-spacing: .03em;
        background: rgba(255,255,255,.1); color: #e6e8ec; }
      .tag.stream { background: rgba(255,255,255,.08); color: #8a8e98; }
      .meta { flex: 1; min-width: 0; }
      .meta b { display: block; font-size: 12.5px; font-weight: 600; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      .meta span { display: block; margin-top: 1px; font-size: 11.5px; color: #8a8e98; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
      /* quality list: one short row each, size on the right */
      .panel.q { width: 210px; }
      .qrow { display: flex; align-items: baseline; gap: 12px; width: 100%; height: 30px; padding: 0 10px; border: 0; border-radius: 7px; background: none; color: #eef0f3; cursor: pointer; font-size: 13px; font-weight: 600; font-variant-numeric: tabular-nums; align-items: center; }
      .qrow:hover { background: rgba(255,255,255,.08); }
      .qrow .sz { margin-left: auto; font-weight: 500; font-size: 12px; color: #8a8e98; }
      .qsep { height: 1px; margin: 4px 6px; background: rgba(255,255,255,.08); }
      .msg { padding: 10px 8px 12px; font-size: 12.5px; color: #a4a8b1; line-height: 1.5; }
      .msg.ok { color: #4cb782; } .msg.err { color: #eb6b6b; }
    </style>
    <div class="wrap"><button class="btn" title="Download with SwiftDL">${LOGO}<span>Download video</span><svg class="chev" width="10" height="10" viewBox="0 0 10 10"><path d="M2 3.5l3 3 3-3" stroke="#fff" stroke-width="1.4" fill="none" stroke-linecap="round"/></svg></button><div class="panel"></div></div>`;
    wrap = root.querySelector(".wrap");
    btn = root.querySelector(".btn");
    panel = root.querySelector(".panel");
    btn.addEventListener("click", (e) => { e.stopPropagation(); e.preventDefault(); toggle(); });
    panel.addEventListener("click", (e) => {
      e.stopPropagation();
      const item = e.target.closest(".item:not([disabled])");
      if (item) grab(item.dataset.url, item.dataset.name);
    });
    wrap.addEventListener("mouseenter", () => clearTimeout(hideTimer));
    wrap.addEventListener("mouseleave", () => scheduleHide());
    mount();
  }

  function mount() {
    const parent = document.fullscreenElement || document.documentElement;
    if (host && host.parentNode !== parent) parent.append(host);
  }
  document.addEventListener("fullscreenchange", () => host && mount());

  const bytes = (n) => {
    if (!n) return "";
    const u = ["B", "KB", "MB", "GB"]; let i = 0;
    while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
    return `${n < 10 && i ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
  };
  const extOf = (url, mime) => (/\.([a-z0-9]{2,4})(?:$|[?#])/i.exec(new URL(url).pathname) || [])[1]?.toLowerCase() || (mime.split("/")[1] || "").replace(/^x-/, "").slice(0, 4) || "mp4";
  const title = () => (document.title || "video").replace(/[\\/:*?"<>|]+/g, " ").replace(/\s+/g, " ").trim().slice(0, 120);

  function position() {
    if (!video || !wrap.classList.contains("on")) { raf = 0; return; }
    const r = video.getBoundingClientRect();
    if (r.width < MIN_W || r.height < MIN_H || r.bottom < 0 || r.top > innerHeight) { hide(); raf = 0; return; }
    wrap.style.left = `${Math.min(innerWidth - 8, r.right - 10)}px`;
    wrap.style.top = `${Math.max(8, r.top + 10)}px`;
    raf = requestAnimationFrame(position);
  }
  // YouTube: once the pointer has rested on a video for a moment, ask the app
  // to look it up, so the download can start the moment a quality is picked.
  // Once per video; a quick pass of the mouse doesn't count.
  // leftAt: when the pointer last moved off the video (a resting pointer
  // sends no events, so "left" is what can be seen, not "still there").
  let prepTimer = 0, leftAt = 0;
  const prepared = new Set();
  function prepareSoon() {
    // Called on every mouse move: start the countdown once, don't restart it.
    if (prepTimer || !/(^|\.)youtube\.com$/i.test(location.hostname)) return;
    const started = performance.now();
    prepTimer = setTimeout(async () => {
      prepTimer = 0;
      // The pointer moved off during the countdown: it was only passing.
      if (leftAt > started && !open) return;
      const info = await askPage();
      if (!info?.id || info.live || prepared.has(info.id) || !video) return;
      prepared.add(info.id);
      chrome.runtime.sendMessage({ type: "prepare", url: `https://www.youtube.com/watch?v=${info.id}` }).catch(() => {});
    }, 1000);
  }

  function show(v) {
    if (!host) build();
    mount();
    if (video !== v) { video = v; setOpen(false); }
    prepareSoon();
    clearTimeout(hideTimer);
    wrap.classList.add("on");
    if (!raf) raf = requestAnimationFrame(position);
  }
  function hide() { clearTimeout(prepTimer); prepTimer = 0; wrap?.classList.remove("on"); setOpen(false); video = null; }
  function scheduleHide() { clearTimeout(hideTimer); if (!open) hideTimer = setTimeout(hide, 1600); }
  function setOpen(v) { open = v; wrap?.classList.toggle("open", v); }

  // Which captured stream is the video being watched. `items` is newest first.
  // A tab that stays open collects streams from every video played in it, and
  // their links expire, so the oldest is often stale. Prefer the newest playlist
  // that lists several qualities (the main one); failing that, the oldest of the
  // recent ones, since a player loads its main playlist first.
  function pickStream(items) {
    const streams = items.filter((m) => m.kind === "hls" || m.kind === "dash");
    const main = streams.find((m) => m.qualities?.length > 1) || streams.find((m) => m.qualities?.length);
    if (main || !streams.length) return main || null;
    return streams.filter((m) => streams[0].at - m.at < 60000).at(-1);
  }

  async function toggle() {
    if (open) { setOpen(false); return; }
    panel.classList.remove("q");
    setOpen(true);
    if (/(^|\.)youtube\.com$/i.test(location.hostname) && (await youtubeMenu())) return;
    let res;
    try { res = await chrome.runtime.sendMessage({ type: "media" }); } catch { res = { items: [] }; }
    // Any site: if the player loaded a stream (HLS or DASH), list its
    // qualities now. The oldest one is the main playlist with every quality.
    const hls = pickStream(res?.items || []);
    if (hls?.qualities?.length) { qualityMenu(hls.url, title(), hls.qualities); return; }
    if (hls && (await streamMenu(hls.url))) return;
    if (onVideoSite()) return grab(location.href, title());
    panel.innerHTML = `<div class="msg">Looking for downloadable files…</div>`;
    const items = [...(res?.items || [])];
    const src = video?.currentSrc;
    if (src && /^https?:/i.test(src) && !items.some((m) => m.url === src)) items.unshift({ url: src, kind: "file", mime: "video/mp4", size: 0 });
    const files = items.filter((m) => m.kind === "file");
    // A player loads its main playlist first, then one per quality: the
    // oldest stream is the main one, which lets SwiftDL offer every quality.
    const stream = pickStream(items);
    const name = title();
    let html = "";
    if (files.length) {
      html += `<div class="head">Video and audio on this page</div>`;
      html += files.map((m) => {
        const ext = extOf(m.url, m.mime || "");
        const kind = /^audio/.test(m.mime) || /mp3|m4a|aac|ogg|opus|flac|wav/.test(ext) ? "Audio" : "Video";
        return `<button class="item" data-url="${encodeURI(m.url).replace(/"/g, "%22")}" data-name="${name.replace(/"/g, "'")}.${ext}"><span class="tag">${ext.toUpperCase()}</span><span class="meta"><b>${kind}${m.size ? ` · ${bytes(m.size)}` : ""}</b><span>${new URL(m.url).hostname}</span></span></button>`;
      }).join("");
    }
    if (stream) {
      html += `<div class="head">Stream</div><button class="item" data-url="${encodeURI(stream.url).replace(/"/g, "%22")}" data-name="${name.replace(/"/g, "'")}.mp4"><span class="tag stream">${stream.kind.toUpperCase()}</span><span class="meta"><b>This video</b><span>Choose the quality in SwiftDL</span></span></button>`;
    }
    if (!files.length && !stream) html = `<div class="msg">No downloadable file found yet. Play the video for a moment, then try again.</div>`;
    panel.innerHTML = html;
  }

  // Say what actually went wrong. The commonest case isn't SwiftDL at all:
  // the extension was updated or reloaded, which cuts off pages that were
  // already open until they're reloaded.
  function failure(error) {
    const text = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" })[c]);
    if (/context invalidated|receiving end does not exist/i.test(error) || !chrome.runtime?.id) return "SwiftDL's extension was updated. Reload this page and try again.";
    if (/host not found|not allowed/i.test(error)) return "SwiftDL isn't set up in this browser yet. Open the SwiftDL app once, then try again.";
    return `SwiftDL couldn't take this download: ${text(error) || "no answer from the app"}.`;
  }

  // ---------- YouTube: qualities straight from the page ----------
  // yt.js (inside the page) reads what the player already has. Same choices
  // and format selectors as the app's own lookup, so either way matches.
  const askPage = () => new Promise((resolve) => {
    const on = (e) => { if (e.source === window && e.data?.swiftdl === "yt-info") { removeEventListener("message", on); resolve(e.data); } };
    addEventListener("message", on);
    postMessage({ swiftdl: "yt-ask" }, "*");
    setTimeout(() => { removeEventListener("message", on); resolve(null); }, 800);
  });
  function ytQualities(formats) {
    const videos = formats.filter((f) => f.mime.startsWith("video/") && f.height > 0);
    const audios = formats.filter((f) => f.mime.startsWith("audio/"));
    const m4a = audios.filter((f) => f.mime.startsWith("audio/mp4"));
    const audioSize = Math.max(0, ...(m4a.length ? m4a : audios).map((f) => f.size));
    const heights = [...new Set(videos.map((f) => f.height))].sort((a, b) => b - a);
    const list = heights.map((h) => {
      const at = videos.filter((f) => f.height === h);
      const best = at.reduce((a, b) => (b.bitrate > a.bitrate ? b : a));
      const fps = Math.max(...at.map((f) => f.fps));
      return {
        label: fps > 30.5 ? `${h}p${Math.round(fps)}` : `${h}p`,
        format: `bv*[height=${h}][vcodec^=avc1]+ba[ext=m4a]/bv*[height=${h}]+ba[ext=m4a]/bv*[height=${h}]+ba/b[height=${h}]/bv*[height<=${h}]+ba/b`,
        ext: "mp4",
        size: best.size ? best.size + audioSize : null,
      };
    });
    if (audios.length) list.push({ label: "Audio only", format: "ba[ext=m4a]/ba", ext: m4a.length ? "m4a" : "webm", size: audioSize || null });
    return list;
  }
  async function youtubeMenu() {
    const info = await askPage();
    if (!info?.id || info.live || !info.formats.length) return false;
    const qualities = ytQualities(info.formats);
    if (!qualities.length) return false;
    const name = (info.title || title()).replace(/[\\/:*?"<>|]+/g, " ").replace(/\s+/g, " ").trim().slice(0, 120);
    // Just the video: a "list=" mix would make yt-dlp think about the playlist.
    const url = `https://www.youtube.com/watch?v=${info.id}`;
    qualityMenu(url, name, qualities);
    // Start the slow part now, unless hovering already did.
    if (!prepared.has(info.id)) { prepared.add(info.id); chrome.runtime.sendMessage({ type: "prepare", url }).catch(() => {}); }
    return true;
  }

  function qualityMenu(url, name, qualities) {
    const row = (q, i) => `<button class="qrow" data-q="${i}"><span>${q.label}</span><span class="sz">${q.size ? bytes(q.size) : ""}</span></button>`;
    const audio = qualities.findIndex((q) => q.label === "Audio only");
    panel.classList.add("q");
    panel.innerHTML = qualities.map((q, i) => (i === audio ? `<div class="qsep"></div>` : "") + row(q, i)).join("");
    panel.querySelectorAll("[data-q]").forEach((b) => b.addEventListener("click", (e) => {
      e.stopPropagation();
      const pick = Number(b.dataset.q);
      grab(url, `${name}.${qualities[pick].ext}`, { qualities, pick });
    }));
  }
  async function streamMenu(url) {
    panel.innerHTML = `<div class="msg">Reading qualities…</div>`;
    let res;
    try { res = await chrome.runtime.sendMessage({ type: "streamQualities", url }); } catch { res = null; }
    if (!res?.qualities?.length) return false;
    qualityMenu(url, title(), res.qualities);
    return true;
  }

  async function grab(url, filename, extra = {}) {
    panel.classList.remove("q");
    panel.innerHTML = `<div class="msg">Sending to SwiftDL…</div>`;
    let res;
    try { res = await chrome.runtime.sendMessage({ type: "grab", url: decodeURI(url), filename, ...extra }); } catch (e) { res = { ok: false, error: String(e?.message || e) }; }
    panel.innerHTML = res?.ok ? `<div class="msg ok">Sent to SwiftDL ✓</div>` : `<div class="msg err">${failure(res?.error || "")}</div>`;
    setTimeout(() => { setOpen(false); scheduleHide(); }, 1800);
  }

  // Find the video under the pointer, even when the site layers controls
  // on top of it.
  let last = 0, disabled = null;
  document.addEventListener("mousemove", async (e) => {
    const now = performance.now();
    if (now - last < 120) return;
    last = now;
    if (host && e.composedPath().includes(host)) return;
    const v = document.elementsFromPoint(e.clientX, e.clientY).find((el) => el.tagName === "VIDEO");
    if (v) {
      const r = v.getBoundingClientRect();
      if (r.width < MIN_W || r.height < MIN_H) return;
      if (disabled === null) {
        try { disabled = (await chrome.runtime.sendMessage({ type: "media" }))?.disabled ?? false; } catch { disabled = false; }
      }
      if (!disabled) show(v);
    } else if (video) { leftAt = now; scheduleHide(); }
  }, { passive: true });
  document.addEventListener("click", (e) => { if (open && host && !e.composedPath().includes(host)) setOpen(false); }, true);
  chrome.storage.onChanged.addListener((c) => { if (c.disabledSites) disabled = null; });
})();
