// SwiftDL main window. Talks to the Rust backend through api.ts; opened
// in a normal browser, mock.ts stands in for the backend.

import "./styles.css";
import { invoke, listen, currentWindow, $ } from "./api";
import type { Item, Settings, IntegrationStatus, BrowserStatus, Incoming, ProbeInfo, Header, Status, Commands, UpdateInfo } from "./api";
import { icon, CATS, catOf, parts, bytes, speed, dur, when, esc, host, segBar } from "./util";

const appWindow = currentWindow();
const target = (e: Event) => e.target as HTMLElement;

const hydrate = () => document.querySelectorAll<HTMLElement>("i[data-icon]").forEach((el) => {
  const tpl = document.createElement("template");
  tpl.innerHTML = icon(el.dataset.icon || "file");
  const svg = tpl.content.firstElementChild as SVGElement;
  if (el.className) svg.classList.add(...el.className.split(" "));
  el.replaceWith(svg);
});

const cat = (it: { category: string }) => CATS[it.category] || CATS.Other;
const typeIcon = (name: string) => `<span class="ni-type" style="--c:${(CATS[name] || CATS.Other).color}">${icon((CATS[name] || CATS.Other).icon)}</span>`;
const ftile = (it: { category: string }, lg = false) => `<div class="ftile ${lg ? "lg" : ""}" style="--c:${cat(it).color}">${icon(cat(it).icon)}</div>`;

// Status glyph: a progress ring that fills clockwise while downloading
// (pause bars inside when paused), a dotted ring when queued, and filled
// badges for done and failed.
type StatusLike = { status: Status; size: number | null; downloaded: number };
function statusIcon(it: StatusLike) {
  const s = it.status;
  if (s === "completed") return `<svg class="status" viewBox="0 0 16 16"><circle cx="8" cy="8" r="7.25" fill="var(--green)"/><path d="m5.1 8.2 2 2 3.8-4" fill="none" stroke="#fff" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>`;
  if (s === "error") return `<svg class="status" viewBox="0 0 16 16"><circle cx="8" cy="8" r="7.25" fill="var(--red)"/><path d="M8 4.6v4.2" stroke="#fff" stroke-width="1.7" stroke-linecap="round"/><circle cx="8" cy="11.2" r="1" fill="#fff"/></svg>`;
  if (s === "queued") return `<svg class="status" viewBox="0 0 16 16"><circle cx="8" cy="8" r="6.4" fill="none" stroke="var(--faint)" stroke-width="1.7" stroke-linecap="round" stroke-dasharray="0.1 3.25"/></svg>`;
  const color = s === "paused" ? "var(--amber)" : "var(--accent)";
  const r = 6.4, c = 2 * Math.PI * r, p = Math.max(0.03, Math.min(1, pct(it) / 100));
  const inner = s === "paused"
    ? `<path d="M6.6 6v4M9.4 6v4" stroke="${color}" stroke-width="1.5" stroke-linecap="round"/>`
    : `<path d="M8 5.4v4.6M6.1 8.3 8 10.2l1.9-1.9" fill="none" stroke="${color}" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round"/>`;
  return `<svg class="status" viewBox="0 0 16 16"><circle cx="8" cy="8" r="${r}" fill="none" stroke="var(--track)" stroke-width="1.7"/><circle cx="8" cy="8" r="${r}" fill="none" stroke="${color}" stroke-width="1.7" stroke-linecap="round" stroke-dasharray="${(c * p).toFixed(2)} ${c.toFixed(2)}" transform="rotate(-90 8 8)"/>${inner}</svg>`;
}

const pct = (it: StatusLike) => (it.size ? Math.min(100, (it.downloaded / it.size) * 100) : it.status === "completed" ? 100 : 0);
const eta = (it: Item) => (it.speed > 0 && it.size ? (it.size - it.downloaded) / it.speed : Infinity);
// Place in the waiting line (1 = next).
const queuePos = (it: Item) => state.items.filter((i) => i.status === "queued").findIndex((i) => i.id === it.id) + 1;
const isActive = (it: Item) => it.status === "downloading" || it.status === "queued";
const ext = (name: string) => (/\.([a-z0-9]{1,5})$/i.exec(name || "") || [])[1]?.toUpperCase() || "";
const STATUS: Record<Status, string> = { downloading: "Downloading", queued: "Queued", paused: "Paused", completed: "Completed", error: "Failed" };

// ---------- state ----------
const HISTORY = 120;
const LIMITS = [0, 256, 1024, 5120, 10240];
interface State {
  version: string;
  items: Item[]; settings: Settings | null; view: string; search: string; selected: number | null; pending: Incoming[];
  speedHist: Map<number, number[]>; totalHist: number[]; typesOpen: boolean; integration: IntegrationStatus | null;
}
const state: State = {
  version: "",
  items: [], settings: null, view: "all", search: "", selected: null, pending: [],
  speedHist: new Map(), totalHist: [], typesOpen: true, integration: null,
};
const settings = (): Settings => state.settings as Settings;
interface View { name: string; icon: string; test: (i: Item) => boolean }
const VIEWS: Record<string, View> = {
  all: { name: "All downloads", icon: "inbox", test: () => true },
  active: { name: "In progress", icon: "arrowDown", test: (i) => i.status !== "completed" && i.status !== "error" },
  completed: { name: "Completed", icon: "doneCircle", test: (i) => i.status === "completed" },
  failed: { name: "Failed", icon: "alert", test: (i) => i.status === "error" },
};
const byId = (id: number | null) => state.items.find((i) => i.id === id);
function visible() {
  const v = VIEWS[state.view];
  const q = state.search.trim().toLowerCase();
  return state.items.filter((it) => (v ? v.test(it) : it.category === state.view) &&
    (!q || it.filename.toLowerCase().includes(q) || it.url.toLowerCase().includes(q)));
}
const GROUPS: [Status, string][] = [
  ["downloading", "Downloading"], ["queued", "Queued"], ["paused", "Paused"], ["error", "Failed"], ["completed", "Completed"],
];
function grouped(list: Item[]) {
  return GROUPS.map(([s, title]) => {
    const items = list.filter((i) => i.status === s);
    // Waiting downloads are listed in the order they will start.
    const queue = new Map(state.items.map((i, n) => [i.id, n]));
    items.sort(s === "completed" ? (a, b) => (b.completedAt || 0) - (a.completedAt || 0)
      : s === "queued" ? (a, b) => (queue.get(a.id) || 0) - (queue.get(b.id) || 0)
      : (a, b) => b.addedAt - a.addedAt);
    return { s, title, items };
  }).filter((g) => g.items.length);
}
const orderIds = () => grouped(visible()).flatMap((g) => g.items.map((i) => i.id));

// ---------- charts ----------
function areaChart(canvas: HTMLCanvasElement, data: number[], { grid = false, fill = true } = {}) {
  const dpr = devicePixelRatio || 1, w = canvas.clientWidth, h = canvas.clientHeight;
  if (!w || !h) return;
  if (canvas.width !== Math.round(w * dpr)) { canvas.width = Math.round(w * dpr); canvas.height = Math.round(h * dpr); }
  const ctx = canvas.getContext("2d")!;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);
  const css = getComputedStyle(document.documentElement);
  const accent = css.getPropertyValue("--accent").trim();
  const line = css.getPropertyValue("--border").trim();
  const faint = css.getPropertyValue("--faint").trim();
  const max = Math.max(1, ...data) * 1.2;
  // With a scale, labels get their own margin on the right so the graph
  // never draws over them.
  const gutter = grid ? 58 : 0;
  const pw = w - gutter, top = grid ? 6 : 2, bottom = grid ? 6 : 2;
  const yOf = (v: number) => h - bottom - (v / max) * (h - top - bottom);
  if (grid) {
    ctx.lineWidth = 1;
    ctx.font = `500 10.5px ${css.getPropertyValue("--font")}`;
    ctx.textBaseline = "middle";
    const hasData = data.some((v) => v > 0);
    for (const frac of [1 / 1.2, 0.5 / 1.2, 0]) {
      const y = Math.round(yOf(max * frac)) + 0.5;
      ctx.strokeStyle = line;
      ctx.setLineDash(frac ? [3, 3] : []);
      ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(pw, y); ctx.stroke();
      ctx.fillStyle = faint;
      if (hasData || !frac) ctx.fillText(frac ? speed(max * frac) : "0", pw + 8, y);
    }
    ctx.setLineDash([]);
  }
  const n = HISTORY, span = Math.max(Math.min(data.length, n), 30) - 1;
  const pts = data.slice(-n).map((v, i, a): [number, number] => [pw - (a.length - 1 - i) * (pw / span), yOf(v)]);
  if (pts.length < 2) return;
  const path = new Path2D();
  pts.forEach(([x, y], i) => {
    if (!i) path.moveTo(x, y);
    else { const [px, py] = pts[i - 1], cx = (px + x) / 2; path.bezierCurveTo(cx, py, cx, y, x, y); }
  });
  const area = new Path2D(path);
  area.lineTo(pts[pts.length - 1][0], h - bottom); area.lineTo(pts[0][0], h - bottom); area.closePath();
  const g = ctx.createLinearGradient(0, 0, 0, h);
  g.addColorStop(0, accent + "40"); g.addColorStop(1, accent + "00");
  if (fill) { ctx.fillStyle = g; ctx.fill(area); }
  ctx.strokeStyle = accent; ctx.lineWidth = 1.5; ctx.stroke(path);
}

// ---------- sidebar ----------
function renderNav() {
  const count = (fn: (i: Item) => boolean) => state.items.filter(fn).length;
  const live = state.items.some((i) => i.status === "downloading");
  const item = (view: string, label: string, ic: string, n: number, extra = "") =>
    `<button class="nav-item ${state.view === view ? "on" : ""}" data-view="${view}"><span class="ni-icon">${ic}</span><span class="ni-label">${label}</span>${extra}<span class="ni-count">${n || ""}</span></button>`;
  let html = item("all", "All downloads", icon("inbox"), state.items.length) +
    item("active", "In progress", icon("arrowDown"), count(VIEWS.active.test), live ? '<span class="ni-live"></span>' : "") +
    item("completed", "Completed", icon("doneCircle"), count(VIEWS.completed.test));
  const failed = count(VIEWS.failed.test);
  if (failed || state.view === "failed") html += item("failed", "Failed", icon("alert"), failed);
  html += `<button class="nav-sec ${state.typesOpen ? "" : "closed"}" data-toggle="types">Types ${icon("chevDown")}</button>`;
  if (state.typesOpen) for (const name of Object.keys(CATS)) html += item(name, name, typeIcon(name), count((i) => i.category === name));
  if ($("nav").innerHTML !== html) $("nav").innerHTML = html;

  const active = state.items.filter((i) => i.status === "downloading");
  const queued = state.items.filter((i) => i.status === "queued").length;
  const total = active.reduce((s, i) => s + (i.speed || 0), 0);
  const [v, u] = total > 0 ? parts(total) : ["0", "B"];
  $("net-speed").textContent = v;
  $("net-unit").textContent = `${u}/s`;
  const left = active.reduce((s, i) => s + Math.max(0, (i.size || 0) - i.downloaded), 0);
  $("net-foot").textContent = active.length ? `${active.length} active${queued ? `, ${queued} queued` : ""} · ${bytes(left)} left` : queued ? `${queued} queued` : "Idle";
  const lim = state.settings?.speedLimitKib || 0;
  $("limit-btn").textContent = lim ? `≤ ${bytes(lim * 1024)}/s` : "No limit";
  $("limit-btn").classList.toggle("on", !!lim);
  areaChart($("net-graph"), state.totalHist);
}

// ---------- header ----------
function renderHeader(list: Item[]) {
  const v = VIEWS[state.view];
  $("h-icon").innerHTML = v ? icon(v.icon) : typeIcon(state.view);
  $("h-name").textContent = v ? v.name : state.view;
  $("h-count").textContent = list.length ? String(list.length) : "";
  $("btn-pause-all").hidden = !list.some(isActive);
  $("btn-resume-all").hidden = !list.some((i) => i.status === "paused" || i.status === "error");
}

// ---------- list ----------
interface RowEl extends HTMLDivElement { _c: HTMLElement[]; _k: string[] }
const rowEls = new Map<number, RowEl>(), headEls = new Map<string, HTMLDivElement>();
function makeRow(id: number): RowEl {
  const el = document.createElement("div") as RowEl;
  el.className = "row"; el.dataset.id = String(id);
  el.innerHTML = `<span class="r-status"></span><div class="r-main"></div><div class="r-meta"></div>`;
  el._c = [...el.children] as HTMLElement[]; el._k = [];
  return el;
}
function set(el: RowEl, i: number, html: string) { if (el._k[i] !== html) { el._k[i] = html; el._c[i].innerHTML = html; } }
function meta(it: Item) {
  const p = Math.floor(pct(it));
  const mini = (cls: string) => `<span class="mini ${cls}"><i style="width:${pct(it)}%"></i></span><span class="pct">${it.size ? p + "%" : ""}</span>`;
  switch (it.status) {
    case "downloading":
      return `${mini("")}<span class="m-size">${bytes(it.downloaded)} / ${bytes(it.size)}</span><span class="m-speed">${it.speed > 0 ? speed(it.speed) : "Connecting…"}</span><span class="m-eta">${dur(eta(it))}</span>`;
    case "queued": return `${mini("queued")}<span class="m-size">${it.size ? bytes(it.size) : "—"}</span><span class="m-speed" style="color:var(--muted);font-weight:500">Queued · #${queuePos(it)}</span><span class="m-eta"></span>`;
    case "paused": return `${mini("paused")}<span class="m-size">${bytes(it.downloaded)} / ${bytes(it.size)}</span><span class="m-speed" style="color:var(--amber);font-weight:500">Paused</span><span class="m-eta"></span>`;
    case "error": return `<span class="m-size">${bytes(it.size)}</span><span class="m-when">${when(it.addedAt)}</span>`;
    default: return `<span class="cat"><i style="background:${cat(it).color}"></i>${esc(it.category)}</span><span class="m-size">${bytes(it.size)}</span><span class="m-when">${when(it.completedAt)}</span>`;
  }
}
function updateRow(el: RowEl, it: Item) {
  el.classList.toggle("sel", state.selected === it.id);
  set(el, 0, statusIcon(it));
  const e = ext(it.filename);
  set(el, 1, `<span class="r-name" title="${esc(it.filename)}">${esc(it.filename.replace(/\.[^.]+$/, "") || it.filename)}</span>${e ? `<span class="r-ext">${e}</span>` : ""}${it.status === "error" ? `<span class="r-err">${esc(it.error || "Failed")}</span>` : ""}`);
  set(el, 2, meta(it));
}
function renderList() {
  const list = visible();
  const nodes = [];
  for (const g of grouped(list)) {
    let h = headEls.get(g.s);
    if (!h) { h = document.createElement("div"); h.className = "ghead"; headEls.set(g.s, h); }
    const right = g.s === "downloading" ? speed(g.items.reduce((s, i) => s + (i.speed || 0), 0)) : g.s === "completed" ? bytes(g.items.reduce((s, i) => s + (i.size || 0), 0)) : "";
    const html = `${statusIcon({ status: g.s, size: 4, downloaded: 3 })}<span>${g.title}</span><span class="gc">${g.items.length}</span><span class="gr">${right}</span>${g.s === "completed" ? `<button type="button" class="gclear" data-clear title="Remove finished downloads from the list (the files stay)">Clear</button>` : ""}`;
    if (h.innerHTML !== html) h.innerHTML = html;
    nodes.push(h);
    for (const it of g.items) {
      let el = rowEls.get(it.id);
      if (!el) { el = makeRow(it.id); rowEls.set(it.id, el); }
      updateRow(el, it);
      nodes.push(el);
    }
  }
  for (const id of rowEls.keys()) if (!byId(id)) rowEls.delete(id);
  const root = $("rows");
  if (root.children.length !== nodes.length || nodes.some((n, i) => root.children[i] !== n)) root.replaceChildren(...nodes);

  $("empty").hidden = list.length > 0;
  if (!list.length) {
    const q = state.search.trim();
    $("empty-title").textContent = q ? "No results" : state.items.length ? "Nothing here" : "No downloads yet";
    $("empty-text").textContent = q ? `No downloads match “${q}”.` : state.items.length ? "Downloads in this view will show up here." : "Paste a link, drop one onto this window, or start a download in your browser.";
    $("empty-actions").hidden = !!q || state.items.length > 0;
  }
  return list;
}

// ---------- inspector ----------
let insKey: string | null = null;
function renderInspector() {
  const panel = $("inspector");
  const it = byId(state.selected);
  if (!it) { panel.hidden = true; insKey = null; return; }
  panel.hidden = false;
  const key = `${it.id}:${it.status}`;
  const dl = it.status === "downloading";
  if (insKey !== key) {
    insKey = key;
    panel.innerHTML = `<div class="ins-scroll">
        <div class="ins-top"><span class="ins-glyph">${icon(cat(it).icon)}</span><div class="ins-head"><div class="ins-title">${esc(it.filename)}</div><div class="ins-meta" id="ins-chips"></div></div><button class="sq-btn" data-ins="close" title="Close (Esc)">${icon("x")}</button></div>
        <div class="ins-sec"><div class="ins-sec-head">Progress<span id="ins-pct"></span></div><div class="segbar" id="ins-bar"></div><dl class="props" id="ins-stats"></dl></div>
        <div class="ins-sec" id="ins-graph-sec"><div class="ins-sec-head">Speed<span id="ins-peak"></span></div><canvas id="ins-graph"></canvas></div>
        <div class="ins-sec"><div class="ins-sec-head">Details</div><dl class="props" id="ins-props"></dl></div>
      </div>
      <div class="ins-foot" id="ins-foot"></div>`;
  }
  const finishing = dl && !!it.size && it.downloaded >= it.size;
  const meta = `<span class="st-${it.status}">${finishing ? "Finishing…" : STATUS[it.status]}</span><span class="trunc">from ${esc(host(it.url))}</span>`;
  if ($("ins-chips").innerHTML !== meta) $("ins-chips").innerHTML = meta;

  $("ins-pct").textContent = it.size ? `${Math.floor(pct(it))}%` : "";
  const bar = $("ins-bar");
  bar.className = `segbar ${it.status}`;
  segBar(bar, null, undefined, pct(it));
  const row = (k: string, v: string) => `<dt>${k}</dt><dd><span>${v}</span></dd>`;
  const stats = it.status === "completed"
    ? row("Size", bytes(it.size)) + row("Finished", when(it.completedAt) || "—") + (it.verified ? row("Checksum", "SHA-256 matches") : "")
    : row("Downloaded", it.size ? `${bytes(it.downloaded)} of ${bytes(it.size)}` : bytes(it.downloaded)) +
      (dl && !finishing ? row("Speed", speed(it.speed)) + row("Time left", dur(eta(it))) : "") +
      (dl && it.connections ? row("Connections", String(it.connections)) : "");
  if ($("ins-stats").innerHTML !== stats) $("ins-stats").innerHTML = stats;

  const hist = state.speedHist.get(it.id) || [];
  $("ins-graph-sec").hidden = !hist.some((v: number) => v > 0);
  if (!$("ins-graph-sec").hidden) { areaChart($("ins-graph"), hist, { fill: false }); $("ins-peak").textContent = `Peak ${speed(Math.max(...hist))}`; }

  const dir = it.path.replace(/[\\/][^\\/]*$/, "");
  const props = `
    <dt>Location</dt><dd><span class="link" data-ins="folder" title="${esc(it.path)}">${esc(dir)}</span><button class="copy" data-ins="copy-path" title="Copy path">${icon("copy")}</button></dd>
    <dt>Source</dt><dd><span title="${esc(it.url)}">${esc(it.url)}</span><button class="copy" data-ins="copy" title="Copy link">${icon("copy")}</button></dd>
    <dt>Resumable</dt><dd><span>${it.resumable ? "Yes" : "No, the server doesn't support it"}</span></dd>
    <dt>Added</dt><dd><span>${new Date(it.addedAt).toLocaleString([], { dateStyle: "medium", timeStyle: "short" })}</span></dd>
    ${it.error ? `<dt>Error</dt><dd><span style="color:var(--red)" title="${esc(it.error)}">${esc(it.error)}</span></dd>` : ""}`;
  if ($("ins-props").innerHTML !== props) $("ins-props").innerHTML = props;

  const primary = it.status === "completed" ? `<button class="btn primary" data-ins="open">Open</button>`
    : it.status === "error" ? `<button class="btn" data-ins="browser" title="Open the link in your browser, which downloads it with your own session">Download in browser</button><button class="btn primary" data-ins="resume">Retry</button>`
    : isActive(it) ? `<button class="btn" data-ins="pause">Pause</button>`
    : `<button class="btn primary" data-ins="resume">Resume</button>`;
  const foot = `<button class="btn" data-ins="folder">Show in folder</button>${primary}<button class="btn icon" data-ins="more" title="More">${icon("more")}</button>`;
  if ($("ins-foot").innerHTML !== foot) $("ins-foot").innerHTML = foot;
}

// ---------- browser integration ----------
let loadedOnce = false;
async function loadIntegration() {
  const before = state.integration?.lastSeen || 0;
  try { state.integration = await invoke("integration_status"); } catch { state.integration = null; }
  const now = state.integration;
  // The extension checks in as soon as it is added or reloaded: say so at once.
  if (now?.lastSeen && now.lastSeen > before && (!before || $("dlg-ext").open)) {
    const who = now.browsers.filter((x) => x.lastSeen).sort((x, y) => (y.lastSeen || 0) - (x.lastSeen || 0))[0];
    if ($("dlg-ext").open) $("dlg-ext").close();
    if (loadedOnce) toast(`Connected in ${who?.name || "your browser"}`, "ok");
  }
  loadedOnce = true;
  renderOnboard();
  if ($("dlg-settings").open) renderBrowserPanel();
}
const installedBrowsers = (): BrowserStatus[] => (state.integration?.browsers || []).filter((b) => b.installed);
function renderOnboard() {
  const card = $("onboard");
  const i = state.integration;
  const show = !!i && !i.lastSeen && !state.settings?.onboardingDismissed && installedBrowsers().length > 0;
  card.hidden = !show;
  if (!show || !i) return;
  // Until the extension is on the Chrome Web Store, store buttons would lead
  // to a missing page, so offer the guided manual install instead.
  const actions = i.storeAvailable
    ? installedBrowsers().slice(0, 3).map((b) => `<button class="link-btn" data-store="${b.id}">Add to ${b.name}</button>`).join("")
    : `<button class="link-btn" data-guide>Add the extension</button>`;
  const html = `<span class="ob-icon">${icon("globe")}</span>
    <span class="ob-text"><b>Connect your browser</b>Catch downloads automatically and get a download button on videos</span>
    <span class="ob-actions">${actions}<button class="sq-btn" data-dismiss title="Dismiss">${icon("x")}</button></span>`;
  // innerHTML never reads back identical, so compare with what we wrote last;
  // rebuilding it on every poll replaced the button mid-click.
  if (card.dataset.html !== html) { card.innerHTML = html; card.dataset.html = html; }
}
// One flow for every entry point: the banner, the empty state, the command
// palette and each browser in Settings → Browser.
const EXT_PAGE: Record<string, string> = { edge: "edge://extensions", brave: "brave://extensions", vivaldi: "vivaldi://extensions", firefox: "about:debugging" };
let guidePoll = 0;
async function openExtensionGuide(browserId?: string) {
  const i = state.integration;
  // Firefox has its listing on Firefox Add-ons; Chromium browsers wait for the Chrome Web Store.
  if (browserId === "firefox" || (i?.storeAvailable && browserId)) return openStore(browserId);
  const dir = await invoke("extension_dir").catch(() => null);
  const ff = browserId === "firefox";
  const path = (dir || "%LOCALAPPDATA%\\SwiftDL\\extension") + (ff ? "-firefox" : "");
  $("ext-path").textContent = path;
  $("ext-path").title = path;
  const b = installedBrowsers().find((x) => x.id === browserId);
  $("ext-step1").innerHTML = ff
    ? "Firefox: open <code>about:debugging</code>, choose This Firefox, then Load Temporary Add-on and pick <code>manifest.json</code> in the folder below"
    : `Type <code>${EXT_PAGE[browserId || ""] || "chrome://extensions"}</code> in the address bar${b ? `, or open it in ${b.name}:` : ""}`;
  const choices = b ? [b] : installedBrowsers().filter((x) => x.id !== "firefox");
  $("ext-browsers").innerHTML = choices.map((x) => `<button type="button" class="btn sm" data-open-ext="${x.id}"><img class="blogo sm" src="browsers/${x.id}.svg" alt="">Open in ${x.name}</button>`).join("");
  $("ext-wait").textContent = "Waiting for the extension to connect…";
  if (!$("dlg-ext").open) $("dlg-ext").showModal();
  if (b) openExtPage(b.id);
  // Check often while the guide is open, so it notices the connection at once.
  clearInterval(guidePoll);
  guidePoll = window.setInterval(() => {
    if (!$("dlg-ext").open) return clearInterval(guidePoll);
    loadIntegration();
  }, 1500);
}
async function openExtPage(id: string) {
  copy($("ext-path").textContent || "", "Folder path copied, paste it into the picker");
  try { await invoke("open_extensions_page", { browser: id }); }
  catch (e) { toast(esc(String(e)), "err"); }
}
$("ext-browsers").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>("[data-open-ext]"); if (b) openExtPage(b.dataset.openExt!); });
$("ext-copy").innerHTML = icon("copy");
$("ext-copy").onclick = () => copy($("ext-path").textContent || "", "Folder path copied");
$("ext-open").onclick = () => call("open_extension_folder");
$("ext-ff-store").onclick = (e) => { e.preventDefault(); openStore("firefox"); };
$("onboard").addEventListener("click", async (e) => {
  if (target(e).closest<HTMLElement>("[data-guide]")) return openExtensionGuide();
  const s = target(e).closest<HTMLElement>("[data-store]");
  if (s) return openStore(s.dataset.store!);
  if (target(e).closest<HTMLElement>("[data-dismiss]")) {
    await saveSettings({ ...settings(), onboardingDismissed: true });
    renderOnboard();
  }
});
async function openStore(browser: string) {
  await call("open_store", { browser });
  toast(browser === "firefox" ? "Opened Firefox Add-ons. Click “Add to Firefox” there." : "Opened the extension page. Click “Add to browser” there.", "ok");
}
function renderBrowserPanel() {
  const i = state.integration;
  if (!i) return;
  const store = i.storeAvailable;
  const hint = "Click Add next to a browser below";
  const connected = i.browsers.filter((b) => b.lastSeen).sort((a, b) => (b.lastSeen || 0) - (a.lastSeen || 0));
  const logo = (b: BrowserStatus, cls = "") => `<img class="blogo ${cls}" src="browsers/${b.id}.svg" alt="" width="28" height="28">`;
  $("ext-status").innerHTML = connected.length
    ? `${logo(connected[0])}<div class="sr-text"><b>Connected in ${connected.map((b) => b.name).join(", ").replace(/, ([^,]*)$/, " and $1")}</b><span>Last active ${when(connected[0].lastSeen).toLowerCase()}</span></div>`
    : `<div class="sr-text"><b>Not connected yet</b><span>${hint}</span></div>`;
  const rank = (b: BrowserStatus) => (b.lastSeen ? 2 : b.installed ? 1 : 0);
  $("browser-list").innerHTML = [...i.browsers].sort((a, b) => rank(b) - rank(a)).map((b) => {
    const sub = b.lastSeen ? `Connected · last active ${when(b.lastSeen).toLowerCase()}`
      : !b.installed ? "Not installed"
      : store && b.autoOffer ? "Installed · asks to enable the extension on next start" : "Installed";
    return `<div class="set-row">
      ${logo(b, b.installed || b.lastSeen ? "" : "off")}
      <div class="sr-text"><b>${b.name}</b><span>${sub}</span></div>
      ${!b.lastSeen && b.installed ? `<button type="button" class="btn sm" data-add="${b.id}">Add to ${b.name}</button>` : ""}</div>`;
  }).join("");
  $("ext-saveas").hidden = !connected.length;
  // The automatic offer only works once the extension is on the store.
  ($("set-offer").closest(".set-group") as HTMLElement).hidden = !store;
}
$("set-log").onclick = () => call("open_log");
$("browser-list").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>("[data-add]"); if (b) openExtensionGuide(b.dataset.add); });

function render() {
  if (state.selected != null && !byId(state.selected)) state.selected = null;
  const list = renderList();
  renderNav();
  renderHeader(list);
  renderInspector();
}

// ---------- toasts ----------
type ToastKind = "info" | "ok" | "err" | "dl";
function toast(msg: string, kind: ToastKind = "info") {
  const el = document.createElement("div");
  el.className = "toast";
  const ic = kind === "ok" ? "check" : kind === "err" ? "alert" : kind === "dl" ? "download" : "info";
  el.innerHTML = `<span class="t-ic ${kind}">${icon(ic)}</span><span>${msg}</span>`;
  $("toasts").append(el);
  setTimeout(() => { el.classList.add("out"); setTimeout(() => el.remove(), 200); }, 2800);
  while ($("toasts").children.length > 3) $("toasts").firstElementChild?.remove();
}
/** invoke(), with any error shown as a toast. */
async function call<K extends keyof Commands>(cmd: K, ...args: Commands[K][0] extends void ? [] : [Commands[K][0]]): Promise<Commands[K][1]> {
  try { return await invoke(cmd, ...args); } catch (e) { toast(esc(String(e)), "err"); throw e; }
}

// ---------- actions ----------
const resume = (ids: number[]) => { if (ids.length) call("resume", { ids }); };
const pause = (ids: number[]) => { if (ids.length) call("pause", { ids }); };
const copy = (text: string, msg: string) => navigator.clipboard.writeText(text).then(() => toast(msg, "ok"));
function toggle(it: Item | null | undefined) {
  if (!it || it.status === "completed") return;
  if (isActive(it)) pause([it.id]); else resume([it.id]);
}
function askDelete(it: Item | null | undefined) {
  if (!it) return;
  const dlg = $("dlg-delete");
  const live = it.status !== "completed";
  $("del-title").textContent = live ? "Cancel this download?" : "Remove from list?";
  $("del-text").textContent = live ? `“${it.filename}” will stop and its partial data will be deleted.` : `“${it.filename}” will be removed from SwiftDL.`;
  $("del-ok").textContent = live ? "Cancel download" : "Remove";
  $("del-files").checked = false;
  $("del-files-row").hidden = live;
  dlg.returnValue = "";
  dlg.onclose = () => {
    if (dlg.returnValue !== "ok") return;
    call("remove", { ids: [it.id], deleteFiles: $("del-files").checked });
    toast(live ? "Download cancelled" : "Removed from list", "ok");
  };
  dlg.showModal();
}
function act(name: string, it: Item | null | undefined, anchor?: HTMLElement) {
  if (!it) return;
  switch (name) {
    case "open": return void call("open_file", { id: it.id });
    case "folder": return void call("open_folder", { id: it.id });
    case "pause": return pause([it.id]);
    case "resume": return resume([it.id]);
    case "browser": return void call("open_in_browser", { url: it.url });
    case "copy": return void copy(it.url, "Link copied");
    case "copy-path": return void copy(it.path, "Path copied");
    case "delete": return askDelete(it);
    case "more": { const r = anchor!.getBoundingClientRect(); return itemMenu(it, r.right - 220, r.top - 8, true); }
  }
}
function select(id: number | null, scroll = true) {
  state.selected = id;
  render();
  if (scroll && id != null) rowEls.get(id)?.scrollIntoView({ block: "nearest" });
}
function move(d: number) {
  const ids = orderIds();
  if (!ids.length) return;
  const i = state.selected == null ? -1 : ids.indexOf(state.selected);
  select(ids[i < 0 ? (d > 0 ? 0 : ids.length - 1) : Math.max(0, Math.min(ids.length - 1, i + d))]);
}

// ---------- menus ----------
interface MenuEntry { label: string; icon?: string; keys?: string[]; enabled?: boolean; danger?: boolean; checked?: boolean; dot?: string; run: () => void }
type MenuLine = MenuEntry | "-" | { title: string };
function showMenu(x: number, y: number, entries: MenuLine[], above = false) {
  const menu = $("menu");
  menu.innerHTML = entries.map((e, i) => e === "-" ? `<div class="msep"></div>` : "title" in e ? `<div class="mtitle">${e.title}</div>` :
    `<button data-i="${i}" class="${e.danger ? "danger" : ""}" ${e.enabled === false ? "disabled" : ""}>${e.icon ? icon(e.icon) : e.dot ? `<span class="ni-dot" style="background:${e.dot};margin:0 3px"></span>` : ""}<span>${e.label}</span>${e.checked ? `<span class="mcheck">${icon("check")}</span>` : e.keys ? `<span class="mh">${e.keys.map((k) => `<kbd>${k}</kbd>`).join("")}</span>` : ""}</button>`).join("");
  menu.onclick = (ev) => { const b = target(ev).closest<HTMLElement>("button[data-i]"); if (b) { menu.hidden = true; (entries[Number(b.dataset.i)] as MenuEntry).run(); } };
  menu.hidden = false;
  const r = menu.getBoundingClientRect();
  if (above) y = y - r.height;
  menu.style.left = `${Math.max(8, Math.min(x, innerWidth - r.width - 8))}px`;
  menu.style.top = `${Math.max(8, Math.min(y, innerHeight - r.height - 8))}px`;
}
async function downloadNext(it: Item) {
  const ok = await invoke("download_next", { id: it.id }).catch(() => false);
  toast(ok ? "Will download next" : "It's not waiting in the queue", ok ? "ok" : undefined);
}
// A link that expired: copy the fresh one, then this carries on where it stopped.
async function replaceLink(it: Item) {
  const url = await readClip();
  if (!url) return toast("Copy the new link first, then choose this again");
  try { await invoke("set_url", { id: it.id, url }); toast("Link replaced, resuming", "ok"); }
  catch (e) { toast(esc(String(e)), "err"); }
}
function itemMenu(it: Item, x: number, y: number, above = false) {
  showMenu(x, y, [
    { icon: "open", label: "Open", keys: ["Enter"], enabled: it.status === "completed", run: () => act("open", it) },
    { icon: "folder", label: "Show in folder", keys: ["O"], run: () => act("folder", it) },
    "-",
    isActive(it) ? { icon: "pause", label: "Pause", keys: ["Space"], run: () => pause([it.id]) }
      : { icon: it.status === "error" ? "retry" : "play", label: it.status === "error" ? "Retry" : "Resume", keys: ["Space"], enabled: it.status !== "completed", run: () => resume([it.id]) },
    { icon: "play", label: "Download next", enabled: it.status === "queued", run: () => downloadNext(it) },
    { icon: "open", label: "Download in browser", enabled: it.status === "error", run: () => act("browser", it) },
    { icon: "link", label: "Replace link from clipboard", enabled: it.status === "error" || it.status === "paused", run: () => replaceLink(it) },
    { icon: "link", label: "Copy link", keys: ["C"], run: () => act("copy", it) },
    "-",
    { icon: "trash", label: it.status === "completed" ? "Remove…" : "Cancel download…", keys: ["Del"], danger: true, run: () => askDelete(it) },
  ], above);
}
document.addEventListener("mousedown", (e) => { if (!target(e).closest<HTMLElement>("#menu") && !target(e).closest<HTMLElement>("#limit-btn") && !target(e).closest<HTMLElement>("[data-ins=more]")) $("menu").hidden = true; });
document.addEventListener("contextmenu", (e) => { if (!target(e).closest<HTMLElement>("input")) e.preventDefault(); });

// ---------- events ----------
// Double-click is read from mousedown's click count: a live row's details
// are replaced every half second, and "dblclick" doesn't fire when the
// element under the first click is gone by the second.
$("rows").addEventListener("click", async (e) => {
  if (!target(e).closest<HTMLElement>("[data-clear]")) return;
  const ids = state.items.filter((i) => i.status === "completed").map((i) => i.id);
  if (!ids.length) return;
  await call("remove", { ids, deleteFiles: false });
  toast(`Cleared ${ids.length} from the list`, "ok");
});
$("rows").addEventListener("mousedown", (e) => {
  const r = target(e).closest<HTMLElement>(".row"), it = r ? byId(Number(r.dataset.id)) : undefined;
  if (!it || e.button !== 0) return;
  if (e.detail === 2) { if (it.status === "completed") act("open", it); else toggle(it); return; }
  select(it.id, false);
});
$("rows").addEventListener("contextmenu", (e) => { const r = target(e).closest<HTMLElement>(".row"); const it = r ? byId(Number(r.dataset.id)) : undefined; if (it) { e.preventDefault(); itemMenu(it, e.clientX, e.clientY); } });
$("list").addEventListener("mousedown", (e) => { if (!target(e).closest<HTMLElement>(".row") && !target(e).closest<HTMLElement>(".ghead") && !target(e).closest<HTMLElement>(".empty") && state.selected != null) { state.selected = null; render(); } });
$("inspector").addEventListener("click", (e) => {
  const b = target(e).closest<HTMLElement>("[data-ins]");
  if (!b) return;
  if (b.dataset.ins === "close") { state.selected = null; render(); return; }
  act(b.dataset.ins!, byId(state.selected), b);
});
$("nav").addEventListener("click", (e) => {
  if (target(e).closest<HTMLElement>("[data-toggle]")) { state.typesOpen = !state.typesOpen; render(); return; }
  const b = target(e).closest<HTMLElement>("[data-view]");
  if (b) { state.view = b.dataset.view!; state.search = ""; render(); }
});
$("btn-pause-all").onclick = () => pause(visible().filter(isActive).map((i) => i.id));
$("btn-resume-all").onclick = () => resume(visible().filter((i: Item) => i.status === "paused" || i.status === "error").map((i) => i.id));
$("btn-add").onclick = $("empty-add").onclick = () => openComposer();
$("btn-palette").onclick = $("btn-search").onclick = () => openPalette();
$("btn-settings").onclick = () => openSettings();
$("empty-ext").onclick = () => openExtensionGuide();
$("w-min").onclick = () => appWindow?.minimize();
$("w-max").onclick = () => appWindow?.toggleMaximize();
$("w-close").onclick = () => appWindow?.close();
if (!appWindow) $("win-ctrls").hidden = true;

async function setLimit(k: number) {
  await saveSettings({ ...settings(), speedLimitKib: k });
  toast(k ? `Speed limited to ${bytes(k * 1024)}/s` : "Speed limit removed", "ok");
}
const limitEntries = (): MenuLine[] => [
  { title: "Speed limit" },
  ...LIMITS.map((k): MenuEntry => ({ label: k ? `${bytes(k * 1024)}/s` : "No limit", checked: k === settings().speedLimitKib, run: () => setLimit(k) })),
  "-",
  { icon: "sliders", label: "Custom…", run: () => openSettings("speed") },
];
$("limit-btn").onclick = (e) => { const r = (e.currentTarget as HTMLElement).getBoundingClientRect(); showMenu(r.left, r.top - 6, limitEntries(), true); };

document.addEventListener("keydown", (e) => {
  const mod = e.ctrlKey || e.metaKey, k = e.key;
  if (mod && k.toLowerCase() === "k") { e.preventDefault(); if (!$("dlg-palette").open) openPalette(); return; }
  if (document.querySelector("dialog[open]") || target(e).matches?.("input")) return;
  const it = byId(state.selected);
  if (mod && k.toLowerCase() === "n") { e.preventDefault(); openComposer(); }
  else if (mod && k === ",") { e.preventDefault(); openSettings(); }
  else if (mod) return;
  else if (k === "ArrowDown" || k === "j") { e.preventDefault(); move(1); }
  else if (k === "ArrowUp" || k === "k") { e.preventDefault(); move(-1); }
  else if (k === " ") { e.preventDefault(); toggle(it); }
  else if (k === "Enter" && it) { e.preventDefault(); if (it.status === "completed") act("open", it); else toggle(it); }
  else if ((k === "Delete" || k === "Backspace") && it) { e.preventDefault(); askDelete(it); }
  else if (k === "o" && it) act("folder", it);
  else if (k === "c" && it) act("copy", it);
  else if (k === "n") openComposer();
  else if (k === "/") { e.preventDefault(); openPalette(); }
  else if (k === "Escape") { state.selected = null; render(); }
});

const firstUrl = (t: string | null | undefined) => (t || "").split(/\s+/).find((s) => /^(https?:\/\/|magnet:\?)\S+$/i.test(s));
document.addEventListener("paste", (e) => {
  if (document.querySelector("dialog[open]") || target(e).matches?.("input")) return;
  const url = firstUrl(e.clipboardData?.getData("text"));
  if (url) openComposer({ url });
});
let drag = 0;
document.addEventListener("dragenter", (e) => { if ([...(e.dataTransfer?.types || [])].some((t) => t === "text/uri-list" || t === "text/plain")) { drag++; $("drop").hidden = false; } });
document.addEventListener("dragleave", () => { if (--drag <= 0) { drag = 0; $("drop").hidden = true; } });
document.addEventListener("dragover", (e) => e.preventDefault());
document.addEventListener("drop", (e) => {
  e.preventDefault(); drag = 0; $("drop").hidden = true;
  const url = firstUrl(e.dataTransfer?.getData("text/uri-list") || e.dataTransfer?.getData("text/plain"));
  if (url) openComposer({ url });
});

// ---------- command palette ----------
interface PalItem { label: string; icon?: string; type?: string; status?: Item; sub?: string; keys?: string[]; run: () => void }
type PalLine = PalItem | { title: string };
const pal: { items: PalLine[]; sel: number } = { items: [], sel: 0 };
const selectable = () => pal.items.filter((i): i is PalItem => !("title" in i));
function paletteItems(q: string): PalLine[] {
  q = q.trim().toLowerCase();
  const match = (s: string) => !q || s.toLowerCase().includes(q);
  const out: PalLine[] = [];
  const actions: PalItem[] = [
    { icon: "plus", label: "New download", keys: ["Ctrl", "N"], run: () => openComposer() },
    { icon: "pause", label: "Pause all downloads", run: () => pause(state.items.filter(isActive).map((i) => i.id)) },
    { icon: "play", label: "Resume all downloads", run: () => resume(state.items.filter((i) => i.status === "paused" || i.status === "error").map((i) => i.id)) },
    ...LIMITS.map((k): PalItem => ({ icon: "gauge", label: k ? `Limit speed to ${bytes(k * 1024)}/s` : "Remove speed limit", run: () => setLimit(k) })),
    { icon: "settings", label: "Open settings", keys: ["Ctrl", ","], run: () => openSettings() },
    { icon: "globe", label: "Set up browser extension", run: () => openExtensionGuide() },
  ].filter((a) => match(a.label));
  const views: PalItem[] = [
    ...Object.entries(VIEWS).map(([k, v]): PalItem => ({ icon: v.icon, label: `Go to ${v.name}`, run: () => { state.view = k; render(); } })),
    ...Object.keys(CATS).map((c): PalItem => ({ type: c, label: `Go to ${c}`, run: () => { state.view = c; render(); } })),
  ].filter((a) => match(a.label));
  const dls = state.items.filter((i) => q && (i.filename.toLowerCase().includes(q) || i.url.toLowerCase().includes(q))).slice(0, 8)
    .map((i): PalItem => ({ status: i, label: i.filename, sub: STATUS[i.status], run: () => { state.view = "all"; select(i.id); } }));
  if (dls.length) out.push({ title: "Downloads" }, ...dls);
  if (actions.length) out.push({ title: "Actions" }, ...actions);
  if (views.length && q) out.push({ title: "Navigation" }, ...views);
  else if (!q) out.push({ title: "Navigation" }, ...views.slice(0, 4));
  return out;
}
function renderPalette() {
  const q = $("pal-q").value;
  pal.items = paletteItems(q);
  const sel = selectable();
  pal.sel = Math.min(pal.sel, Math.max(0, sel.length - 1));
  let n = -1;
  $("pal-list").innerHTML = sel.length ? pal.items.map((i) => {
    if ("title" in i) return `<div class="pal-sec">${i.title}</div>`;
    n++;
    const ic = i.status ? statusIcon(i.status) : i.type ? typeIcon(i.type) : icon(i.icon || "file");
    return `<button class="pal-item ${n === pal.sel ? "on" : ""}" data-n="${n}"><span class="pi-ic">${ic}</span><span class="pi-label">${esc(i.label)}${i.sub ? `<span class="pi-sub">${i.sub}</span>` : ""}</span>${i.keys ? `<span class="pi-right">${i.keys.map((k) => `<kbd>${k}</kbd>`).join("")}</span>` : ""}</button>`;
  }).join("") : `<div class="pal-empty">No results for “${esc(q)}”</div>`;
  $("pal-list").querySelector(".pal-item.on")?.scrollIntoView({ block: "nearest" });
}
function openPalette() {
  pal.sel = 0;
  $("pal-q").value = "";
  renderPalette();
  $("dlg-palette").showModal();
  $("pal-q").focus();
}
function runPalette(n: number) {
  const item = selectable()[n];
  if (!item) return;
  $("dlg-palette").close();
  item.run();
}
$("pal-q").addEventListener("input", () => { pal.sel = 0; renderPalette(); });
$("pal-q").addEventListener("keydown", (e) => {
  const count = selectable().length;
  if (e.key === "ArrowDown") { e.preventDefault(); pal.sel = (pal.sel + 1) % Math.max(1, count); renderPalette(); }
  else if (e.key === "ArrowUp") { e.preventDefault(); pal.sel = (pal.sel - 1 + count) % Math.max(1, count); renderPalette(); }
  else if (e.key === "Enter") { e.preventDefault(); runPalette(pal.sel); }
});
$("pal-list").addEventListener("mousemove", (e) => {
  const b = target(e).closest<HTMLElement>(".pal-item");
  if (b && Number(b.dataset.n) !== pal.sel) { pal.sel = Number(b.dataset.n); $("pal-list").querySelectorAll(".pal-item").forEach((x) => x.classList.toggle("on", x === b)); }
});
$("pal-list").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>(".pal-item"); if (b) runPalette(Number(b.dataset.n)); });
$("dlg-palette").addEventListener("click", (e) => { if (e.target === $("dlg-palette")) $("dlg-palette").close(); });

// ---------- composer (new download) ----------
const add: { headers: Header[]; seq: number; timer: number; dirTouched: boolean; dir: string; info: ProbeInfo | null; suggest: string } =
  { headers: [], seq: 0, timer: 0, dirTouched: false, dir: "", info: null, suggest: "" };
interface ComposerInput { url: string; headers?: Header[]; filename?: string | null }
function openComposer(incoming?: ComposerInput) {
  const dlg = $("dlg-add");
  if (dlg.open) return;
  add.headers = incoming?.headers || [];
  add.suggest = incoming?.filename || "";
  add.dirTouched = false;
  add.info = null;
  add.dir = state.settings?.downloadDir || "";
  $("add-url").value = incoming?.url || "";
  $("add-file").hidden = true;
  renderChips();
  setState("idle");
  dlg.showModal();
  $("add-url").focus();
  if (incoming?.url) probe();
  else readClip().then((u) => { if (u && !$("add-url").value) { $("add-url").value = u; $("add-url").select(); probe(); } });
}
// Opt-in: switching back to SwiftDL after copying a download link offers to download it.
let lastClip = "";
window.addEventListener("focus", async () => {
  if (!state.settings?.watchClipboard || $("dlg-add").open || $("dlg-settings").open) return;
  const u = await readClip();
  if (!u || u === lastClip) return;
  const file = /\.(zip|rar|7z|iso|exe|msi|dmg|pkg|deb|rpm|apk|mp4|mkv|mp3|flac|pdf|torrent)(\?|#|$)/i.test(u) || /^magnet:/i.test(u);
  if (!file && !(await invoke("is_media_url", { url: u }).catch(() => false))) return;
  lastClip = u;
  openComposer();
});
const readClip = (): Promise<string | undefined> => (navigator.clipboard?.readText?.() || Promise.resolve("")).then(firstUrl).catch(() => undefined);
function setState(kind: "idle" | "loading" | "error" | "done", msg = "") {
  const el = $("add-state");
  el.className = `comp-state ${kind === "error" ? "err" : ""}`;
  el.innerHTML = kind === "loading" ? `<span class="spinner"></span>Fetching file details…` : kind === "error" ? `${icon("alert")}${esc(msg)}` : kind === "idle" ? `SwiftDL checks the file's name and size before downloading.` : "";
  el.hidden = kind === "done";
  $("add-start").disabled = $("add-later").disabled = kind !== "done";
}
function renderChips() {
  const i = add.info;
  const folder = add.dir.split(/[\\/]/).filter(Boolean).slice(-2).join(" / ") || add.dir;
  $("add-chips").innerHTML = i ? [
    `<span class="chip">${icon("hash")}<span>${i.size != null ? bytes(i.size) : "Unknown size"}</span></span>`,
    `<span class="chip"><i class="dot" style="background:${(CATS[i.category] || CATS.Other).color}"></i><span>${esc(i.category)}</span></span>`,
    i.resumable ? `<span class="chip ok">${icon("bolt")}<span>${state.settings?.connections || 8} connections</span></span>` : `<span class="chip warn">${icon("info")}<span>Single connection, no resume</span></span>`,
    `<button type="button" class="chip" id="add-folder" title="${esc(add.dir)}">${icon("folder")}<span>${esc(folder)}</span>${icon("chevDown")}</button>`,
  ].join("") : "";
}
async function probe() {
  const url = $("add-url").value.trim();
  if (!/^(https?:\/\/|ftp:\/\/|magnet:\?)\S+/i.test(url)) { add.info = null; $("add-file").hidden = true; renderChips(); if (url) setState("error", "Paste a web link (http://, https:// or ftp://) or a magnet link"); else setState("idle"); return; }
  // A Metalink lists several mirrors of one file: download from all of them.
  if (/\.(meta4|metalink)(\?|#|$)/i.test(url)) {
    try {
      const [name, n] = await invoke("add_metalink", { url });
      $("dlg-add").close();
      if (state.view !== "all" && state.view !== "active") state.view = "all";
      toast(`Downloading ${esc(name)} from ${n} address${n === 1 ? "" : "es"}`, "dl");
    } catch (e) { setState("error", `Couldn't read that Metalink: ${e}`); }
    return;
  }
  const seq = ++add.seq;
  // Video sites need a quality and torrents a file choice: their own window.
  const special = await invoke("is_media_url", { url }).catch(() => false) || await invoke("is_torrent_url", { url, filename: null }).catch(() => false);
  if (special) {
    $("dlg-add").close();
    invoke("open_new_download", { url });
    return;
  }
  setState("loading");
  try {
    const info = await invoke("probe_url", { url, headers: add.headers });
    if (seq !== add.seq) return;
    add.info = info;
    if (!add.dirTouched) add.dir = info.dir;
    // A name from the extension (e.g. the video page's title) beats a
    // generic server name like "videoplayback".
    $("add-filename").value = /\.[a-z0-9]{2,5}$/i.test(add.suggest) ? add.suggest : info.filename;
    $("add-tile").innerHTML = ftile(info);
    $("add-sub").textContent = host(url);
    $("add-file").hidden = false;
    renderChips();
    setState("done");
  } catch (e) {
    if (seq !== add.seq) return;
    add.info = null; $("add-file").hidden = true; renderChips();
    setState("error", `Couldn't reach that link: ${e}`);
  }
}
// ---------- find files on a page ----------
type Found = { url: string; name: string };
const grab = { all: [] as Found[], picked: new Set<string>(), cat: "File", q: "" };
const looksLikeFile = (n: string) => /\.[a-z0-9]{2,5}$/i.test(n) && !/\.(html?|php|aspx?|jsp|js|css|json|xml|map)$/i.test(n);
function grabVisible(): Found[] {
  return grab.all.filter((f) => (grab.cat === "File" ? looksLikeFile(f.name) : grab.cat === "All" ? true : looksLikeFile(f.name) && catOf(f.name) === grab.cat));
}
function renderGrab() {
  const list = grabVisible();
  const cats = ["File", ...Object.keys(CATS).filter((c) => c !== "Other" && grab.all.some((f) => looksLikeFile(f.name) && catOf(f.name) === c)), "All"];
  $("grab-chips").innerHTML = cats.map((c) => `<button type="button" class="chip${c === grab.cat ? " on" : ""}" data-c="${c}"><span>${c === "File" ? "All files" : c === "All" ? "Every link" : c}</span></button>`).join("");
  $("grab-list").innerHTML = list.map((f) => `<label class="grab-row"><input type="checkbox" data-u="${esc(f.url)}"${grab.picked.has(f.url) ? " checked" : ""}><span class="gn" title="${esc(f.url)}">${esc(f.name || f.url)}</span><span class="gh">${esc(host(f.url))}</span></label>`).join("") || `<p class="muted" style="padding:10px 12px">Nothing here.</p>`;
  const n = grab.picked.size;
  $("grab-go").disabled = !n;
  $("grab-go").textContent = n ? `Download ${n} file${n === 1 ? "" : "s"}` : "Download";
  $("grab-all").textContent = list.length && list.every((f) => grab.picked.has(f.url)) ? "Select none" : "Select all";
}
async function loadGrab() {
  const url = $("grab-url").value.trim();
  if (!/^https?:\/\//i.test(url)) return;
  $("grab-state").textContent = "Reading the page…";
  try {
    grab.all = await invoke("grab_page", { url });
    grab.picked.clear();
    $("grab-state").textContent = grab.all.length ? `${grabVisible().length} files on this page` : "No links found on this page";
    renderGrab();
  } catch (e) { $("grab-state").textContent = `Couldn't read that page: ${e}`; }
}
$("add-grab").onclick = () => {
  const url = $("add-url").value.trim();
  $("dlg-add").close();
  $("grab-url").value = /^https?:\/\//i.test(url) ? url : "";
  grab.all = []; grab.picked.clear(); grab.cat = "File"; $("grab-state").textContent = ""; renderGrab();
  $("dlg-grab").showModal();
  $("grab-url").focus();
  if ($("grab-url").value) loadGrab();
};
$("grab-x").onclick = () => $("dlg-grab").close();
$("grab-url").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); loadGrab(); } });
$("grab-url").addEventListener("paste", () => setTimeout(loadGrab, 50));
$("grab-chips").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>("[data-c]"); if (b) { grab.cat = b.dataset.c!; renderGrab(); } });
$("grab-list").addEventListener("change", (e) => { const i = target(e) as HTMLInputElement; if (i.dataset.u) { i.checked ? grab.picked.add(i.dataset.u) : grab.picked.delete(i.dataset.u); renderGrab(); } });
$("grab-all").onclick = () => { const list = grabVisible(); const all = list.every((f) => grab.picked.has(f.url)); list.forEach((f) => (all ? grab.picked.delete(f.url) : grab.picked.add(f.url))); renderGrab(); };
$("grab-go").onclick = async () => {
  const n = await invoke("add_links", { text: [...grab.picked].join("\n") }).catch(() => 0);
  $("dlg-grab").close();
  if (state.view !== "all" && state.view !== "active") state.view = "all";
  toast(`Adding ${n} download${n === 1 ? "" : "s"}`, "dl");
};

// Several links, or a [01-50] pattern: add them all at once.
$("add-url").addEventListener("paste", async (e) => {
  const text = e.clipboardData?.getData("text") || "";
  if (!/\s/.test(text.trim()) && !/\[\d+-\d+\]/.test(text)) return;
  e.preventDefault();
  const n = await invoke("add_links", { text }).catch(() => 0);
  if (!n) return toast("No links found in what you pasted", "err");
  $("dlg-add").close();
  if (state.view !== "all" && state.view !== "active") state.view = "all";
  toast(`Adding ${n} download${n === 1 ? "" : "s"}`, "dl");
});
$("add-url").addEventListener("input", () => { clearTimeout(add.timer); add.timer = window.setTimeout(probe, 400); });
$("add-chips").addEventListener("click", async (e) => {
  if (!target(e).closest<HTMLElement>("#add-folder")) return;
  const dir = await call("pick_folder", { start: add.dir || null });
  if (dir) { add.dir = dir; add.dirTouched = true; renderChips(); }
});
$("add-paste").onclick = async () => { const u = await readClip(); if (u) { $("add-url").value = u; probe(); } else toast("No link on the clipboard"); };
$("add-x").onclick = () => $("dlg-add").close();
$("add-later").onclick = () => submitAdd(false);
$("form-add").addEventListener("submit", (e) => { e.preventDefault(); if (!$("add-start").disabled) submitAdd(true); });
$("dlg-add").addEventListener("keydown", (e) => { if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && !$("add-start").disabled) { e.preventDefault(); submitAdd(true); } });
$("dlg-add").addEventListener("close", () => { add.seq++; setTimeout(nextPending, 150); });
async function submitAdd(start: boolean) {
  const filename = $("add-filename").value.trim();
  if (!filename) return;
  const id = await call("add_download", { req: { url: $("add-url").value.trim(), filename, dir: add.dir || settings().downloadDir, headers: add.headers, start } });
  $("dlg-add").close();
  if (state.view !== "all" && state.view !== "active") state.view = "all";
  select(id);
  toast(start ? `Downloading ${esc(filename)}` : `${esc(filename)} added for later`, start ? "dl" : "ok");
}
function nextPending() {
  if ($("dlg-add").open) return;
  const inc = state.pending.shift();
  if (!inc) return;
  const headers: Header[] = [];
  if (inc.cookies) headers.push(["Cookie", inc.cookies]);
  if (inc.referrer) headers.push(["Referer", inc.referrer]);
  if (inc.userAgent) headers.push(["User-Agent", inc.userAgent]);
  openComposer({ url: inc.url, headers, filename: inc.filename });
}

// ---------- settings ----------
// Interface size: the whole window scales (the download windows keep their size).
const applyScale = (pct?: number) => { document.documentElement.style.zoom = String((pct && pct >= 80 && pct <= 160 ? pct : 100) / 100); };
let setScale = 100, setDebridProvider = "realdebrid";
// The local API needs a secret: make one when it is turned on without one.
const apiHint = () => { $("set-api-hint").textContent = `Web remote: http://127.0.0.1:${$("set-api-port").value || 6800}   ·   For tools: /jsonrpc`; };
$("set-api").addEventListener("change", () => {
  if ($("set-api").checked && !$("set-api-secret").value.trim()) {
    $("set-api-secret").value = [...crypto.getRandomValues(new Uint8Array(12))].map((b) => b.toString(16).padStart(2, "0")).join("");
  }
});
$("set-api-port").addEventListener("input", apiHint);
function renderScale() { $("set-scale").querySelectorAll<HTMLElement>("[data-v]").forEach((b) => b.classList.toggle("on", Number(b.dataset.v) === setScale)); }
$("set-scale").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>("[data-v]"); if (b) { setScale = Number(b.dataset.v); renderScale(); applyScale(setScale); } });
function renderDebridProvider() { $("set-debrid-provider").querySelectorAll<HTMLElement>("[data-v]").forEach((b) => b.classList.toggle("on", b.dataset.v === setDebridProvider)); }
$("set-debrid-provider").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>("[data-v]"); if (b) { setDebridProvider = b.dataset.v!; renderDebridProvider(); } });
let setDir = "", setLimitKib = 0, setThemeChoice = "system", setSeed = "stop", setWhenDone = "nothing", setStartAt = 0;
function renderWhenDone() {
  $("set-whendone").querySelectorAll<HTMLElement>("[data-v]").forEach((b) => b.classList.toggle("on", b.dataset.v === setWhenDone));
}
$("set-whendone").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>("[data-v]"); if (b) { setWhenDone = b.dataset.v!; renderWhenDone(); } });
// "Start at 02:00" means the next 02:00: tonight, or tomorrow if it has passed.
function nextTime(hhmm: string): number {
  const [h, m] = hhmm.split(":").map(Number);
  const d = new Date(); d.setHours(h, m, 0, 0);
  if (d.getTime() <= Date.now()) d.setDate(d.getDate() + 1);
  return d.getTime();
}
function renderStartAt() {
  const on = setStartAt > Date.now();
  $("set-startat").value = on ? new Date(setStartAt).toTimeString().slice(0, 5) : "";
  $("set-startat-hint").textContent = on ? `Downloads wait until ${new Date(setStartAt).toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit" })}` : "Downloads wait until this time, once";
}
$("set-startat").addEventListener("change", () => { setStartAt = $("set-startat").value ? nextTime($("set-startat").value) : 0; renderStartAt(); });
$("set-startat-clear").addEventListener("click", () => { setStartAt = 0; renderStartAt(); });
function renderSeed() {
  $("set-seed").querySelectorAll<HTMLElement>("[data-v]").forEach((b) => b.classList.toggle("on", b.dataset.v === setSeed));
}
$("set-seed").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>("[data-v]"); if (b) { setSeed = b.dataset.v!; renderSeed(); } });
function renderThemeChoice() {
  $("set-theme").querySelectorAll<HTMLElement>("[data-v]").forEach((b) => b.classList.toggle("on", b.dataset.v === setThemeChoice));
}
// Picking a theme previews it at once; Cancel puts the saved one back.
$("set-theme").addEventListener("click", (e) => {
  const b = target(e).closest<HTMLElement>("[data-v]");
  if (!b) return;
  setThemeChoice = b.dataset.v!;
  renderThemeChoice();
  window.setTheme(setThemeChoice);
});
const restoreTheme = () => { window.setTheme(settings().theme); applyScale(settings().uiScale); };
$("dlg-settings").addEventListener("cancel", restoreTheme); // Escape
$("dlg-settings").addEventListener("close", () => { restoreTheme(); applyScale(state.settings?.uiScale); });
function openSettings(tab = "general") {
  const s = settings();
  setThemeChoice = s.theme || "system";
  renderThemeChoice();
  setSeed = s.seedUntil || "stop";
  renderSeed();
  setWhenDone = s.whenDone || "nothing";
  renderWhenDone();
  setStartAt = s.startAt || 0;
  renderStartAt();
  setDir = s.downloadDir; $("set-dir").textContent = setDir;
  $("set-sort").checked = s.sortIntoCategories;
  $("set-ask").checked = s.askBeforeDownload;
  $("set-notify").checked = s.notifyOnComplete;
  $("set-clip").checked = s.watchClipboard;
  $("set-extract").checked = s.extractArchives;
  $("set-proxy").value = s.proxy || "";
  $("set-bind").value = s.bindAddress || "";
  $("set-debrid").value = s.debridToken || "";
  setDebridProvider = s.debridProvider || "realdebrid";
  renderDebridProvider();
  setScale = s.uiScale || 100;
  renderScale();
  $("set-api").checked = s.apiEnabled;
  $("set-api-port").value = String(s.apiPort || 6800);
  $("set-api-secret").value = s.apiSecret || "";
  $("set-api-lan").checked = s.apiLan;
  apiHint();
  $("set-offer").checked = s.offerExtension;
  $("set-progress-win").checked = s.showProgressWindow;
  $("set-tray").checked = s.closeToTray;
  loadIntegration();
  $("set-conns").value = String(s.connections);
  $("set-concurrent").value = String(s.maxConcurrent);
  setLimitKib = s.speedLimitKib;
  renderLimits();
  showTab(tab);
  $("dlg-settings").showModal();
}
function renderLimits() {
  $("set-limits").innerHTML = LIMITS.map((k) => `<button type="button" data-k="${k}" class="${k === setLimitKib ? "on" : ""}">${k ? bytes(k * 1024) + "/s" : "Off"}</button>`).join("");
  const c = $("set-limit-custom");
  if (document.activeElement !== c) c.value = LIMITS.includes(setLimitKib) ? "" : String(setLimitKib);
}
$("set-limits").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>("[data-k]"); if (b) { setLimitKib = Number(b.dataset.k); renderLimits(); } });
$("set-limit-custom").addEventListener("input", (e) => { setLimitKib = Math.max(0, Number.parseInt($("set-limit-custom").value, 10) || 0); renderLimits(); });
function showTab(tab: string) {
  $("set-nav").querySelectorAll<HTMLElement>("[data-tab]").forEach((b) => b.classList.toggle("on", b.dataset.tab === tab));
  document.querySelectorAll<HTMLElement>("[data-panel]").forEach((p) => (p.hidden = p.dataset.panel !== tab));
  // Nothing to save on these tabs (Browser has a setting only once the store listing exists).
  ($("dlg-settings").querySelector(".set-foot") as HTMLElement).hidden = tab === "about" || (tab === "browser" && !state.integration?.storeAvailable);
}
$("set-nav").addEventListener("click", (e) => { const b = target(e).closest<HTMLElement>("[data-tab]"); if (b) showTab(b.dataset.tab!); });
document.querySelectorAll<HTMLElement>(".stepper").forEach((st) => st.addEventListener("click", (e) => {
  const b = target(e).closest<HTMLElement>("[data-d]");
  if (!b) return;
  const input = $(st.dataset.for!);
  input.value = String(Math.min(+input.max, Math.max(+input.min, (+input.value || 0) + Number(b.dataset.d))));
}));
$("set-browse").onclick = async () => { const d = await call("pick_folder", { start: setDir || null }); if (d) { setDir = d; $("set-dir").textContent = d; } };
$("set-cancel").onclick = () => { restoreTheme(); $("dlg-settings").close(); };
async function saveSettings(s: Settings) { await call("set_settings", { settings: s }); state.settings = s; render(); }
$("form-settings").addEventListener("submit", async (e) => {
  e.preventDefault();
  const clamp = (v: string, lo: number, hi: number, d: number) => Math.min(hi, Math.max(lo, Number.parseInt(v, 10) || d));
  await saveSettings({
    downloadDir: setDir || settings().downloadDir,
    sortIntoCategories: $("set-sort").checked,
    connections: clamp($("set-conns").value, 1, 32, 8),
    maxConcurrent: clamp($("set-concurrent").value, 1, 10, 3),
    speedLimitKib: setLimitKib,
    askBeforeDownload: $("set-ask").checked,
    notifyOnComplete: $("set-notify").checked,
    watchClipboard: $("set-clip").checked,
    extractArchives: $("set-extract").checked,
    proxy: $("set-proxy").value.trim(),
    bindAddress: $("set-bind").value.trim(),
    debridProvider: setDebridProvider,
    debridToken: $("set-debrid").value.trim(),
    uiScale: setScale,
    apiEnabled: $("set-api").checked && !!$("set-api-secret").value.trim(),
    apiPort: clamp($("set-api-port").value, 1024, 65535, 6800),
    apiSecret: $("set-api-secret").value.trim(),
    apiLan: $("set-api-lan").checked && $("set-api-secret").value.trim().length >= 12,
    offerExtension: $("set-offer").checked,
    showProgressWindow: $("set-progress-win").checked,
    closeToTray: $("set-tray").checked,
    onboardingDismissed: settings().onboardingDismissed,
    theme: setThemeChoice,
    seedUntil: setSeed,
    whenDone: setWhenDone,
    startAt: setStartAt,
  });
  $("dlg-settings").close();
  toast("Settings saved", "ok");
});

// ---------- data ----------
function ingest(items: Item[]) {
  state.items = items;
  let total = 0;
  for (const it of items) {
    if (it.status !== "downloading") continue;
    const h = state.speedHist.get(it.id) || [];
    h.push(it.speed || 0);
    if (h.length > HISTORY) h.shift();
    state.speedHist.set(it.id, h);
    total += it.speed || 0;
  }
  state.totalHist.push(total);
  if (state.totalHist.length > HISTORY) state.totalHist.shift();
  render();
}
// Updates: check a few seconds after start, then twice a day. A dismissed
// version stays quiet; the next one shows again.
let update: UpdateInfo | null = null;
const SKIP_KEY = "swiftdl.skipUpdate";
function renderUpdate(busy = false) {
  const card = $("update");
  let skipped = "";
  try { skipped = localStorage.getItem(SKIP_KEY) || ""; } catch {}
  card.hidden = !update || update.version === skipped;
  if (card.hidden || !update) return;
  const install = update.asset ? (update.asset.endsWith(".exe") ? "Install and restart" : "Download") : "Get it";
  card.innerHTML = `<span class="ob-icon">${icon("download")}</span>
    <span class="ob-text"><b>SwiftDL ${esc(update.version)} is available</b>${busy ? "Downloading the update…" : "You have " + esc(state.version)}</span>
    <span class="ob-actions">${busy ? "" : `<button class="link-btn" data-notes>What's new</button><button class="link-btn" data-install>${install}</button><button class="sq-btn" data-skip title="Skip this version">${icon("x")}</button>`}</span>`;
}
async function checkUpdate() {
  try { update = await invoke("check_update"); } catch { return; }
  renderUpdate();
}
$("update").addEventListener("click", async (e) => {
  const b = (e.target as HTMLElement).closest("button");
  if (!b || !update) return;
  if (b.hasAttribute("data-notes")) invoke("open_url", { url: update.page });
  else if (b.hasAttribute("data-skip")) {
    try { localStorage.setItem(SKIP_KEY, update.version); } catch {}
    renderUpdate();
  } else if (b.hasAttribute("data-install")) {
    renderUpdate(true);
    try { await invoke("install_update", { url: update.asset || update.page }); renderUpdate(); }
    catch (err) { renderUpdate(); toast(`Update failed: ${err}`); }
  }
});
async function boot() {
  hydrate();
  const s = await invoke("get_state");
  state.settings = s.settings;
  window.setTheme(s.settings.theme);
  applyScale(s.settings.uiScale);
  state.pending = s.pending;
  state.version = s.version;
  $("version").textContent = `Version ${s.version}`;
  ingest(s.items);
  nextPending();
  loadIntegration();
  // Every 2 s until a browser connects, so adding the extension shows up at
  // once; after that a slow check is enough. Also on coming back to the window.
  let tick = 0;
  setInterval(() => { if (!state.integration?.lastSeen || ++tick % 8 === 0) loadIntegration(); }, 2000);
  window.addEventListener("focus", () => loadIntegration());
  setTimeout(checkUpdate, 5000);
  setInterval(checkUpdate, 12 * 60 * 60 * 1000);
  await listen("downloads", (e) => ingest(e.payload));
  // A one-time setting finished (when-done fired, or the queue's start time came).
  await listen("settings", (e) => { state.settings = e.payload; if ($("dlg-settings").open) openSettings(); });
  await listen("incoming", () => invoke("get_state").then((s) => { state.pending.push(...s.pending); nextPending(); }));
  setInterval(() => {
    if (!state.items.some((i) => i.status === "downloading")) {
      state.totalHist.push(0);
      if (state.totalHist.length > HISTORY) state.totalHist.shift();
      areaChart($("net-graph"), state.totalHist);
    }
  }, 500);
  addEventListener("resize", () => render());
}
boot();
