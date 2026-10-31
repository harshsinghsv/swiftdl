# Launch video

SwiftDL's 46-second launch film, written as code. It's a screen demo: a
bright desktop where a browser opens, a download goes to SwiftDL and runs
over eight connections, then a YouTube video downloads in 1080p, all seen
through a moving camera. `launch.html` is one timeline whose `seek(t)` draws
the frame at time `t`; `render.mjs` steps through it in headless Chrome and
pipes every frame into ffmpeg.

```bash
npm install
npm run render                 # out/swiftdl-launch.mp4, 1920x1080, 60 fps
node render.mjs --vertical     # out/swiftdl-launch-vertical.mp4, 1080x1920
node render.mjs --still 14,31  # single frames, for checking a scene
```

It uses Chrome (or Edge) and ffmpeg. Set `CHROME` or `FFMPEG` if they aren't
found. The video is silent; add music in any editor.
