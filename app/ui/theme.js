// Light or dark: "system" follows the operating system, "light" and "dark"
// override it (Settings > General > Theme). Runs in <head>, before the page
// draws, using the last choice remembered on this machine, so a window never
// flashes the wrong theme; the app then confirms it from its settings.
(() => {
  const media = matchMedia("(prefers-color-scheme: dark)");
  let pref = "system";
  try { pref = localStorage.getItem("theme") || "system"; } catch {}
  const apply = () => {
    const dark = pref === "dark" || (pref === "system" && media.matches);
    document.documentElement.dataset.theme = dark ? "dark" : "light";
  };
  media.addEventListener("change", apply);
  window.setTheme = (next) => {
    pref = ["light", "dark"].includes(next) ? next : "system";
    try { localStorage.setItem("theme", pref); } catch {}
    apply();
  };
  apply();
})();
