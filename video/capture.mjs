// Captures the real web pages the film shows, at 2x, into out/pages/.
// They're third-party pages, so they're captured at render time rather
// than kept in the repository.
//
//   node capture.mjs           capture what's missing
//   node capture.mjs --force   capture again

import { existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import puppeteer from "puppeteer-core";
import { findBrowser } from "./server.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const dir = join(here, "out", "pages");
mkdirSync(dir, { recursive: true });

// Hide cookie banners and sign-in nags for the picture; nothing is accepted.
const HIDE = `
  #cookie-policy-content, .cookie-policy, [class*="cookie"], [id*="cookie"], [class*="consent"], [id*="consent"],
  tp-yt-paper-dialog, ytd-popup-container, #masthead-ad, ytd-banner-promo-renderer, .ytp-ce-element, .ytp-paid-content-overlay,
  yt-mealbar-promo-renderer, #player-ads, .ytp-pause-overlay { display: none !important; }
  /* Other people's videos aren't ours to feature: soften the sidebar. */
  #secondary { filter: blur(14px) saturate(.6); opacity: .55; }`;

const PAGES = [
  { name: "ubuntu", url: "https://ubuntu.com/download/desktop", wait: 2500 },
  {
    name: "youtube", url: "https://www.youtube.com/watch?v=aqz-KE-bpKQ&t=95s", wait: 14000,
    // Let the player show a frame of the film, paused, with controls hidden.
    after: () => {
      const v = document.querySelector("video");
      if (v) { v.muted = true; v.play().catch(() => {}); }
      return new Promise((r) => setTimeout(() => { v?.pause(); document.querySelector(".html5-video-player")?.classList.add("ytp-autohide"); r(); }, 2500));
    },
  },
];

const force = process.argv.includes("--force");
const todo = PAGES.filter((p) => force || !existsSync(join(dir, `${p.name}.png`)));
if (todo.length) {
  const browser = await puppeteer.launch({ executablePath: findBrowser(), headless: true, args: ["--autoplay-policy=no-user-gesture-required", "--lang=en-US"] });
  for (const p of todo) {
    const page = await browser.newPage();
    await page.setUserAgent((await browser.userAgent()).replace("HeadlessChrome", "Chrome"));
    await page.setViewport({ width: 1440, height: 860, deviceScaleFactor: 2 });
    await page.emulateMediaFeatures([{ name: "prefers-color-scheme", value: "light" }]);
    await page.goto(p.url, { waitUntil: "networkidle2", timeout: 60000 }).catch(() => {});
    await page.addStyleTag({ content: HIDE });
    await new Promise((r) => setTimeout(r, p.wait / 2));
    await page.mouse.move(700, 500);
    await page.evaluate(() => { window.scrollTo(0, 1); window.scrollTo(0, 0); });
    await new Promise((r) => setTimeout(r, p.wait / 2));
    if (p.after) await page.evaluate(p.after);
    await page.screenshot({ path: join(dir, `${p.name}.png`) });
    console.log(`captured ${p.name}`);
    const fav = await fetch(`https://www.google.com/s2/favicons?domain=${new URL(p.url).hostname}&sz=64`).catch(() => null);
    if (fav?.ok) (await import("node:fs")).writeFileSync(join(dir, `${p.name}-icon.png`), Buffer.from(await fav.arrayBuffer()));
    await page.close();
  }
  await browser.close();
}
