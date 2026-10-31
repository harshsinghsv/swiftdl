# Launch video

SwiftDL's 41-second launch film, written as code: `launch.html` is one
timeline whose `seek(t)` draws the frame at time `t`, and `render.mjs` steps
through it in headless Chrome and pipes every frame into ffmpeg.

```bash
npm install
npm run render                 # out/swiftdl-launch.mp4, 1920x1080, 60 fps
node render.mjs --vertical     # out/swiftdl-launch-vertical.mp4, 1080x1920
node render.mjs --still 14,31  # single frames, for checking a scene
```

It uses Chrome (or Edge) and ffmpeg. Set `CHROME` or `FFMPEG` if they aren't
found. The video is silent; add music in any editor.
