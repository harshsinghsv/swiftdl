"""Talk to the app the way the browser extension does, for testing the
native messaging host without a browser.

    python tools/send_native_message.py <path-to-swiftdl-app.exe> ping
    python tools/send_native_message.py <path-to-swiftdl-app.exe> download <url> [referrer]
"""

import json
import struct
import subprocess
import sys

EXTENSION_ORIGIN = "chrome-extension://knmfjbmojepkdiflnckpfjimfmjfcbgh/"


def send(exe: str, message: dict) -> dict:
    # Browsers launch the host with the caller's origin as the argument.
    proc = subprocess.Popen([exe, EXTENSION_ORIGIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    body = json.dumps(message).encode()
    out, _ = proc.communicate(struct.pack("=I", len(body)) + body, timeout=15)
    (length,) = struct.unpack("=I", out[:4])
    return json.loads(out[4 : 4 + length])


def main():
    exe, kind = sys.argv[1], sys.argv[2]
    if kind == "ping":
        msg = {"type": "ping"}
    else:
        msg = {"type": "download", "url": sys.argv[3], "referrer": sys.argv[4] if len(sys.argv) > 4 else None}
    print(json.dumps(send(exe, msg)))


if __name__ == "__main__":
    main()
