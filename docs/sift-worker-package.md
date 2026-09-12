# Deploy `sift-worker` to x86_64 Linux

Issue: #9

## Build the worker

For the MVP, cross-build `sift-worker` from macOS with the Zig-backed Cargo path:

```sh
cargo zigbuild --release --bin sift-worker --target x86_64-unknown-linux-gnu
```

This keeps packaging deliberately simple: no Dockerfile, package manager, systemd unit, CI release flow, or deployment tool yet. The command produces a Linux x86_64 binary that can be copied to a Sift node and run manually.

A plain Cargo cross-build from macOS is not enough on this machine because dependencies such as `ring` require a Linux target C compiler and `x86_64-linux-gnu-gcc` is not installed.

The deployable worker binary is written to:

```text
target/x86_64-unknown-linux-gnu/release/sift-worker
```

Local artifact inspection:

```text
ELF 64-bit LSB pie executable, x86-64, dynamically linked, interpreter /lib64/ld-linux-x86-64.so.2, for GNU/Linux 2.0.0
```

## Copy the artifact

Copy the worker binary to `sift-01`:

```sh
scp target/x86_64-unknown-linux-gnu/release/sift-worker cody@sift-01:/tmp/sift-worker
```

Make it executable on the node:

```sh
ssh cody@sift-01 'chmod +x /tmp/sift-worker'
```

## Install runtime dependencies

`sift-01` runs Arch Linux. Install the current worker media dependencies with:

```sh
ssh cody@sift-01 'sudo pacman -Sy --needed --noconfirm yt-dlp ffmpeg'
```

Required for worker deployment:

- `yt-dlp`, available on `PATH`, or set `SIFT_YT_DLP` to an executable path.
- `ffmpeg`, because the current bounded debug download path passes `--force-keyframes-at-cuts` to `yt-dlp`.
- A writable jobs directory, configured with `SIFT_JOBS_DIR` or defaulting to `sift-jobs` under the current working directory.

Verified on `sift-01`:

```text
/usr/bin/yt-dlp
2026.08.19
/usr/bin/ffmpeg
ffmpeg version n9.0.1
```

## Run manually

Start the worker on `sift-01`:

```sh
ssh cody@sift-01 'mkdir -p /tmp/sift-jobs && SIFT_WORKER_ID=sift-01 SIFT_JOBS_DIR=/tmp/sift-jobs /tmp/sift-worker'
```

The worker listens on `127.0.0.1:7387` and accepts job submissions at `POST /jobs`.

For a persistent host-local jobs directory later, use something like:

```sh
SIFT_WORKER_ID=sift-01 SIFT_JOBS_DIR=/var/lib/sift/jobs /path/to/sift-worker
```

## Smoke test

In another shell, submit a local job on `sift-01`:

```sh
ssh cody@sift-01 'curl -i \
  -H '\''content-type: application/json'\'' \
  -d '\''{"url":"https://www.twitch.tv/videos/123456789","full_download":false}'\'' \
  http://127.0.0.1:7387/jobs'
```

Expected result:

- HTTP status is `202 Accepted`.
- Response contains an id, `"status":"accepted"`, and `"worker":"sift-01"`.
- `/tmp/sift-jobs/<job-id>/state.json` exists.

## On-node fallback build

If cross-building from macOS is unavailable, build from a checkout of this repository on `sift-01`:

```sh
cargo build --release --bin sift-worker
```

The deployable binary is written to:

```text
target/release/sift-worker
```

Useful for building on-node:

- Rust toolchain with Cargo.
- System C build tools.

## Verification notes

Verified from this macOS workspace:

```sh
cargo test
cargo build --release --bin sift-worker
cargo zigbuild --release --bin sift-worker --target x86_64-unknown-linux-gnu
file target/x86_64-unknown-linux-gnu/release/sift-worker
```

The native macOS release build succeeds, and the Zig-backed cross-build produces an x86-64 GNU/Linux ELF binary.

Verified on `sift-01` by copying the cross-built binary to `/tmp/sift-worker`, installing `yt-dlp` and `ffmpeg`, and running:

```sh
SIFT_WORKER_ID=sift-01 SIFT_JOBS_DIR=/tmp/sift-jobs /tmp/sift-worker
```

A local `POST /jobs` returned:

```text
HTTP/1.1 202 Accepted
{"id":"job_1789240248097_0","status":"accepted","worker":"sift-01"}
```

The worker persisted `/tmp/sift-jobs/job_1789240248097_0/state.json`, completed initial Twitch media acquisition, and reached the `downloaded` state. It wrote a media artifact at `/tmp/sift-jobs/job_1789240248097_0/media/vod-v123456789.mp4`.

The first smoke test exposed missing media dependencies. After installing `yt-dlp` and `ffmpeg`, both binaries are present on `PATH`, and the worker can complete the current bounded media acquisition path on `sift-01`.
