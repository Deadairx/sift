# Install `sift` CLI on macOS

Issue: #12

This document records the MVP macOS setup for using `sift` as the user-facing entrypoint to the first installed worker.

## Endpoint

The first worker endpoint is:

```text
http://sift-01:7387
```

The `sift` CLI reads the worker endpoint from `SIFT_WORKER`. If `SIFT_WORKER` is not set, it falls back to the local development default `http://127.0.0.1:7387`.

## Install The CLI

From this repository on macOS:

```sh
cargo install --path crates/sift --bin sift
```

Cargo installs the binary to:

```text
$HOME/.cargo/bin/sift
```

Make sure Cargo's bin directory is on `PATH`:

```sh
export PATH="$HOME/.cargo/bin:$PATH"
```

## Configure The Worker Endpoint

For a one-off shell session:

```sh
export SIFT_WORKER=http://sift-01:7387
```

For a persistent zsh setup on this Mac, add the same line to `~/.zshrc` or another shell startup file that is sourced by interactive terminals.

## Verify Hostname Resolution

Verify that macOS can resolve `sift-01` before testing the CLI:

```sh
dscacheutil -q host -a name sift-01
```

Expected result: at least one resolved address for `sift-01`.

## Verify Worker Reachability

The worker exposes job submission at `POST /jobs`. A simple reachability check from macOS is:

```sh
curl -i \
  -H 'content-type: application/json' \
  -d '{"url":"https://www.twitch.tv/videos/123456789","full_download":false}' \
  http://sift-01:7387/jobs
```

Expected result:

- HTTP status is `202 Accepted`.
- Response contains an id, `"status":"accepted"`, and `"worker":"sift-01"`.

## Verify The CLI

Submit through the installed CLI:

```sh
sift https://www.twitch.tv/videos/123456789
```

Expected result:

- The command attempts submission without SSHing into `sift-01` first.
- On success, it prints `submitted job <id> (accepted) to worker sift-01`.
