# `sift-worker` Runtime Layout

Issue: #10

This document records the MVP runtime layout for installing `sift-worker` on Sift nodes. The goal is a concrete, boring layout that is easy to inspect over SSH and sufficient for the upcoming systemd install work.

## Decisions

For the MVP, run each worker as a dedicated system user:

```text
sift
```

Install the worker binary at:

```text
/usr/local/bin/sift-worker
```

Keep optional host-local worker configuration at:

```text
/etc/sift/sift-worker.env
```

Store per-job JSON state, ingestion metadata, media downloads, and other worker artifacts under:

```text
/var/lib/sift/jobs
```

Rely on journald for MVP log access:

```sh
journalctl -u sift-worker.service
```

Use the same layout on all Sift worker nodes immediately. Host-specific values, such as `SIFT_WORKER_ID`, belong in each node's `/etc/sift/sift-worker.env` file.

## Rationale

The dedicated `sift` user keeps the service separate from personal login users without adding deployment complexity.

`/usr/local/bin/sift-worker` matches a manually installed binary that is not owned by the OS package manager.

`/etc/sift/sift-worker.env` gives the systemd unit a stable place to load simple environment-based configuration while avoiding a custom config format for the MVP.

`/var/lib/sift/jobs` is the correct host-local location for mutable service state. It keeps job state out of `/tmp`, survives reboots, and avoids shared storage assumptions while the MVP is still single-node per worker.

Journald is enough for current logs because the worker already emits structured tracing events to stdout/stderr. File logs, log shipping, and metrics can wait until the project has operational pressure that justifies them.

Using the same layout everywhere prevents one-off node drift while still allowing each node to have local state. This does not imply shared storage, scheduler coordination, or distributed workers yet.

## Expected Environment

The systemd install should provide at least:

```text
SIFT_WORKER_ID=<node-name>
SIFT_JOBS_DIR=/var/lib/sift/jobs
SIFT_WORKER_LISTEN_ADDR=0.0.0.0:7387
SIFT_INGESTION_CONCURRENCY=1
SIFT_INGESTION_QUEUE_CAPACITY=50
```

Optional environment values:

```text
SIFT_YT_DLP=/usr/bin/yt-dlp
```

If `SIFT_YT_DLP` is omitted, `sift-worker` expects `yt-dlp` to be available on `PATH`.

`SIFT_WORKER_LISTEN_ADDR` defaults to `127.0.0.1:7387` for local development. Installed node services should set it to `0.0.0.0:7387` or a specific LAN interface address so the worker can accept submissions from the LAN.

`SIFT_INGESTION_CONCURRENCY` defaults to `1`. It limits how many ingestion jobs on this worker may own download capacity at the same time. Jobs that have been accepted but are waiting for capacity are persisted with `"state": "waiting"`; jobs that own capacity and are running the media download are persisted with `"state": "downloading"`.

## Ingestion Queue and Backpressure

The worker has a bounded in-memory waiting queue, defaulting to 50 jobs per worker. `SIFT_INGESTION_QUEUE_CAPACITY` must be at least 1. A single dispatcher waits for an available ingestion permit before removing the next job, so queued jobs are distinct from active downloads. `SIFT_INGESTION_CONCURRENCY` independently limits active ingestions (default 1; increase to 3 for a worker capable of three simultaneous downloads).

The HTTP handler reserves queue capacity before persisting a job. A full or unavailable queue returns HTTP 503 with `Retry-After: 120` (seconds), without writing a job record. On acceptance the worker persists `waiting` state before enqueueing and responds HTTP 202. Retry-After is advisory and does not guarantee capacity when the client retries. The CLI does not yet automatically retry.

**Durability boundary:** Queue entries are in memory; the existing per-job JSON is persistent, but this version does not replay it on startup. A crash can strand an accepted `waiting` or `downloading` job. Recovery is tracked in #25, while graceful draining and task ownership are tracked in #22 and #21. No durable delivery guarantee is made yet. A failed ingestion currently remains in `downloading` state; failure-state handling remains follow-up work.

## Directory Ownership

The install process should create the user and directories with ownership equivalent to:

```sh
sudo useradd --system --home-dir /var/lib/sift --shell /usr/bin/nologin sift
sudo install -d -o sift -g sift -m 0750 /var/lib/sift/jobs
sudo install -d -o root -g root -m 0755 /etc/sift
```

The worker binary should be owned by root and executable by the service user:

```sh
sudo install -o root -g root -m 0755 sift-worker /usr/local/bin/sift-worker
```

## Current Non-Goals

Do not add these as part of the MVP layout:

- shared NAS-backed job storage
- object storage
- a scheduler-owned artifact namespace
- per-worker subdirectories under a shared root
- log files outside journald
- package-manager integration
- Docker or Kubernetes runtime layout
