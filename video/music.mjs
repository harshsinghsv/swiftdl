// Adds a music track to the rendered film without rendering it again.
// Use a track you have the rights to (your own, licensed, or royalty-free).
//
//   node music.mjs track.mp3                  -> out/swiftdl-launch-music.mp4
//   node music.mjs track.mp3 --from 32        start the track 32 s in (at its drop, say)
//   node music.mjs track.mp3 --volume 0.8     quieter or louder (1 = normalised)
//
// The track fades in, is levelled to streaming loudness (-14 LUFS), and fades
// out with the end card. The picture is copied as is.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const value = (name, d) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : d; };
const track = args.find((a) => !a.startsWith("--") && !args[args.indexOf(a) - 1]?.startsWith("--"));
if (!track || !existsSync(track)) {
  console.error("usage: node music.mjs <track.mp3> [--from seconds] [--volume 1]");
  process.exit(1);
}
const FFMPEG = process.env.FFMPEG || [join(process.env.APPDATA || "", "com.swiftdl.app/tools/ffmpeg/ffmpeg.exe")].find(existsSync) || "ffmpeg";
const video = join(here, "out", "swiftdl-launch.mp4");
const out = join(here, "out", "swiftdl-launch-music.mp4");

const probe = spawnSync(FFMPEG, ["-i", video], { encoding: "utf8" }).stderr;
const [, h, m, s] = /Duration: (\d+):(\d+):([\d.]+)/.exec(probe) || [];
const length = +h * 3600 + +m * 60 + +s;
const fadeOut = 2.5;

const filter = [
  `[1:a]atrim=0:${length},asetpts=PTS-STARTPTS,loudnorm=I=-14:TP=-1.5:LRA=11,aresample=48000,volume=${value("--volume", 1)}`,
  `afade=t=in:d=0.6,afade=t=out:st=${(length - fadeOut).toFixed(2)}:d=${fadeOut}[a]`,
].join(",");
const run = spawnSync(FFMPEG, ["-y", "-loglevel", "error", "-i", video, "-ss", String(value("--from", 0)), "-i", track,
  "-filter_complex", filter, "-map", "0:v", "-map", "[a]", "-c:v", "copy", "-c:a", "aac", "-b:a", "256k", "-ac", "2", "-shortest", "-movflags", "+faststart", out], { stdio: "inherit" });
if (run.status !== 0) process.exit(run.status ?? 1);
console.log(out);
