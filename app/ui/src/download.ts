// The per-download window, like IDM's:
//   download.html?confirm=<token>  "New download": a file caught from the
//                                  browser, waiting for Start / Later / Cancel
//   download.html?id=<id>          live progress, then "Download complete"

import "./styles.css";
import "./download.css";
import { invoke, listen, currentWindow, LogicalSize, $ } from "./api";
import type { Item, Incoming, Header, MediaInfo, TorrentInfo, AddRequest, ProbeInfo, Settings, Quality } from "./api";
import { icon, CATS, catOf, bytes, speed, dur, when, esc, host, catInfo, segBar } from "./util";
import { t, tn, tt, tb, locale, setLang, resolveLang } from "./i18n";

const win = currentWindow();

const glyph = (cat: string, _name = "") => `<span class="glyph">${icon(catInfo(cat).icon)}</span>`;
// Download windows show the full date for anything older than an hour.
const whenFull = (ms: number | null) => {
  if (!ms) return "";
  return Date.now() - ms < 3600_000 ? when(ms) : new Date(ms).toLocaleString(locale(), { dateStyle: "medium", timeStyle: "short" });
};

const view = $("view");

// The window grows and shrinks to fit what it shows.
const fit = () => {
  if (!win || !LogicalSize) return;
  // The card plus the 16px transparent margin on each side (for its shadow).
  const root = $("root");
  const h = Math.ceil(Math.max(root.getBoundingClientRect().height, root.scrollHeight)) + 32;
  // A resize that fails (or lands before the layout settles) must not leave the buttons cut off.
  Promise.resolve(win.setSize(new LogicalSize(552, Math.min(780, h)))).catch(() => setTimeout(fit, 400));
};
new ResizeObserver(fit).observe($("root"));
// Again once the page and fonts have settled; some screens report their size late.
window.addEventListener("load", () => [100, 400, 1200].forEach((t) => setTimeout(fit, t)));
const setTitle = (t: string) => { $("w-title").textContent = t; document.title = t; win?.setTitle(t); };

$("w-min").onclick = () => win?.minimize();
$("w-close").onclick = () => (win ? win.close() : window.close());
document.addEventListener("keydown", (e) => { if (e.key === "Escape") $("w-close").click(); });
document.addEventListener("contextmenu", (e) => { if (!(e.target as HTMLElement & HTMLInputElement).closest<HTMLElement>("input")) e.preventDefault(); });

const params = new URLSearchParams(location.search);

// Adds the download, or shows why it couldn't be added above the buttons
// (a failed click must never just do nothing). Returns its id, or null.
async function addDownload(req: AddRequest): Promise<number | null> {
  view.querySelector<HTMLElement>(".add-err")?.remove();
  try {
    return await invoke("add_download", { req });
  } catch (e) {
    view.querySelector<HTMLElement>(".d-foot")?.insertAdjacentHTML("beforebegin", `<p class="err add-err">${esc(t("Couldn't add the download: {e}", { e: String(e) }))}</p>`);
    return null;
  }
}

// ---------- New download (caught from the browser) ----------
async function confirmView(token: number) {
  setTitle(t("New download"));
  const inc = await invoke("confirm_info", { token });
  if (!inc) { view.innerHTML = `<p class="muted">${t("This download is no longer available.")}</p>`; return; }
  const { settings } = await invoke("get_state");
  const headers: Header[] = [];
  if (inc.cookies) headers.push(["Cookie", inc.cookies]);
  if (inc.referrer) headers.push(["Referer", inc.referrer]);
  if (inc.userAgent) headers.push(["User-Agent", inc.userAgent]);
  let dir = settings.downloadDir, dirTouched = false, info: ProbeInfo | null = null;
  if (await invoke("is_media_url", { url: inc.url })) return mediaView(token, inc, settings, headers);
  if (await invoke("is_torrent_url", { url: inc.url, filename: inc.filename || null })) return torrentView(token, inc, settings, headers);

  const dirFor = (cat: string) => (settings.sortIntoCategories && cat !== "Other" ? `${settings.downloadDir}\\${cat}` : settings.downloadDir);
  view.innerHTML = `
    <div class="d-head"><span id="c-tile">${glyph("Other")}</span><div class="d-name"><input id="c-name" spellcheck="false" placeholder="${esc(t("Getting file details…"))}"><div class="d-sub"><span class="st">${esc(host(inc.url))}</span></div></div></div>
    <div class="kv">
      <span class="k">${t("Address")}</span><span class="v"><span title="${esc(inc.url)}">${esc(inc.url)}</span></span>
      <span class="k">${t("Size")}</span><span class="v" id="c-size"><span class="spinner"></span><span class="muted">${t("Checking…")}</span></span>
      <span class="k">${t("Category")}</span><span class="v"><select id="c-cat">${Object.keys(CATS).map((c) => `<option value="${c}">${esc(tt(c))}</option>`).join("")}</select></span>
      <span class="k">${t("Save to")}</span><span class="v"><span class="path" id="c-dir" title=""></span><button class="btn sm" id="c-browse">${t("Change")}</button></span>
      <span class="k">${t("SHA-256")}</span><span class="v"><input id="c-sum" spellcheck="false" placeholder="${esc(t("Optional: paste it to check the file"))}"></span>
    </div>
    <div class="d-foot">
      <button class="btn" id="c-later">${t("Download later")}</button>
      <span class="grow"></span>
      <button class="btn" id="c-cancel">${t("Cancel")}</button>
      <button class="btn primary" id="c-start" disabled>${t("Start download")}</button>
    </div>`;
  const setDir = (d: string) => { dir = d; $("c-dir").textContent = d; $("c-dir").title = d; };
  setDir(dir);
  $("c-start").focus();

  try {
    info = await invoke("probe_url", { url: inc.url, headers });
    const suggested = /\.[a-z0-9]{2,5}$/i.test(inc.filename || "") ? inc.filename!.split(/[\\/]/).pop() : "";
    $("c-name").value = suggested || info.filename;
    $("c-cat").value = info.category;
    $("c-tile").innerHTML = glyph(info.category, $("c-name").value);
    setDir(info.dir);
    $("c-size").innerHTML = `<span>${info.size != null ? bytes(info.size) : t("Unknown size")}</span><span class="muted">${info.resumable ? t("Resumable · {n} connections", { n: settings.connections }) : t("Can't resume · 1 connection")}</span>`;
    $("c-start").disabled = false;
  } catch (e) {
    $("c-name").value = inc.filename || "download";
    $("c-size").innerHTML = `<span class="err">${esc(t("Couldn't reach the file: {e}", { e: String(e) }))}</span>`;
  }
  styledSelect($("c-cat") as HTMLSelectElement);
  $("c-cat").onchange = () => { $("c-tile").innerHTML = glyph($("c-cat").value, $("c-name").value); if (!dirTouched) setDir(dirFor($("c-cat").value)); };
  $("c-browse").onclick = async () => { const d = await invoke("pick_folder", { start: dir }); if (d) { setDir(d); dirTouched = true; } };
  $("c-sum").addEventListener("input", () => $("c-sum").classList.remove("bad"));
  $("c-cancel").onclick = () => { invoke("forget_confirm", { token }); $("w-close").click(); };
  const add = async (start: boolean) => {
    const filename = $("c-name").value.trim();
    if (!filename) return;
    const sum = $("c-sum").value.trim().toLowerCase();
    if (sum && !/^[a-f0-9]{64}$/.test(sum)) { $("c-sum").classList.add("bad"); $("c-sum").title = t("A SHA-256 is 64 letters and digits"); $("c-sum").focus(); return; }
    const id = await addDownload({ url: inc.url, filename, dir, headers, start, window: win?.label, checksum: sum || null });
    if (id == null) return;
    invoke("forget_confirm", { token });
    if (!start) { $("w-close").click(); return; }
    history.replaceState(null, "", `?id=${id}`);
    progressView(id);
  };
  $("c-later").onclick = () => add(false);
  $("c-start").onclick = () => add(true);
  document.addEventListener("keydown", (e) => { if (e.key === "Enter" && !$("c-start").disabled && !(e.target as HTMLElement & HTMLInputElement).matches("select")) { e.preventDefault(); $("c-start").click(); } });
}

// ---------- New download from a video site (YouTube and others) ----------
// These go through yt-dlp: the page is probed for its qualities, and the
// chosen one becomes the download's format.
const safeName = (s: string) => s.replace(/[<>:"/\\|?*\u0000-\u001f]+/g, " ").replace(/\s+/g, " ").trim().replace(/[. ]+$/, "").slice(0, 150) || "video";
async function mediaView(token: number, inc: Incoming, settings: Settings, headers: Header[]) {
  let dir = settings.downloadDir, dirTouched = false, info: MediaInfo | null = null;
  const dirFor = (cat: string) => (settings.sortIntoCategories && cat !== "Other" ? `${settings.downloadDir}\\${cat}` : settings.downloadDir);
  view.innerHTML = `
    <div class="d-head"><span id="c-tile">${glyph("Video")}</span><div class="d-name"><input id="c-name" spellcheck="false" placeholder="${esc(t("Getting video details…"))}"><div class="d-sub"><span class="st">${esc(host(inc.url))}</span></div></div></div>
    <div class="kv">
      <span class="k">${t("Address")}</span><span class="v"><span title="${esc(inc.url)}">${esc(inc.url)}</span></span>
      <span class="k">${t("Quality")}</span><span class="v" id="c-q"><span class="spinner"></span><span class="muted" id="c-status">${t("Getting video details…")}</span></span>
      <span class="k">${t("Save to")}</span><span class="v"><span class="path" id="c-dir" title=""></span><button class="btn sm" id="c-browse">${t("Change")}</button></span>
    </div>
    <div class="d-foot">
      <button class="btn" id="c-later" disabled>${t("Download later")}</button>
      <span class="grow"></span>
      <button class="btn" id="c-cancel">${t("Cancel")}</button>
      <button class="btn primary" id="c-start" disabled>${t("Start download")}</button>
    </div>`;
  const setDir = (d: string) => { dir = d; $("c-dir").textContent = d; $("c-dir").title = d; };
  setDir(dirFor("Video"));
  // Say what is actually happening, with the seconds so far: fetching the
  // video tools the first time, otherwise waiting on the site.
  const site = host(inc.url).replace(/^www\./, "");
  const started = Date.now();
  const status = setInterval(async () => {
    const el = document.getElementById("c-status");
    if (!el) return;
    const note = await invoke("probe_note").catch(() => null);
    const s = Math.round((Date.now() - started) / 1000);
    const label = tb(note);
    el.textContent = label ? `${label.replace(" · ", "… ")}${label.includes("%") ? "" : "…"}`
      : s < 3 ? t("Getting video details…") : t("Asking {site} about this video… {s} s", { site, s });
  }, 500);
  const quality = (): Quality => info!.qualities[Number($("c-qs").value) || 0];
  const applyQuality = () => {
    const q = quality(), cat = q.ext === "mp4" || q.ext === "webm" ? "Video" : "Music";
    $("c-name").value = $("c-name").value.replace(/\.[a-z0-9]{2,4}$/i, "") + `.${q.ext}`;
    $("c-tile").innerHTML = glyph(cat, $("c-name").value);
    if (!dirTouched) setDir(dirFor(cat));
  };
  try {
    // The extension may have read the qualities from the page already: then
    // there is nothing to wait for.
    const known = inc.qualities?.length ? inc.qualities : null;
    info = known
      ? { title: (inc.filename || "video").replace(/\.[a-z0-9]{2,4}$/i, ""), site: "", duration: null, qualities: known }
      : await invoke("probe_media", { url: inc.url, headers });
    // A stream's own title is just its playlist's name ("index", "master"),
    // so use the page's title that came with it from the browser.
    const stream = /\.(m3u8|mpd)(?:$|[?#])/i.test(inc.url);
    const title = stream && inc.filename ? inc.filename.replace(/\.[a-z0-9]{2,4}$/i, "") : info.title;
    $("c-name").value = `${safeName(title)}.${info.qualities[0].ext}`;
    $("c-q").innerHTML = `<select id="c-qs">${info.qualities.map((q, i) => `<option value="${i}">${esc(q.label)}${q.size ? ` · ${bytes(q.size)}` : ""}</option>`).join("")}</select>`;
    // Default to the best quality up to 1080p: 4K files are huge, and
    // they're one click away.
    const preferred = known && inc.pick != null ? inc.pick : info.qualities.findIndex((q) => /^\d+p/.test(q.label) && parseInt(q.label) <= 1080 && !/HDR/.test(q.label));
    $("c-qs").value = String(Math.max(0, preferred));
    $("c-qs").onchange = applyQuality;
    styledSelect($("c-qs") as HTMLSelectElement);
    applyQuality();
    $("c-start").disabled = $("c-later").disabled = false;
    $("c-start").focus();
  } catch (e) {
    $("c-name").value = "";
    $("c-q").innerHTML = `<span class="err">${esc(tb(String(e)))}</span>`;
  } finally {
    clearInterval(status);
  }
  $("c-browse").onclick = async () => { const d = await invoke("pick_folder", { start: dir }); if (d) { setDir(d); dirTouched = true; } };
  $("c-cancel").onclick = () => { invoke("forget_confirm", { token }); $("w-close").click(); };
  const add = async (start: boolean) => {
    const filename = safeName($("c-name").value);
    if (!filename || !info) return;
    const q = quality();
    const id = await addDownload({ url: inc.url, filename, dir, headers, start, window: win?.label, format: q.format, size: q.size ?? null });
    if (id == null) return;
    invoke("forget_confirm", { token });
    if (!start) { $("w-close").click(); return; }
    history.replaceState(null, "", `?id=${id}`);
    progressView(id);
  };
  $("c-later").onclick = () => add(false);
  $("c-start").onclick = () => add(true);
  document.addEventListener("keydown", (e) => { if (e.key === "Enter" && !$("c-start").disabled && !(e.target as HTMLElement & HTMLInputElement).matches("select")) { e.preventDefault(); $("c-start").click(); } });
}

// ---------- New download from a torrent or magnet link ----------
// The torrent is read for its files; the user picks which to download.
async function torrentView(token: number, inc: Incoming, settings: Settings, headers: Header[]) {
  let dir = settings.downloadDir, info: TorrentInfo | null = null;
  const magnet = inc.url.startsWith("magnet:");
  view.innerHTML = `
    <div class="d-head">${glyph("Compressed")}<div class="d-name"><input id="c-name" spellcheck="false" placeholder="${esc(magnet ? t("Asking peers for the details…") : t("Reading the torrent…"))}" readonly><div class="d-sub"><span class="st">${magnet ? t("Magnet link") : t("Torrent")}</span></div></div></div>
    <div class="t-files" id="t-files"><div class="t-wait"><span class="spinner"></span><span class="muted">${magnet ? t("Finding peers who share it. This can take a few seconds.") : t("Reading the file list…")}</span></div></div>
    <div class="kv">
      <span class="k">${t("Selected")}</span><span class="v" id="t-sel">—</span>
      <span class="k">${t("Save to")}</span><span class="v"><span class="path" id="c-dir" title=""></span><button class="btn sm" id="c-browse">${t("Change")}</button></span>
    </div>
    <div class="d-foot">
      <button class="btn" id="c-later" disabled>${t("Download later")}</button>
      <span class="grow"></span>
      <button class="btn" id="c-cancel">${t("Cancel")}</button>
      <button class="btn primary" id="c-start" disabled>${t("Start download")}</button>
    </div>`;
  const setDir = (d: string) => { dir = d; $("c-dir").textContent = d; $("c-dir").title = d; };
  setDir(dir);
  const chosen = (): number[] => [...view.querySelectorAll<HTMLInputElement>(".t-file input[data-i]:checked")].map((c) => Number(c.dataset.i));
  const update = () => {
    const sel = chosen(), size = sel.reduce((a, i) => a + info!.files[i].size, 0);
    $("t-sel").textContent = t("{n} of {m} files · {size}", { n: sel.length, m: info!.files.length, size: bytes(size) });
    $("c-start").disabled = $("c-later").disabled = sel.length === 0;
    const all = $("t-all"); if (all) all.checked = sel.length === info!.files.length;
  };
  try {
    info = await invoke("probe_torrent", { url: inc.url, headers });
    $("c-name").value = info.name;
    const many = info.files.length > 1;
    $("t-files").innerHTML = (many ? `<label class="t-file t-allrow"><input type="checkbox" id="t-all" checked><span>${t("All files")}</span><span class="muted">${bytes(info.size)}</span></label>` : "") +
      info.files.map((f, i) => `<label class="t-file"><input type="checkbox" data-i="${i}" checked><span title="${esc(f.path)}">${esc(f.path)}</span><span class="muted">${bytes(f.size)}</span></label>`).join("");
    $("t-files").addEventListener("change", (e) => {
      if ((e.target as HTMLElement & HTMLInputElement).id === "t-all") view.querySelectorAll<HTMLInputElement>(".t-file input[data-i]").forEach((c) => (c.checked = (e.target as HTMLElement & HTMLInputElement).checked));
      update();
    });
    update();
    $("c-start").focus();
  } catch (e) {
    $("t-files").innerHTML = `<div class="t-wait"><span class="err">${esc(tb(String(e)))}</span></div>`;
  }
  $("c-browse").onclick = async () => { const d = await invoke("pick_folder", { start: dir }); if (d) setDir(d); };
  $("c-cancel").onclick = () => { invoke("forget_confirm", { token }); $("w-close").click(); };
  const add = async (start: boolean) => {
    if (!info) return;
    const sel = chosen();
    if (!sel.length) return;
    const files = sel.length === info.files.length ? [] : sel;
    const size = sel.reduce((a, i) => a + info!.files[i].size, 0);
    const id = await addDownload({ url: inc.url, filename: info.name, dir, headers, start, window: win?.label, size, torrentFiles: files });
    if (id == null) return;
    invoke("forget_confirm", { token });
    if (!start) { $("w-close").click(); return; }
    history.replaceState(null, "", `?id=${id}`);
    progressView(id);
  };
  $("c-later").onclick = () => add(false);
  $("c-start").onclick = () => add(true);
}

// ---------- progress / complete ----------
let current: Item | null = null, details = false, armedTimer = 0, unlisten: (() => void) | null = null;
async function progressView(id: number) {
  current = await invoke("get_item", { id });
  render();
  unlisten?.();
  unlisten = await listen("downloads", (e) => {
    const it = e.payload.find((i) => i.id === id);
    if (!it) { $("w-close").click(); return; }
    current = it;
    render();
  });
}

let layout = "";
function render() {
  const it = current;
  if (!it) return;
  const cat = it.category || catOf(it.filename);
  const pct = it.size ? Math.min(100, (it.downloaded / it.size) * 100) : it.status === "completed" ? 100 : 0;
  const eta = it.speed > 0 && it.size ? (it.size - it.downloaded) / it.speed : Infinity;

  // One card for every state; only the status line, bar colour, stats and
  // buttons change, so finishing a download feels like the same window.
  const state = it.status === "completed" ? "done" : it.status === "error" ? "error" : it.status === "downloading" ? "run" : "paused";
  const paused = state === "paused";
  const torrent = Array.isArray(it.torrentFiles);
  const media = !!it.format || torrent;
  const showDetails = details && !media && (state === "run" || state === "paused");
  const key = `${state}:${showDetails}`;
  if (layout !== key) {
    layout = key;
    const buttons = {
      run: `<button class="btn" data-a="cancel">${t("Cancel")}</button><button class="btn" data-a="pause">${t("Pause")}</button>`,
      paused: `<button class="btn" data-a="cancel">${t("Cancel")}</button><button class="btn primary" data-a="resume">${t("Resume")}</button>`,
      done: `<button class="btn" data-a="folder">${t("Show in folder")}</button><button class="btn" data-a="close">${t("Close")}</button><button class="btn primary" data-a="open">${t("Open")}</button>`,
      error: `<button class="btn" data-a="close">${t("Close")}</button><button class="btn primary" data-a="resume">${t("Retry")}</button>`,
    }[state];
    view.innerHTML = `
      <div class="d-head">${glyph(cat, it.filename)}<div class="d-name"><b title="${esc(it.filename)}">${esc(it.filename)}</b><div class="d-sub" id="p-sub"></div></div></div>
      <div class="p-top"><span id="p-left"></span><span class="p-pct" id="p-pct"></span></div>
      <div class="segbar" id="p-bar"></div>
      <div class="p-meta" id="p-right"></div>
      <div class="kv" id="p-kv"></div>
      ${showDetails ? `<div class="conn"><div class="cscroll"><table class="ctable"><thead><tr><th>${t("Part")}</th><th>${t("Size")}</th><th>${t("Received")}</th><th>${t("Status")}</th></tr></thead><tbody id="p-rows"></tbody></table></div></div>` : ""}
      <div class="d-foot">
        ${!media && (state === "run" || state === "paused") ? `<button class="ghost-btn" data-a="details">${icon(showDetails ? "chevUp" : "chevDown")}${showDetails ? t("Hide details") : t("Show details")}</button>` : ""}
        <span class="grow"></span>${buttons}
      </div>`;
  }

  const dir = it.path.replace(/[\\/][^\\/]*$/, "");
  // Every byte is in but the file is still being put together or checked.
  const finishing = state === "run" && !!it.size && it.downloaded >= it.size;
  const status = {
    run: [tb(it.note) || (finishing ? t("Finishing…") : media ? (it.downloaded ? t("Downloading") : t("Starting…")) : it.connections ? t("Downloading") : t("Connecting…")), "var(--accent)"],
    paused: [it.status === "queued" ? t("Waiting in queue") : t("Paused"), "var(--amber)"],
    done: [t("Completed"), "var(--green)"],
    error: [tb(it.error) || t("Failed"), "var(--red)"],
  }[state];
  setTitle(state === "done" ? t("Download complete") : state === "error" ? t("Download failed") : `${Math.floor(pct)}% · ${it.filename}`);
  $("p-sub").innerHTML = `<span class="st ${state === "error" ? "err" : ""}" title="${esc(status[0])}">${esc(status[0])}</span><span class="from">${t("from {host}", { host: esc(host(it.url)) })}</span>`;
  $("p-pct").textContent = it.size || state === "done" ? `${Math.floor(pct)}%` : "";
  // Still preparing (fetching the video tools): no numbers to show yet.
  const preparing = state === "run" && !!it.note && !it.downloaded;
  $("p-left").textContent = preparing ? "" : state === "done" ? bytes(it.size) : it.size ? t("{a} of {b}", { a: bytes(it.downloaded), b: bytes(it.size) }) : bytes(it.downloaded);
  const conns = it.connections ? tn("{n} connection", "{n} connections", it.connections) : "";
  $("p-right").textContent = state === "run" && !finishing && !preparing ? [speed(it.speed), t("{x} left", { x: dur(eta) }), conns].filter(Boolean).join("   ·   ")
    : state === "done" ? t("Finished {when}", { when: whenFull(it.completedAt) }) : "";
  const bar = $("p-bar");
  bar.className = `segbar ${state} ${!it.size && state === "run" ? "indet" : ""}`;
  segBar(bar, null, undefined, pct);
  const row = (k: string, v: string, cls = "") => `<span class="k">${k}</span><span class="v"><span class="${cls}" title="${esc(v)}">${esc(v)}</span></span>`;
  const kv = row(t("Location"), dir, "path") +
    (state === "done" ? "" : torrent ? row(t("Files"), it.torrentFiles!.length ? t("{n} selected", { n: it.torrentFiles!.length }) : t("All")) : media ? row(t("Quality"), qualityLabel(it.format)) : row(t("Resume"), it.resumable ? t("Supported") : t("Not supported by this server"))) +
    row(t("Address"), it.url);
  if ($("p-kv").innerHTML !== kv) $("p-kv").innerHTML = kv;

  if (showDetails) {
    const segs = (it.segments || []).filter(([s, , e]) => e > s);
    $("p-rows").innerHTML = segs.map(([s, p, e], n) => {
      const done = p >= e;
      return `<tr class="${done ? "done" : ""}"><td>${n + 1}</td><td>${bytes(e - s)}</td><td>${Math.floor(((p - s) / Math.max(1, e - s)) * 100)}%</td><td>${done ? t("Done") : paused ? t("Paused") : t("Receiving")}</td></tr>`;
    }).join("") || `<tr><td colspan="4" class="muted">${t("Parts appear once the download starts.")}</td></tr>`;
  }
}

function qualityLabel(format: string | null) {
  const h = /height=(\d+)/.exec(format || "");
  return h ? `${h[1]}p` : /^ba/.test(format || "") ? t("Audio only") : /^hls/.test(format || "") ? t("Chosen in the browser") : t("Best available");
}

view.addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement & HTMLInputElement).closest<HTMLElement>("[data-a]");
  if (!b || !current) return;
  const id = current.id;
  switch (b.dataset.a) {
    case "details": details = !details; render(); break;
    case "pause": invoke("pause", { ids: [id] }); break;
    case "resume": layout = ""; invoke("resume", { ids: [id] }); break;
    case "open": invoke("open_file", { id }); $("w-close").click(); break;
    case "folder": invoke("open_folder", { id }); break;
    case "close": $("w-close").click(); break;
    case "cancel":
      // Ask twice instead of a second dialog: the button arms for 3 s.
      if (!b.classList.contains("armed")) {
        b.classList.add("armed"); b.textContent = t("Cancel download?");
        clearTimeout(armedTimer);
        armedTimer = window.setTimeout(() => { b.classList.remove("armed"); b.textContent = t("Cancel"); }, 3000);
      } else {
        await invoke("remove", { ids: [id], deleteFiles: false });
        $("w-close").click();
      }
      break;
  }
});

// Theme and language: the saved choices, and changes made in Settings while
// the window is open. The language is known before the first paint of content.
function applyLanguage(setting: string | null | undefined) {
  setLang(resolveLang(setting));
  void invoke("set_ui_language", { language: locale() });
}

async function bootWindow() {
  try {
    const s = await invoke("get_state");
    window.setTheme(s.settings.theme);
    applyLanguage(s.settings.language);
  } catch {}
  if (params.has("confirm")) confirmView(Number(params.get("confirm")));
  else if (params.has("id")) progressView(Number(params.get("id")));
}
listen("settings", (e) => {
  window.setTheme(e.payload.theme);
  applyLanguage(e.payload.language);
  if (current) render();
});
bootWindow();

// The system dropdown is a plain white list on Windows. This keeps the
// <select> as the value holder but shows the choices in the app's own menu.
const nativeValue = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")!;
function styledSelect(sel: HTMLSelectElement) {
  if (sel.dataset.styled) return;
  sel.dataset.styled = "1";
  sel.hidden = true;
  const btn = document.createElement("button");
  btn.type = "button";
  btn.className = "dd";
  btn.id = `${sel.id}-btn`;
  sel.after(btn);
  const label = () => { btn.innerHTML = `<span>${esc(sel.selectedOptions[0]?.text || "")}</span>${icon("chevron")}`; };
  // Keep the button in step when code sets the value.
  Object.defineProperty(sel, "value", {
    get: () => nativeValue.get!.call(sel),
    set: (v) => { nativeValue.set!.call(sel, v); label(); },
  });
  label();
  btn.onclick = (e) => {
    e.stopPropagation();
    document.querySelector(".dd-menu")?.remove();
    const menu = document.createElement("div");
    menu.className = "menu dd-menu";
    menu.innerHTML = [...sel.options].map((o, i) =>
      `<button type="button" data-i="${i}" class="${i === sel.selectedIndex ? "on" : ""}">${esc(o.text)}</button>`).join("");
    document.body.append(menu);
    const r = btn.getBoundingClientRect();
    menu.style.minWidth = `${r.width}px`;
    menu.style.left = `${Math.min(r.left, innerWidth - menu.offsetWidth - 8)}px`;
    // Open downward if it fits, else upward; scroll if neither does.
    const below = innerHeight - r.bottom - 8, above = r.top - 8;
    const down = menu.offsetHeight <= below || below >= above;
    menu.style.maxHeight = `${down ? below : above}px`;
    menu.style.top = down ? `${r.bottom + 4}px` : `${r.top - 4 - Math.min(menu.offsetHeight, above)}px`;
    (menu.querySelector(".on") as HTMLElement | null)?.scrollIntoView({ block: "nearest" });
    const close = () => { menu.remove(); document.removeEventListener("click", close); };
    menu.onclick = (ev) => {
      const b = (ev.target as HTMLElement).closest<HTMLElement>("[data-i]");
      if (!b) return;
      sel.selectedIndex = Number(b.dataset.i);
      label();
      sel.dispatchEvent(new Event("change"));
      close();
    };
    setTimeout(() => document.addEventListener("click", close));
  };
}
