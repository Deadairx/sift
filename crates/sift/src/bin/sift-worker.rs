use std::{path::PathBuf, sync::Arc};

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use sift::{SubmitJobRequest, SubmitJobResponse, error_response, generate_job_id};
use tokio::sync::Semaphore;

#[derive(Clone)]
struct WorkerState {
    jobs_root: PathBuf,
    worker: String,
    ingestion_capacity: Arc<Semaphore>,
    ingestion_concurrency_limit: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    sift::init_tracing();

    let ingestion_concurrency_limit = sift::ingestion_concurrency_from_env()?;
    let state = WorkerState {
        jobs_root: sift::jobs_dir_from_env(),
        worker: std::env::var("SIFT_WORKER_ID").unwrap_or_else(|_| "local-worker".to_string()),
        ingestion_capacity: Arc::new(Semaphore::new(ingestion_concurrency_limit)),
        ingestion_concurrency_limit,
    };

    let addr = sift::worker_listen_addr_from_env()?;
    let app = Router::new()
        .route("/jobs", post(submit_job))
        .with_state(state.clone());
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!(%addr, ingestion_concurrency_limit = state.ingestion_concurrency_limit, "sift-worker started");

    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(%error, "failed to listen for shutdown signal");
            }
        })
        .await?;

    tracing::info!("sift-worker stopping");
    Ok(())
}

async fn submit_job(
    State(state): State<WorkerState>,
    Json(request): Json<SubmitJobRequest>,
) -> Result<(StatusCode, Json<SubmitJobResponse>), (StatusCode, Json<sift::ErrorResponse>)> {
    if let Err(message) = sift::validate_twitch_vod_url(&request.url) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(error_response("invalid_url", message)),
        ));
    }

    let id = generate_job_id();
    let worker = state.worker.clone();
    let jobs_root = state.jobs_root.clone();
    let metadata =
        sift::persist_initial_job_state(&jobs_root, id, request.url, worker, request.full_download)
            .map_err(|message| {
                tracing::error!(%message, "failed to persist accepted job state");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(error_response("state_persistence_failed", message)),
                )
            })?;

    tracing::info!(
        job_id = %metadata.id.0,
        url = %metadata.source_url,
        worker = %metadata.worker,
        from_state = "new",
        to_state = %metadata.state.as_str(),
        "job state transition"
    );
    tracing::info!(job_id = %metadata.id.0, url = %metadata.source_url, worker = %metadata.worker, state = %metadata.state.as_str(), "accepted job submission");

    let ingestion_metadata = metadata.clone();
    tokio::spawn(async move {
        let job_id = ingestion_metadata.id.0.clone();
        let url = ingestion_metadata.source_url.clone();
        let worker = ingestion_metadata.worker.clone();

        let ingestion_metadata = match sift::transition_job_state(
            &jobs_root,
            ingestion_metadata,
            sift::JobStatus::Waiting,
        ) {
            Ok(metadata) => metadata,
            Err(message) => {
                tracing::error!(%job_id, %url, %worker, %message, "failed to persist waiting job state");
                return;
            }
        };

        tracing::info!(
            job_id = %job_id,
            url = %url,
            worker = %worker,
            from_state = "accepted",
            to_state = %ingestion_metadata.state.as_str(),
            "job state transition"
        );
        tracing::info!(
            job_id = %job_id,
            concurrency_limit = state.ingestion_concurrency_limit,
            available_permits = state.ingestion_capacity.available_permits(),
            "job waiting for ingestion capacity"
        );

        // Hold the permit for the ingestion work; dropping it releases capacity.
        // `let _ = ...` would drop the permit immediately.
        let _permit = match state.ingestion_capacity.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => {
                tracing::error!(%job_id, %url, %worker, "ingestion capacity semaphore closed");
                return;
            }
        };

        tracing::info!(
            job_id = %job_id,
            concurrency_limit = state.ingestion_concurrency_limit,
            available_permits = state.ingestion_capacity.available_permits(),
            "job acquired ingestion capacity"
        );

        match sift::begin_twitch_ingestion(jobs_root, ingestion_metadata).await {
            Ok(metadata) => tracing::info!(
                job_id = %metadata.id.0,
                url = %metadata.source_url,
                worker = %metadata.worker,
                to_state = %metadata.state.as_str(),
                twitch_vod_id = metadata.twitch_vod_id.as_deref().unwrap_or(""),
                "job completed initial Twitch media acquisition"
            ),
            Err(message) => {
                tracing::error!(%job_id, %url, %worker, %message, "failed to begin Twitch ingestion")
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(SubmitJobResponse {
            id: metadata.id,
            status: metadata.state,
            worker: metadata.worker,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn semaphore_capacity_is_not_available_beyond_limit() {
        let semaphore = Arc::new(Semaphore::new(1));
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("first permit should be available");

        assert!(semaphore.clone().try_acquire_owned().is_err());

        drop(permit);

        assert!(semaphore.try_acquire_owned().is_ok());
    }
}
