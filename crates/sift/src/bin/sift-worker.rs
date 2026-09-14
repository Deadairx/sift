use axum::{Json, Router, http::StatusCode, routing::post};
use sift::{SubmitJobRequest, SubmitJobResponse, error_response, generate_job_id};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    sift::init_tracing();

    let addr = sift::worker_listen_addr_from_env()?;
    let app = Router::new().route("/jobs", post(submit_job));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!(%addr, "sift-worker started");

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
    Json(request): Json<SubmitJobRequest>,
) -> Result<(StatusCode, Json<SubmitJobResponse>), (StatusCode, Json<sift::ErrorResponse>)> {
    if let Err(message) = sift::validate_twitch_vod_url(&request.url) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(error_response("invalid_url", message)),
        ));
    }

    let id = generate_job_id();
    let worker = std::env::var("SIFT_WORKER_ID").unwrap_or_else(|_| "local-worker".to_string());
    let jobs_root = sift::jobs_dir_from_env();
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
