# Launch video

SwiftDL's launch film (about a minute), made from real material. It opens on
the problem, Chrome downloading Ubuntu on one connection with nearly an hour
left, then shows the same file through SwiftDL, a side-by-side race, a
YouTube download and the app.

- **Real websites.** `capture.mjs` captures the Ubuntu download page and a
  YouTube video page in Chrome at 2x (into `out/pages/`, not committed, since
  they're other people's pages).
- **SwiftDL's real UI.** The download windows and the main window are the app's
  own pages (`app/ui`), running in the preview's video mode (`?video`): the
  film clicks their real buttons and feeds each download's progress frame by
  frame.

`launch.html` frames it all like a screen recording, a Chrome window and the
app on a light wallpaper with a moving camera and cursor, and `render.mjs`
steps through it in headless Chrome and pipes every frame into ffmpeg.

```bash
npm install
npm run render                 # out/swiftdl-launch.mp4, 1920x1080, 60 fps
node render.mjs --still 14,31  # single frames, for checking a scene
node capture.mjs --force       # capture the web pages again
```

It needs Chrome (or Edge) and ffmpeg; set `CHROME` or `FFMPEG` if they aren't
found. The video is silent; add music in any editor.

## Music

The render is silent. To add a track you have the rights to (your own,
licensed, or royalty-free such as the YouTube Audio Library or Pixabay Music):

```bash
node music.mjs track.mp3 --from 32   # start the track at its drop, 32 s in
```

This writes `out/swiftdl-launch-music.mp4`: the track fades in, is levelled to
streaming loudness and fades out with the end card. Good beats to land on:
the SwiftDL logo at 9.7 s, the first SwiftDL window at 16.5 s, the race at
30.7 s, YouTube at 35.5 s and the end card at 51.9 s.
