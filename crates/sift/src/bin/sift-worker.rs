use std::path::PathBuf;

use axum::{
    Json, Router,
    extract::State,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use sift::{JobMetadata, SubmitJobRequest, SubmitJobResponse, error_response, generate_job_id};
use tokio::{sync::{Semaphore, mpsc}, task::JoinSet};

#[derive(Clone)]
struct WorkerState {
    jobs_root: PathBuf,
    worker: String,
    queue: mpsc::Sender<JobMetadata>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    sift::init_tracing();

    let ingestion_concurrency_limit = sift::ingestion_concurrency_from_env()?;
    let queue_capacity = sift::ingestion_queue_capacity_from_env()?;
    let (queue, receiver) = mpsc::channel(queue_capacity);
    let state = WorkerState {
        jobs_root: sift::jobs_dir_from_env(),
        worker: std::env::var("SIFT_WORKER_ID").unwrap_or_else(|_| "local-worker".to_string()),
        queue,
    };
    tokio::spawn(dispatch_jobs(
        receiver,
        state.jobs_root.clone(),
        ingestion_concurrency_limit,
    ));

    let addr = sift::worker_listen_addr_from_env()?;
    let app = Router::new()
        .route("/jobs", post(submit_job))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!(%addr, ingestion_concurrency_limit, queue_capacity, "sift-worker started");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(%error, "failed to listen for shutdown signal");
            }
        })
        .await?;

    // Task ownership and graceful draining are tracked in #21 and #22.
    tracing::info!("sift-worker stopping");
    Ok(())
}

/// One dispatcher owns dequeueing. It waits for an execution slot BEFORE
/// removing a job from the queue, so queue capacity means waiting jobs.
async fn dispatch_jobs(
    mut receiver: mpsc::Receiver<JobMetadata>,
    jobs_root: PathBuf,
    concurrency: usize,
) {
    let capacity = std::sync::Arc::new(Semaphore::new(concurrency));
    let mut tasks: JoinSet<(String, Result<JobMetadata, String>)> = JoinSet::new();
    let mut accepting = true;
    loop {
        // Reap completions even when the concurrency limit is saturated.
        tokio::select! {
            completed = tasks.join_next(), if !tasks.is_empty() => {
                if let Some(completed) = completed {
                    observe_ingestion_completion(completed);
                }
            }
            permit = capacity.clone().acquire_owned(), if accepting => {
                let Ok(permit) = permit else { break };
                match receiver.recv().await {
                    Some(metadata) => {
                        let root = jobs_root.clone();
                        tasks.spawn(async move {
                            let _permit = permit;
                            let job_id = metadata.id.0.clone();
                            let result = sift::begin_twitch_ingestion(root, metadata).await;
                            (job_id, result)
                        });
                    }
                    None => accepting = false,
                }
            }
        }
        if !accepting && tasks.is_empty() {
            break;
        }
    }
    // The dispatcher owns all ingestion tasks. Draining on queue closure is
    // intentional; signal-driven graceful shutdown is a separate concern (#22).
    while let Some(completed) = tasks.join_next().await {
        observe_ingestion_completion(completed);
    }
}

fn observe_ingestion_completion(
    completed: Result<(String, Result<JobMetadata, String>), tokio::task::JoinError>,
) {
    match completed {
        Ok((_, Ok(metadata))) => tracing::info!(
            job_id = %metadata.id.0,
            state = %metadata.state.as_str(),
            "job completed initial Twitch media acquisition"
        ),
        Ok((job_id, Err(message))) => {
            tracing::error!(%job_id, %message, "ingestion failed");
        }
        Err(error) if error.is_panic() => {
            tracing::error!(%error, "ingestion task panicked");
        }
        Err(error) => {
            tracing::warn!(%error, "ingestion task cancelled");
        }
    }
}

async fn submit_job(
    State(state): State<WorkerState>,
    Json(request): Json<SubmitJobRequest>,
) -> Response {
    if let Err(message) = sift::validate_twitch_vod_url(&request.url) {
        return (StatusCode::BAD_REQUEST, Json(error_response("invalid_url", message)))
            .into_response();
    }

    // Reserve capacity before writing anything. A full queue causes no
    // accepted job record and no unbounded task creation.
    let permit = match state.queue.try_reserve() {
        Ok(permit) => permit,
        Err(mpsc::error::TrySendError::Full(_)) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "120")],
                Json(error_response("queue_full", "ingestion queue is full; retry later")),
            ).into_response();
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(header::RETRY_AFTER, "120")],
                Json(error_response("worker_unavailable", "ingestion dispatcher is unavailable")),
            ).into_response();
        }
    };

    let metadata = match sift::persist_initial_job_state(
        &state.jobs_root,
        generate_job_id(),
        request.url,
        state.worker.clone(),
        request.full_download,
    ) {
        Ok(metadata) => metadata,
        Err(message) => {
            tracing::error!(%message, "failed to persist accepted job state");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_response("state_persistence_failed", message)),
            ).into_response();
        }
    };
    let metadata = match sift::transition_job_state(
        &state.jobs_root,
        metadata,
        sift::JobStatus::Waiting,
    ) {
        Ok(metadata) => metadata,
        Err(message) => {
            tracing::error!(%message, "failed to persist waiting job state");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(error_response("state_persistence_failed", message)),
            ).into_response();
        }
    };

    tracing::info!(job_id = %metadata.id.0, "accepted queued ingestion job");
    let response = SubmitJobResponse {
        id: metadata.id.clone(),
        status: metadata.state.clone(),
        worker: metadata.worker.clone(),
    };
    permit.send(metadata);
    (StatusCode::ACCEPTED, Json(response)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn queue_rejects_at_capacity_and_recovers_after_dequeue() {
        let (sender, mut receiver) = mpsc::channel::<u32>(2);
        sender.try_send(1).unwrap();
        sender.try_send(2).unwrap();
        assert!(matches!(sender.try_send(3), Err(mpsc::error::TrySendError::Full(3))));
        assert_eq!(receiver.recv().await, Some(1));
        sender.try_send(3).unwrap();
        assert_eq!(receiver.recv().await, Some(2));
        assert_eq!(receiver.recv().await, Some(3));
    }

    #[tokio::test]
    async fn full_queue_returns_503_without_persisting_job() {
        let root = std::env::temp_dir().join(format!("sift-queue-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (sender, _receiver) = mpsc::channel(1);
        sender.try_send(JobMetadata {
            id: sift::JobId("existing".into()),
            source_url: "https://www.twitch.tv/videos/123".into(),
            twitch_vod_id: None,
            worker: "test".into(),
            full_download: false,
            created_at_ms: 0,
            updated_at_ms: 0,
            state: sift::JobStatus::Waiting,
        }).unwrap();
        let response = submit_job(
            State(WorkerState { jobs_root: root.clone(), worker: "test".into(), queue: sender }),
            Json(SubmitJobRequest { url: "https://www.twitch.tv/videos/456".into(), full_download: false }),
        ).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers().get(header::RETRY_AFTER).unwrap(), "120");
        assert!(!root.exists());
    }

    #[tokio::test]
    async fn accepted_job_is_persisted_as_waiting_and_enqueued() {
        let root = std::env::temp_dir().join(format!("sift-accepted-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (sender, mut receiver) = mpsc::channel(1);
        let response = submit_job(
            State(WorkerState { jobs_root: root.clone(), worker: "test".into(), queue: sender }),
            Json(SubmitJobRequest { url: "https://www.twitch.tv/videos/456".into(), full_download: false }),
        ).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        let job = receiver.recv().await.unwrap();
        assert_eq!(job.state, sift::JobStatus::Waiting);
        let persisted = fs::read_to_string(root.join(&job.id.0).join("state.json")).unwrap();
        assert!(persisted.contains("\"state\": \"waiting\""));
        fs::remove_dir_all(root).unwrap();
    }
}
