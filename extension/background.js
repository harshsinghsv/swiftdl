// SwiftDL browser integration.
//
// Catches downloads the browser starts and hands them to the SwiftDL app
// (through native messaging), along with the cookies and referrer the site
// expects, so downloads behind a login keep working. If the app isn't
// installed or refuses, the browser simply finishes the download itself.

const HOST = "com.swiftdl.host";
const DEFAULTS = { enabled: true, minSizeKB: 0 };

// URLs the user explicitly wants the browser to handle (from the fallback path).
const bypass = new Set();

const getSettings = () => chrome.storage.sync.get(DEFAULTS);

// Last few events, kept for troubleshooting (shown nowhere yet; readable
// from the service worker console or chrome.storage.local).
async function log(event) {
  console.log("SwiftDL:", event);
  const { log: entries = [] } = await chrome.storage.local.get("log");
  entries.push(`${new Date().toISOString()} ${event}`);
  await chrome.storage.local.set({ log: entries.slice(-30) });
}

async function cookieHeader(url) {
  try {
    const cookies = await chrome.cookies.getAll({ url });
    return cookies.map((c) => `${c.name}=${c.value}`).join("; ");
  } catch {
    return "";
  }
}

async function sendToApp({ url, referrer, filename }) {
  const message = {
    type: "download",
    url,
    referrer: referrer || "",
    filename: filename ? filename.split(/[\\/]/).pop() : "",
    cookies: await cookieHeader(url),
    userAgent: navigator.userAgent,
  };
  const reply = await chrome.runtime.sendNativeMessage(HOST, message);
  if (!reply?.ok) throw new Error(reply?.error || "SwiftDL did not accept the download");
}

function flashBadge(text, color) {
  chrome.action.setBadgeBackgroundColor({ color });
  chrome.action.setBadgeText({ text });
  setTimeout(() => chrome.action.setBadgeText({ text: "" }), 2500);
}

chrome.downloads.onCreated.addListener(async (item) => {
  const settings = await getSettings();
  const url = item.finalUrl || item.url;
  await log(`created #${item.id} state=${item.state} ${url}`);
  if (!settings.enabled || item.state !== "in_progress") return;

  // blob:, data: and file: URLs only exist inside the browser.
  if (!/^https?:/i.test(url)) return;
  if (bypass.delete(url)) return;
  if (settings.minSizeKB > 0 && item.totalBytes > 0 && item.totalBytes < settings.minSizeKB * 1024) return;

  // Pause first so the browser doesn't keep downloading while we ask the app.
  await chrome.downloads.pause(item.id).catch(() => {});
  try {
    await sendToApp({ url, referrer: item.referrer, filename: item.filename });
    await chrome.downloads.cancel(item.id).catch(() => {});
    await chrome.downloads.erase({ id: item.id }).catch(() => {});
    await log(`#${item.id} handed to SwiftDL`);
    flashBadge("✓", "#1f9d55");
  } catch (err) {
    await log(`#${item.id} left to the browser: ${err?.message || err}`);
    await chrome.downloads.resume(item.id).catch(() => {});
    flashBadge("!", "#d23f3f");
  }
});

// Right-click "Download with SwiftDL" on links and media.
function createMenus() {
  chrome.contextMenus.removeAll(() => {
    chrome.contextMenus.create({ id: "swiftdl-link", title: "Download with SwiftDL", contexts: ["link"] });
    chrome.contextMenus.create({ id: "swiftdl-media", title: "Download with SwiftDL", contexts: ["video", "audio", "image"] });
  });
}
chrome.runtime.onInstalled.addListener(createMenus);
chrome.runtime.onStartup.addListener(createMenus);

chrome.contextMenus.onClicked.addListener(async (info, tab) => {
  const url = info.linkUrl || info.srcUrl;
  if (!url || !/^https?:/i.test(url)) {
    flashBadge("!", "#d23f3f");
    return;
  }
  try {
    await sendToApp({ url, referrer: info.pageUrl || tab?.url });
    flashBadge("✓", "#1f9d55");
  } catch (err) {
    console.warn("SwiftDL: could not reach the app:", err);
    flashBadge("!", "#d23f3f");
  }
});

// The popup asks whether the app is reachable.
chrome.runtime.onMessage.addListener((msg, _sender, respond) => {
  if (msg?.type === "ping") {
    chrome.runtime
      .sendNativeMessage(HOST, { type: "ping" })
      .then((r) => respond({ ok: !!r?.ok, version: r?.version }))
      .catch((e) => respond({ ok: false, error: String(e?.message || e) }));
    return true; // async response
  }
});
