## Sift

Sift is a system for consuming long-form internet content through AI-generated summaries rather than requiring hours of video watching.

The first MVP goal is narrow:

> Submit a Twitch VOD URL and eventually retrieve its transcript and AI-generated summary.

Current project context and constraints live in `PROJECT_CONTEXT.md`.

Deployment notes:

- Deploy `sift-worker` to x86_64 Linux: `docs/sift-worker-package.md`
- Install `sift-worker` with the MVP runtime layout: `docs/sift-worker-runtime-layout.md`
- Systemd unit for installed workers: `deploy/systemd/sift-worker.service`
