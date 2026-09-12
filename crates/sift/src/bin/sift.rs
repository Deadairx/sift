use clap::Parser;
use sift::{DEFAULT_WORKER_ENDPOINT, ErrorResponse, SubmitJobRequest, SubmitJobResponse};

#[derive(Debug, Parser)]
#[command(version, about = "Submit long-form content to Sift")]
struct Cli {
    /// Download the full VOD, even in debug builds.
    #[arg(long)]
    full_download: bool,

    /// URL to submit for processing.
    url: String,
}

#[tokio::main]
async fn main() {
    sift::init_tracing();

    let cli = Cli::parse();

    if let Err(error) = run(cli).await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn run(cli: Cli) -> Result<(), String> {
    sift::validate_twitch_vod_url(&cli.url)?;

    let worker =
        std::env::var("SIFT_WORKER").unwrap_or_else(|_| DEFAULT_WORKER_ENDPOINT.to_string());
    let endpoint = format!("{}/jobs", worker.trim_end_matches('/'));

    let response = reqwest::Client::new()
        .post(&endpoint)
        .json(&SubmitJobRequest {
            url: cli.url,
            full_download: cli.full_download,
        })
        .send()
        .await
        .map_err(|error| format!("could not reach worker at {worker}: {error}"))?;

    let status = response.status();
    if status.is_success() {
        let accepted = response
            .json::<SubmitJobResponse>()
            .await
            .map_err(|error| format!("worker returned an unreadable success response: {error}"))?;

        println!(
            "submitted job {} ({}) to worker {}",
            accepted.id.0,
            accepted.status.as_str(),
            accepted.worker
        );
        return Ok(());
    }

    let rejected = response.json::<ErrorResponse>().await.map_err(|error| {
        format!("worker rejected the request with HTTP {status}, but returned an unreadable error: {error}")
    })?;

    Err(format!(
        "worker rejected the request with HTTP {status}: {}",
        rejected.error.message
    ))
}
