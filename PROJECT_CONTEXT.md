# Sift — Project Context

I want to build a project called **Sift**.

Sift is a system for consuming long-form internet content through AI-generated summaries rather than requiring me to watch hours of video or streams.

The long-term idea is to ingest content from sources such as Twitch and YouTube, collect not only the video/audio but eventually associated information such as Twitch chat and YouTube comments, transcribe the content, and use AI to produce useful summaries and other derived information.

Eventually I want to be able to do things like:

* Queue Twitch VODs and YouTube videos for processing.
* Transcribe long-form audio/video.
* Produce summaries at different levels of detail.
* Correlate transcripts with Twitch chat or YouTube comments.
* Search across previously processed content.
* Ask questions across multiple videos or streams.
* Compare what different creators have said about a topic.
* Submit content from a browser extension.
* Consume processed content from a web UI or iOS app.

However, **do not design or build those future features yet.**

## Learning goal

Sift is also intentionally a distributed-systems learning project.

I have three inexpensive x86 homelab servers that will eventually run Sift services, a NAS for storage, and an NVIDIA DGX Spark that can perform GPU-heavy workloads such as transcription and LLM inference.

I specifically want to use Sift to learn distributed systems by encountering scaling and reliability problems naturally and solving them myself.

Areas I eventually want the project to force me to explore include:

* work distribution
* queues
* worker coordination
* leases
* retries
* idempotency
* backpressure
* service discovery
* shared state
* scheduling
* heterogeneous workers/resources
* partial failures
* network partitions
* observability
* horizontal scaling
* deployment and operations

For that reason, **do not prematurely introduce infrastructure that solves these problems for me**. In particular, don't start by introducing Kubernetes, Kafka, RabbitMQ, Temporal, or similar systems unless I explicitly decide to experiment with them later.

I want to build naive versions, encounter their limitations, reason about those limitations, and evolve the architecture.

The interesting Rust and distributed-systems code is something I want to understand and implement myself. AI assistance should help me reason, investigate, review, and iterate rather than simply generating the entire core system.

Peripheral product code such as browser extensions, dashboards, or mobile apps can eventually be generated much more aggressively because those are not the primary learning objective.

## Repository strategy

Start Sift as a **single repository**.

Do not prematurely split it into microservices or separate repositories.

The architecture may eventually evolve into independently deployable components with names such as:

* `sift-api`
* `sift-worker`
* `sift-scheduler`
* `sift-ingest`
* `sift-transcribe`
* `sift-summarize`
* `sift-cli`

Those names describe possible future boundaries, not the architecture we should build now.

Prefer a simple structure that can evolve toward those boundaries if real requirements eventually justify them.

## MVP

The first MVP has exactly one meaningful goal:

> Submit a Twitch VOD URL and eventually retrieve its transcript and AI-generated summary.

Conceptually:

```text
Twitch VOD URL
      |
      v
   submit
      |
      v
download VOD
      |
      v
extract audio
      |
      v
 transcribe
      |
      v
 summarize
      |
      v
store result
      |
      v
retrieve result
```

For now this can run as a **single-node sequential pipeline**.

Distribution is explicitly NOT part of the first MVP.

A minimal API could look approximately like:

```text
POST /content
{
    "url": "https://twitch.tv/videos/..."
}
```

returning:

```json
{
    "id": "abc123",
    "status": "queued"
}
```

and:

```text
GET /content/:id
```

eventually returning something conceptually like:

```json
{
    "id": "abc123",
    "status": "complete",
    "title": "...",
    "duration": 10842,
    "summary": "...",
    "transcript": "..."
}
```

The exact schema is not fixed. We should discover it while implementing the system.

## MVP non-goals

Do NOT add these yet:

* distributed workers
* Kubernetes
* sophisticated job queues
* Twitch chat ingestion
* YouTube support
* YouTube comments
* browser extension
* web dashboard
* iOS application
* embeddings
* semantic search
* multi-video analysis
* authentication
* elaborate deployment infrastructure

Capture worthwhile ideas as future work rather than expanding the MVP.

## First task

There is older code from a previous attempt at downloading Twitch VODs and transcribing them.

Before designing much new architecture:

1. Inspect the existing repository.
2. Determine what functionality already exists and still works.
3. Explain the current architecture to me.
4. Identify the smallest path from the current implementation to the MVP above.
5. Propose a short implementation plan.

Do **not** immediately implement a large architecture or rewrite the project.

I want to establish a working vertical slice first, then evolve Sift as actual problems emerge.

