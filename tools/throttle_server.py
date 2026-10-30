"""Local test server that limits speed *per connection*, like many real
servers and CDNs do. Lets you measure what multiple connections buy you and
test pause/resume without depending on the internet.

    python tools/throttle_server.py [--size-mb 64] [--rate-mb 2] [--port 8765]

Serves http://127.0.0.1:8765/test.bin (random bytes, fixed seed) with Range
support, and prints its SHA-256 so you can verify downloads.
"""

import argparse
import hashlib
import random
import re
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

CHUNK = 64 * 1024


def make_handler(data: bytes, rate: int):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *args):
            pass

        def do_GET(self):
            if self.path == "/":
                # A page with a link, for testing browser-extension capture.
                page = b'<!doctype html><title>SwiftDL test</title><h1>SwiftDL test page</h1>' \
                       b'<p><a id="dl" href="/test.bin" style="font-size:40px">Download test.bin</a></p>'
                self.send_response(200)
                self.send_header("Content-Type", "text/html")
                self.send_header("Content-Length", str(len(page)))
                self.end_headers()
                self.wfile.write(page)
                return
            if self.path == "/video":
                # A page with a video player, for testing the extension's
                # "Download video" button and media detection.
                page = b'<!doctype html><title>Sample Clip 4K</title><h1>Video test page</h1>' \
                       b'<video src="/video.mp4" width="640" height="360" controls style="background:#000"></video>' \
                       b'<p><a id="dl" href="/test.bin">Download test.bin</a></p>'
                self.send_response(200)
                self.send_header("Content-Type", "text/html")
                self.send_header("Content-Length", str(len(page)))
                self.end_headers()
                self.wfile.write(page)
                return
            if self.path not in ("/test.bin", "/video.mp4"):
                self.send_error(404)
                return
            is_video = self.path == "/video.mp4"
            size = len(data)
            start, end = 0, size - 1
            m = re.fullmatch(r"bytes=(\d+)-(\d*)", self.headers.get("Range", ""))
            if m:
                start = int(m.group(1))
                end = min(int(m.group(2)), size - 1) if m.group(2) else size - 1
                self.send_response(206)
                self.send_header("Content-Range", f"bytes {start}-{end}/{size}")
            else:
                self.send_response(200)
            if is_video:
                self.send_header("Content-Type", "video/mp4")
            else:
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Disposition", 'attachment; filename="test.bin"')
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", str(end - start + 1))
            self.send_header("ETag", '"test-v1"')
            self.end_headers()

            began, sent, pos = time.monotonic(), 0, start
            try:
                while pos <= end:
                    n = min(CHUNK, end - pos + 1)
                    self.wfile.write(data[pos : pos + n])
                    pos += n
                    sent += n
                    ahead = sent / rate - (time.monotonic() - began)
                    if ahead > 0:
                        time.sleep(ahead)
            except (ConnectionResetError, BrokenPipeError, ConnectionAbortedError):
                pass  # client hung up (e.g. its segment was stolen); that's normal

    return Handler


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--size-mb", type=int, default=64)
    ap.add_argument("--rate-mb", type=float, default=2.0, help="per-connection limit, MB/s")
    ap.add_argument("--port", type=int, default=8765)
    a = ap.parse_args()

    data = random.Random(42).randbytes(a.size_mb * 1024 * 1024)
    print(f"sha256 {hashlib.sha256(data).hexdigest()}", flush=True)
    print(f"serving http://127.0.0.1:{a.port}/test.bin  ({a.size_mb} MiB, {a.rate_mb} MB/s per connection)", flush=True)
    ThreadingHTTPServer(("127.0.0.1", a.port), make_handler(data, int(a.rate_mb * 1024 * 1024))).serve_forever()


if __name__ == "__main__":
    main()
