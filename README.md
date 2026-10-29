# swiftdl

A fast, resumable, multi-connection download manager written in Rust.

## Usage

```bash
swiftdl https://example.com/big.iso                  # 8 connections, saves to ./big.iso
swiftdl https://example.com/big.iso -c 16 -o D:\ISOs # 16 connections, into a folder
swiftdl URL -H "Cookie: session=abc" -H "Referer: https://example.com"
swiftdl URL --restart                                # ignore saved progress
```

Press **Ctrl+C** to pause. Run the same command again to resume. A crash or
network drop is also resumable: progress is saved every second.

## How it works

1. **Probe:** requests `bytes=0-0`. A `206` reply proves range support and
   gives the total size; a `200` falls back to a single stream.
2. **Pre-allocate:** `file.part` is created at full size, so each connection
   writes straight to its final offset. No temp chunks, no join step.
3. **Segment:** the file is split into N ranges, one HTTP/1.1 connection each
   (HTTP/2 is disabled on purpose: it would multiplex all ranges over one
   TCP connection and one congestion window).
4. **Dynamic segmentation:** when a connection finishes, it takes the back half
   of the largest unfinished range. All connections stay busy until the end.
5. **Resume:** `file.swiftdl` (JSON) lists the missing ranges. It is matched
   against URL + size + ETag/Last-Modified before reuse.
6. **Retries:** each segment reconnects from where it stopped, with backoff.

| File | Purpose |
|---|---|
| `src/probe.rs` | Size, range support, file name detection |
| `src/segment.rs` | Byte-range ownership, positioned writes, splitting |
| `src/engine.rs` | Workers, work stealing, saving, progress |
| `src/state.rs` | Resume file |
| `tools/throttle_server.py` | Local per-connection-throttled server for benchmarks |

## Benchmark

Local server limited to 2 MB/s **per connection** (like many real servers), 64 MiB file:

| Connections | Time | Speed |
|---|---|---|
| 1 | 32.0 s | 2.0 MiB/s |
| 8 | 4.0 s | 15.9 MiB/s |
| 16 | 2.6 s | 25.1 MiB/s |

```bash
python tools/throttle_server.py --size-mb 64 --rate-mb 2
swiftdl http://127.0.0.1:8765/test.bin -c 8
```

## Building on Windows

Uses the `x86_64-pc-windows-gnu` toolchain with MinGW-w64 (WinLibs) on PATH:

```bash
cargo build --release
```

## Roadmap

- [ ] Adaptive connection count (add connections while throughput rises)
- [ ] SHA-256 verification (`--sha256`)
- [ ] Download queue + SQLite state, speed limiter, scheduler
- [ ] Local API + Tauri desktop UI
- [ ] Browser extension (MV3) + native messaging host
- [ ] Multi-mirror downloads, HLS/DASH streams
