const DEFAULTS = { enabled: true, minSizeKB: 0 };
const $ = (id) => document.getElementById(id);

chrome.storage.sync.get(DEFAULTS).then((s) => {
  $("enabled").checked = s.enabled;
  $("minSize").value = s.minSizeKB;
});
$("enabled").addEventListener("change", (e) => chrome.storage.sync.set({ enabled: e.target.checked }));
$("minSize").addEventListener("change", (e) => {
  const v = Math.max(0, Number.parseInt(e.target.value, 10) || 0);
  e.target.value = v;
  chrome.storage.sync.set({ minSizeKB: v });
});

chrome.runtime.sendMessage({ type: "ping" }).then((r) => {
  $("dot").className = `dot ${r?.ok ? "ok" : "err"}`;
  $("status").textContent = r?.ok ? `Connected to SwiftDL ${r.version}` : "SwiftDL app not found";
  $("help").hidden = !!r?.ok;
});
