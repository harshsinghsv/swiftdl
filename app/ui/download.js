// The per-download window, like IDM's:
//   download.html?confirm=<token>  "New download": a file caught from the
//                                  browser, waiting for Start / Later / Cancel
//   download.html?id=<id>          live progress, then "Download complete"

const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const win = window.__TAURI__.window?.getCurrentWindow?.();
const LogicalSize = window.__TAURI__.dpi?.LogicalSize;

// ---------- helpers ----------
const P = {
  check: '<path d="m5.5 12.5 4 4 9-9"/>',
  alert: '<circle cx="12" cy="12" r="8.5"/><path d="M12 8v5M12 16h.01"/>',
  pause: '<path d="M9 6v12M15 6v12"/>',
  play: '<path d="M8 5.5v13l10.5-6.5z"/>',
  folder: '<path d="M3.5 7.5A2 2 0 0 1 5.5 5.5H9l2 2h7.5a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2z"/>',
  open: '<path d="M14 4.5h5.5V10M19.5 4.5 11 13M18 14v4a1.5 1.5 0 0 1-1.5 1.5h-10A1.5 1.5 0 0 1 5 18V8a1.5 1.5 0 0 1 1.5-1.5h4"/>',
  retry: '<path d="M4.5 12a7.5 7.5 0 0 1 13.2-4.8L19.5 9M19.5 4.5V9H15M19.5 12a7.5 7.5 0 0 1-13.2 4.8L4.5 15M4.5 19.5V15H9"/>',
  download: '<path d="M12 4v11M7.5 10.5 12 15l4.5-4.5M5 20h14"/>',
  chevDown: '<path d="m6 9.5 6 6 6-6"/>',
  chevUp: '<path d="m6 14.5 6-6 6 6"/>',
  video: '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M7.5 4.5v15M16.5 4.5v15M3.5 9.5h4M3.5 14.5h4M16.5 9.5h4M16.5 14.5h4"/>',
  music: '<path d="M9 17.5V6.3l10-1.8v11"/><circle cx="6.5" cy="17.5" r="2.5"/><circle cx="16.5" cy="15.5" r="2.5"/>',
  app: '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><path d="M3.5 9h17"/><path d="M6.6 6.8h.01M9 6.8h.01" stroke-width="2.2"/>',
  archive: '<path d="M12 3.3 20 7.4v9.2l-8 4.1-8-4.1V7.4z"/><path d="m4 7.4 8 4.1 8-4.1M12 11.5v9.2M8 5.3l8 4.1"/>',
  doc: '<path d="M14 3.5H7.5a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2V8z"/><path d="M14 3.5V8h4.5M9 12.5h6M9 16h6"/>',
  image: '<rect x="3.5" y="4.5" width="17" height="15" rx="2.5"/><circle cx="9" cy="10" r="1.7"/><path d="m20.5 15.5-4.5-4.5-9.5 8.5"/>',
  file: '<path d="M14 3.5H7.5a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2h9a2 2 0 0 0 2-2V8z"/><path d="M14 3.5V8h4.5"/>',
};
const icon = (n) => `<svg class="ic" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">${P[n] || P.file}</svg>`;
const CATS = {
  Video: { color: "#8b5cf6", icon: "video" }, Music: { color: "#ec4899", icon: "music" }, Programs: { color: "#3b82f6", icon: "app" },
  Compressed: { color: "#f59e0b", icon: "archive" }, Documents: { color: "#10b981", icon: "doc" }, Images: { color: "#f97316", icon: "image" },
  Other: { color: "#8b8f98", icon: "file" },
};
const catOf = (name) => {
  const e = (/\.([a-z0-9]{1,5})$/i.exec(name || "") || [])[1]?.toLowerCase() || "";
  if (/^(zip|rar|7z|tar|gz|tgz|xz|bz2|zst|iso|img)$/.test(e)) return "Compressed";
  if (/^(pdf|docx?|xlsx?|pptx?|odt|txt|rtf|epub|csv|md)$/.test(e)) return "Documents";
  if (/^(mp3|wav|flac|aac|ogg|m4a|opus|wma)$/.test(e)) return "Music";
  if (/^(mp4|mkv|avi|mov|webm|flv|wmv|m4v|mpe?g|3gp|ts)$/.test(e)) return "Video";
  if (/^(exe|msi|dmg|pkg|deb|rpm|apk|appimage|msix|bat|jar)$/.test(e)) return "Programs";
  if (/^(jpe?g|png|gif|webp|svg|bmp|heic|avif)$/.test(e)) return "Images";
  return "Other";
};
const ftile = (cat) => `<div class="ftile" style="--c:${(CATS[cat] || CATS.Other).color}">${icon((CATS[cat] || CATS.Other).icon)}</div>`;
const UNITS = ["B", "KB", "MB", "GB", "TB"];
function bytes(n) {
  if (n == null || !isFinite(n)) return "—";
  let i = 0;
  while (n >= 1024 && i < UNITS.length - 1) { n /= 1024; i++; }
  return `${i === 0 ? Math.round(n) : n < 10 ? n.toFixed(2) : n < 100 ? n.toFixed(1) : Math.round(n)} ${UNITS[i]}`;
}
const speed = (b) => (b > 0 ? `${bytes(b)}/s` : "—");
function dur(sec) {
  if (!isFinite(sec) || sec <= 0) return "—";
  sec = Math.ceil(sec);
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60), s = sec % 60;
  return h ? `${h}h ${m}m` : m ? `${m}m ${s}s` : `${s}s`;
}
function when(ms) {
  if (!ms) return "";
  const mins = Math.round((Date.now() - ms) / 60000);
  if (mins < 1) return "Just now";
  if (mins < 60) return `${mins}m ago`;
  return new Date(ms).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
}
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const host = (url) => { try { return new URL(url).hostname.replace(/^www\./, ""); } catch { return ""; } };
const $ = (id) => document.getElementById(id);
const view = $("view");

// The window grows and shrinks to fit what it shows.
const fit = () => {
  if (!win || !LogicalSize) return;
  const h = Math.ceil($("root").getBoundingClientRect().height);
  win.setSize(new LogicalSize(560, Math.min(760, h)));
};
new ResizeObserver(fit).observe($("root"));
const setTitle = (t) => { $("w-title").textContent = t; document.title = t; win?.setTitle(t); };

$("w-min").onclick = () => win?.minimize();
$("w-close").onclick = () => (win ? win.close() : window.close());
document.addEventListener("keydown", (e) => { if (e.key === "Escape") $("w-close").click(); });
document.addEventListener("contextmenu", (e) => { if (!e.target.closest("input")) e.preventDefault(); });

const params = new URLSearchParams(location.search);

// ---------- New download (caught from the browser) ----------
async function confirmView(token) {
  setTitle("New download");
  const inc = await invoke("confirm_info", { token });
  if (!inc) { view.innerHTML = `<p class="muted">This download is no longer available.</p>`; return; }
  const { settings } = await invoke("get_state");
  const headers = [];
  if (inc.cookies) headers.push(["Cookie", inc.cookies]);
  if (inc.referrer) headers.push(["Referer", inc.referrer]);
  if (inc.userAgent) headers.push(["User-Agent", inc.userAgent]);
  let dir = settings.downloadDir, dirTouched = false, info = null;

  const dirFor = (cat) => (settings.sortIntoCategories && cat !== "Other" ? `${settings.downloadDir}\\${cat}` : settings.downloadDir);
  view.innerHTML = `
    <div class="d-head"><span id="c-tile">${ftile("Other")}</span><div class="d-name"><input id="c-name" spellcheck="false" placeholder="Getting file details…"><div class="d-sub" id="c-sub">${esc(host(inc.url))}</div></div></div>
    <div class="form">
      <span class="k">Address</span><span class="v"><span title="${esc(inc.url)}">${esc(inc.url)}</span></span>
      <span class="k">Category</span><span class="v"><select id="c-cat">${Object.keys(CATS).map((c) => `<option>${c}</option>`).join("")}</select></span>
      <span class="k">Save to</span><span class="v"><span class="path" id="c-dir" title=""></span><button class="btn sm" id="c-browse">Change…</button></span>
      <span class="k">Size</span><span class="v" id="c-size"><span class="spinner"></span><span class="muted">Checking…</span></span>
    </div>
    <div class="d-foot">
      <button class="btn" id="c-later">Download later</button>
      <span class="grow"></span>
      <button class="btn" id="c-cancel">Cancel</button>
      <button class="btn primary" id="c-start" disabled>${icon("download")}Start download</button>
    </div>`;
  const setDir = (d) => { dir = d; $("c-dir").textContent = d; $("c-dir").title = d; };
  setDir(dir);
  $("c-start").focus();

  try {
    info = await invoke("probe_url", { url: inc.url, headers });
    const suggested = /\.[a-z0-9]{2,5}$/i.test(inc.filename || "") ? inc.filename.split(/[\\/]/).pop() : "";
    $("c-name").value = suggested || info.filename;
    $("c-cat").value = info.category;
    $("c-tile").innerHTML = ftile(info.category);
    setDir(info.dir);
    $("c-size").innerHTML = `<span class="size-big">${info.size != null ? bytes(info.size) : "Unknown size"}</span><span class="muted">·</span>` +
      (info.resumable ? `<span style="color:var(--green)">Resumable · ${settings.connections} connections</span>` : `<span style="color:var(--amber)">Can't resume · 1 connection</span>`);
    $("c-start").disabled = false;
  } catch (e) {
    $("c-name").value = inc.filename || "download";
    $("c-size").innerHTML = `<span style="color:var(--red)">Couldn't reach the file: ${esc(String(e))}</span>`;
  }
  $("c-cat").onchange = () => { $("c-tile").innerHTML = ftile($("c-cat").value); if (!dirTouched) setDir(dirFor($("c-cat").value)); };
  $("c-browse").onclick = async () => { const d = await invoke("pick_folder", { start: dir }); if (d) { setDir(d); dirTouched = true; } };
  $("c-cancel").onclick = () => { invoke("forget_confirm", { token }); $("w-close").click(); };
  const add = async (start) => {
    const filename = $("c-name").value.trim();
    if (!filename) return;
    const id = await invoke("add_download", { req: { url: inc.url, filename, dir, headers, start, window: win?.label } });
    invoke("forget_confirm", { token });
    if (!start) { $("w-close").click(); return; }
    history.replaceState(null, "", `?id=${id}`);
    progressView(id);
  };
  $("c-later").onclick = () => add(false);
  $("c-start").onclick = () => add(true);
  document.addEventListener("keydown", (e) => { if (e.key === "Enter" && !$("c-start").disabled && !e.target.matches("select")) { e.preventDefault(); $("c-start").click(); } });
}

// ---------- progress / complete ----------
let current = null, details = false, armedTimer = 0, unlisten = null;
async function progressView(id) {
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
  const showDetails = details && (state === "run" || state === "paused");
  const key = `${state}:${showDetails}`;
  if (layout !== key) {
    layout = key;
    const buttons = {
      run: `<button class="btn" data-a="pause">${icon("pause")}Pause</button><button class="btn" data-a="cancel">Cancel</button>`,
      paused: `<button class="btn" data-a="cancel">Cancel</button><button class="btn primary" data-a="resume">${icon("play")}Resume</button>`,
      done: `<button class="btn" data-a="close">Close</button><button class="btn" data-a="folder">${icon("folder")}Show in folder</button><button class="btn primary" data-a="open">${icon("open")}Open</button>`,
      error: `<button class="btn" data-a="close">Close</button><button class="btn primary" data-a="resume">${icon("retry")}Retry</button>`,
    }[state];
    view.innerHTML = `
      <div class="d-head">${ftile(cat)}<div class="d-name"><b title="${esc(it.filename)}">${esc(it.filename)}</b><div class="d-sub" id="p-sub"></div></div></div>
      <div class="p-top"><div class="p-pct" id="p-pct"></div><div class="p-speed" id="p-speed"></div></div>
      <div class="p-bar" id="p-bar"><i></i></div>
      <div class="p-stats" id="p-stats"></div>
      ${showDetails ? `<div class="conn"><div class="conn-head">Connections<span id="p-conn-n"></span></div><div class="lanes" id="p-lanes"></div>
        <div class="cscroll"><table class="ctable"><thead><tr><th>#</th><th>Downloaded</th><th>Progress</th><th>Status</th></tr></thead><tbody id="p-rows"></tbody></table></div></div>` : ""}
      <div class="d-foot">
        ${state === "run" || state === "paused" ? `<button class="ghost-btn" data-a="details">${icon(showDetails ? "chevUp" : "chevDown")}${showDetails ? "Hide details" : "Show details"}</button>` : ""}
        <span class="grow"></span>${buttons}
      </div>`;
  }

  const dirName = it.path.split(/[\\/]/).slice(-2, -1)[0] || "—";
  const status = {
    run: [it.connections ? `Receiving data · ${it.connections} connection${it.connections === 1 ? "" : "s"}` : "Connecting…", "var(--accent)"],
    paused: [it.status === "queued" ? "Waiting in queue" : "Paused", "var(--amber)"],
    done: ["Completed", "var(--green)"],
    error: [it.error || "Failed", "var(--red)"],
  }[state];
  setTitle(state === "done" ? `Download complete · ${it.filename}` : state === "error" ? `Download failed · ${it.filename}` : `${Math.floor(pct)}% · ${it.filename}`);
  $("p-sub").innerHTML = `<span class="st" style="color:${status[1]}" title="${esc(status[0])}">${esc(status[0])}</span> · ${esc(host(it.url))}`;
  $("p-pct").innerHTML = it.size || state === "done" ? `${Math.floor(pct)}<small>%</small>` : bytes(it.downloaded);
  $("p-speed").innerHTML = state === "run" ? `<b>${speed(it.speed)}</b>${dur(eta)} left`
    : "";
  const bar = $("p-bar");
  bar.className = `p-bar ${state} ${!it.size && state === "run" ? "indet" : ""}`;
  bar.firstElementChild.style.width = `${pct}%`;
  const cell = (k, v) => `<div><span>${k}</span><b title="${esc(String(v).replace(/<[^>]+>/g, ""))}">${v}</b></div>`;
  const stats = state === "done"
    ? cell("File size", bytes(it.size)) + cell("Saved to", esc(dirName)) + cell("Finished", when(it.completedAt) || "—")
    : cell("File size", bytes(it.size)) + cell("Downloaded", bytes(it.downloaded)) + cell("Transfer rate", state === "run" ? speed(it.speed) : "—") +
      cell("Time left", state === "run" ? dur(eta) : "—") + cell("Resume", it.resumable ? "Supported" : "No") + cell("Saved to", esc(dirName));
  if ($("p-stats").innerHTML !== stats) $("p-stats").innerHTML = stats;

  if (showDetails) {
    const size = it.size || 1;
    const segs = (it.segments || []).filter(([s, , e]) => e > s);
    $("p-conn-n").textContent = segs.length ? `${segs.filter(([, p, e]) => p < e).length} active · ${segs.length} parts` : "";
    // Start positions and progress by connection, IDM style: each part of
    // the file is a slot, separated by a small gap, filled as it arrives.
    const lanes = $("p-lanes");
    lanes.className = `lanes ${paused ? "paused" : ""}`;
    const html = segs.map(([s, p, e]) =>
      `<span class="slot" style="left:calc(${(s / size) * 100}% + 1px);width:calc(${((e - s) / size) * 100}% - 2px)"><i style="width:${((p - s) / Math.max(1, e - s)) * 100}%"></i></span>`).join("");
    if (lanes.children.length !== segs.length) lanes.innerHTML = html;
    else segs.forEach(([s, p, e], i) => {
      const slot = lanes.children[i];
      slot.style.left = `calc(${(s / size) * 100}% + 1px)`;
      slot.style.width = `calc(${((e - s) / size) * 100}% - 2px)`;
      slot.firstElementChild.style.width = `${((p - s) / Math.max(1, e - s)) * 100}%`;
    });
    $("p-rows").innerHTML = segs.map(([s, p, e], n) => {
      const done = p >= e, frac = (p - s) / Math.max(1, e - s);
      return `<tr><td>${n + 1}</td><td>${bytes(p - s)}</td><td><span class="mini"><i style="width:${frac * 100}%"></i></span></td><td class="${done ? "ok" : paused ? "" : "live"}">${done ? "Done" : paused ? "Paused" : "Receiving data…"}</td></tr>`;
    }).join("") || `<tr><td colspan="4" class="muted">Connection details appear once the download starts.</td></tr>`;
  }
}

view.addEventListener("click", async (e) => {
  const b = e.target.closest("[data-a]");
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
        b.classList.add("armed"); b.textContent = "Cancel download?";
        clearTimeout(armedTimer);
        armedTimer = setTimeout(() => { b.classList.remove("armed"); b.textContent = "Cancel"; }, 3000);
      } else {
        await invoke("remove", { ids: [id], deleteFiles: false });
        $("w-close").click();
      }
      break;
  }
});

if (params.has("confirm")) confirmView(Number(params.get("confirm")));
else if (params.has("id")) progressView(Number(params.get("id")));
