// Renders launch.html to MP4: the page exposes seek(t), which draws the
// frame at time t; this steps through every frame in headless Chrome and
// pipes screenshots into ffmpeg. Deterministic, so every render is identical.
//
//   node render.mjs                 full video -> out/swiftdl-launch.mp4
//   node render.mjs --still 2,9.5   single frames -> out/still-2.png, ...
//   node render.mjs --vertical      9:16 cut for Reels/Shorts (same timeline)

import { spawn } from "node:child_process";
import { existsSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import puppeteer from "puppeteer-core";

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const flag = (name) => args.includes(name);
const value = (name) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : undefined; };

const FPS = 60;
const vertical = flag("--vertical");
const [W, H] = vertical ? [1080, 1920] : [1920, 1080];
const out = join(here, "out");
mkdirSync(out, { recursive: true });

const BROWSERS = [
  process.env.CHROME,
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  "/usr/bin/google-chrome",
  "/usr/bin/chromium",
].filter(Boolean);
const FFMPEG = process.env.FFMPEG || [
  join(process.env.APPDATA || "", "com.swiftdl.app/tools/ffmpeg/ffmpeg.exe"),
].find(existsSync) || "ffmpeg";

const browser = await puppeteer.launch({
  executablePath: BROWSERS.find(existsSync),
  headless: true,
  args: ["--hide-scrollbars", "--force-color-profile=srgb", "--font-render-hinting=none"],
});
const page = await browser.newPage();
await page.setViewport({ width: W, height: H, deviceScaleFactor: 1 });
await page.goto(pathToFileURL(resolve(here, "launch.html")).href + (vertical ? "?vertical" : ""), { waitUntil: "networkidle0" });
await page.evaluate(() => document.fonts.ready);
const duration = await page.evaluate(() => window.DURATION);

const shoot = (t, opts) => page.evaluate((t) => window.seek(t), t).then(() => page.screenshot(opts));

if (flag("--still")) {
  for (const t of (value("--still") || "1").split(",").map(Number)) {
    const path = join(out, `still-${t}${vertical ? "-v" : ""}.png`);
    await shoot(t, { path, type: "png" });
    console.log(path);
  }
} else {
  const file = join(out, vertical ? "swiftdl-launch-vertical.mp4" : "swiftdl-launch.mp4");
  const ff = spawn(FFMPEG, [
    "-y", "-loglevel", "error", "-f", "image2pipe", "-framerate", String(FPS), "-c:v", "mjpeg", "-i", "-",
    // JPEG frames are full-range; convert to the standard (TV) range players
    // and social sites expect, or blacks look washed out on some of them.
    "-vf", "scale=in_range=pc:out_range=tv:out_color_matrix=bt709,format=yuv420p",
    "-c:v", "libx264", "-preset", "slow", "-crf", "16", "-color_range", "tv", "-colorspace", "bt709",
    "-color_primaries", "bt709", "-color_trc", "bt709", "-movflags", "+faststart", file,
  ], { stdio: ["pipe", "inherit", "inherit"] });
  const frames = Math.round(duration * FPS);
  const started = Date.now();
  for (let f = 0; f < frames; f++) {
    const jpg = await shoot(f / FPS, { type: "jpeg", quality: 95 });
    if (!ff.stdin.write(jpg)) await new Promise((r) => ff.stdin.once("drain", r));
    if (f % FPS === 0) process.stdout.write(`\r${Math.round((f / frames) * 100)}%  ${((Date.now() - started) / 1000).toFixed(0)}s`);
  }
  ff.stdin.end();
  await new Promise((r, j) => ff.on("close", (c) => (c === 0 ? r() : j(new Error(`ffmpeg exited ${c}`)))));
  console.log(`\r${file}`);
}
await browser.close();
