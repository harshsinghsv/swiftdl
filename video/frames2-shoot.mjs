import puppeteer from "puppeteer-core";
import { serve, findBrowser } from "./server.mjs";
const b = await puppeteer.launch({ executablePath: findBrowser(), headless: true, args: ["--force-color-profile=srgb"] });
const p = await b.newPage(); await p.setViewport({ width: 1920, height: 1080 });
const { server, origin } = await serve();
for (const f of [1, 2, 3]) {
  await p.goto(`${origin}/video/frames2.html?f=${f}`, { waitUntil: "networkidle0" });
  await p.evaluate(() => document.fonts.ready);
  await p.screenshot({ path: `out/apple-${f}.png` });
  console.log("frame", f);
}
await b.close(); server.close();
