use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use url::Url;

pub const DEFAULT_WORKER_ENDPOINT: &str = "http://127.0.0.1:7387";
pub const DEFAULT_WORKER_LISTEN_ADDR: &str = "127.0.0.1:7387";
pub const DEFAULT_JOBS_DIR: &str = "sift-jobs";
pub const DEFAULT_MEDIA_DOWNLOAD_SECONDS: u64 = 30;
static JOB_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct JobId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct SubmitJobRequest {
    pub url: String,
    pub full_download: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TwitchVod {
    pub id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct SubmitJobResponse {
    pub id: JobId,
    pub status: JobStatus,
    pub worker: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct JobMetadata {
    pub id: JobId,
    pub source_url: String,
    pub twitch_vod_id: Option<String>,
    pub worker: String,
    pub full_download: bool,
    pub created_at_ms: u128,
    pub updated_at_ms: u128,
    pub state: JobStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorBody,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Accepted,
    Downloading,
    Downloaded,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Downloading => "downloading",
            Self::Downloaded => "downloaded",
        }
    }
}

pub fn validate_twitch_vod_url(raw_url: &str) -> Result<(), String> {
    extract_twitch_vod(raw_url).map(|_| ())
}

pub fn extract_twitch_vod(raw_url: &str) -> Result<TwitchVod, String> {
    let parsed = Url::parse(raw_url).map_err(|_| "expected an absolute URL".to_string())?;

    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return Err("expected an http or https URL".to_string());
    }

    let Some(host) = parsed.host_str() else {
        return Err("expected a URL host".to_string());
    };

    if host != "twitch.tv" && host != "www.twitch.tv" {
        return Err("expected a Twitch URL".to_string());
    }

    let mut segments = parsed.path_segments().into_iter().flatten();
    if segments.next() != Some("videos") {
        return Err("expected a Twitch VOD URL like https://www.twitch.tv/videos/<id>".to_string());
    }

    let Some(video_id) = segments.next() else {
        return Err("expected a Twitch VOD id".to_string());
    };

    if video_id.is_empty() || !video_id.chars().all(|character| character.is_ascii_digit()) {
        return Err("expected a numeric Twitch VOD id".to_string());
    }

    if segments.next().is_some() {
        return Err("expected a Twitch VOD URL like https://www.twitch.tv/videos/<id>".to_string());
    }

    Ok(TwitchVod {
        id: video_id.to_string(),
    })
}

pub fn generate_job_id() -> JobId {
    let timestamp = unix_timestamp_millis();
    let counter = JOB_COUNTER.fetch_add(1, Ordering::Relaxed);

    JobId(format!("job_{timestamp}_{counter}"))
}

pub fn jobs_dir_from_env() -> PathBuf {
    std::env::var_os("SIFT_JOBS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_JOBS_DIR))
}

pub fn worker_listen_addr_from_env() -> Result<std::net::SocketAddr, String> {
    let value = std::env::var("SIFT_WORKER_LISTEN_ADDR")
        .unwrap_or_else(|_| DEFAULT_WORKER_LISTEN_ADDR.to_string());

    value
        .parse()
        .map_err(|error| format!("SIFT_WORKER_LISTEN_ADDR must be host:port: {error}"))
}

pub fn persist_initial_job_state(
    jobs_root: impl AsRef<Path>,
    id: JobId,
    source_url: String,
    worker: String,
    full_download: bool,
) -> Result<JobMetadata, String> {
    let now = unix_timestamp_millis();
    let metadata = JobMetadata {
        id,
        source_url,
        twitch_vod_id: None,
        worker,
        full_download,
        created_at_ms: now,
        updated_at_ms: now,
        state: JobStatus::Accepted,
    };

    let job_dir = jobs_root.as_ref().join(&metadata.id.0);
    fs::create_dir_all(&job_dir).map_err(|error| {
        format!(
            "could not create job directory {}: {error}",
            job_dir.display()
        )
    })?;

    let state_path = job_dir.join("state.json");
    let body = serde_json::to_vec_pretty(&metadata)
        .map_err(|error| format!("could not serialize job metadata: {error}"))?;
    fs::write(&state_path, body).map_err(|error| {
        format!(
            "could not write job state file {}: {error}",
            state_path.display()
        )
    })?;

    Ok(metadata)
}

pub async fn begin_twitch_ingestion(
    jobs_root: impl AsRef<Path>,
    metadata: JobMetadata,
) -> Result<JobMetadata, String> {
    let metadata = prepare_twitch_ingestion(&jobs_root, metadata)?;
    tracing::info!(
        job_id = %metadata.id.0,
        url = %metadata.source_url,
        worker = %metadata.worker,
        to_state = %metadata.state.as_str(),
        twitch_vod_id = metadata.twitch_vod_id.as_deref().unwrap_or(""),
        "job entered Twitch media download path"
    );
    let mut metadata = metadata;
    download_twitch_media(&jobs_root, &metadata).await?;
    metadata.updated_at_ms = unix_timestamp_millis();
    metadata.state = JobStatus::Downloaded;
    write_job_state(&jobs_root.as_ref().join(&metadata.id.0), &metadata)?;

    Ok(metadata)
}

pub fn prepare_twitch_ingestion(
    jobs_root: impl AsRef<Path>,
    mut metadata: JobMetadata,
) -> Result<JobMetadata, String> {
    let vod = extract_twitch_vod(&metadata.source_url)?;
    metadata.twitch_vod_id = Some(vod.id.clone());
    metadata.updated_at_ms = unix_timestamp_millis();
    metadata.state = JobStatus::Downloading;

    let job_dir = jobs_root.as_ref().join(&metadata.id.0);
    let ingestion_dir = job_dir.join("ingestion");
    let media_dir = job_dir.join("media");
    fs::create_dir_all(&ingestion_dir).map_err(|error| {
        format!(
            "could not create ingestion directory {}: {error}",
            ingestion_dir.display()
        )
    })?;
    fs::create_dir_all(&media_dir).map_err(|error| {
        format!(
            "could not create media directory {}: {error}",
            media_dir.display()
        )
    })?;

    write_job_state(&job_dir, &metadata)?;

    let artifact = TwitchIngestionArtifact {
        job_id: metadata.id.0.clone(),
        source_url: metadata.source_url.clone(),
        twitch_vod_id: vod.id,
        action: "download_twitch_vod_media".to_string(),
        state: metadata.state.clone(),
        media_dir: "media".to_string(),
        created_at_ms: metadata.updated_at_ms,
    };
    write_json_file(&ingestion_dir.join("twitch-vod.json"), &artifact)?;

    Ok(metadata)
}

async fn download_twitch_media(
    jobs_root: impl AsRef<Path>,
    metadata: &JobMetadata,
) -> Result<(), String> {
    let job_dir = jobs_root.as_ref().join(&metadata.id.0);
    let ingestion_dir = job_dir.join("ingestion");
    let media_dir = job_dir.join("media");
    let downloader = std::env::var("SIFT_YT_DLP").unwrap_or_else(|_| "yt-dlp".to_string());
    let download_seconds = media_download_seconds(metadata.full_download)?;
    let output_template = media_dir.join("vod-%(id)s.%(ext)s");
    let output_template = output_template.to_string_lossy().to_string();
    let mut args = vec!["--no-progress".to_string(), "--newline".to_string()];
    if let Some(seconds) = download_seconds {
        args.extend([
            "--download-sections".to_string(),
            format!("*0-{seconds}"),
            "--force-keyframes-at-cuts".to_string(),
        ]);
    }
    args.extend([
        "-o".to_string(),
        output_template.clone(),
        metadata.source_url.clone(),
    ]);

    let request = MediaDownloadRequestArtifact {
        job_id: metadata.id.0.clone(),
        source_url: metadata.source_url.clone(),
        downloader: downloader.clone(),
        args: args.clone(),
        output_template: relative_to_job(&job_dir, Path::new(&output_template)),
        mode: if download_seconds.is_some() {
            "debug_bounded".to_string()
        } else {
            "full".to_string()
        },
        download_seconds,
        created_at_ms: unix_timestamp_millis(),
    };
    write_json_file(&ingestion_dir.join("media-download-request.json"), &request)?;

    let output = Command::new(&downloader)
        .args(&args)
        .output()
        .await
        .map_err(|error| {
            format!("could not invoke Twitch media downloader {downloader}: {error}")
        })?;
    let media_artifacts = list_media_artifacts(&job_dir, &media_dir)?;
    let result = MediaDownloadResultArtifact {
        job_id: metadata.id.0.clone(),
        source_url: metadata.source_url.clone(),
        downloader,
        exit_code: output.status.code(),
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        media_artifacts: media_artifacts.clone(),
        finished_at_ms: unix_timestamp_millis(),
    };
    write_json_file(&ingestion_dir.join("media-download-result.json"), &result)?;

    if !output.status.success() {
        return Err(format!(
            "Twitch media downloader failed with exit code {:?}",
            output.status.code()
        ));
    }

    if media_artifacts.is_empty() {
        return Err(
            "Twitch media downloader completed without writing media artifacts".to_string(),
        );
    }

    Ok(())
}

fn media_download_seconds(full_download: bool) -> Result<Option<u64>, String> {
    if full_download || !cfg!(debug_assertions) {
        return Ok(None);
    }

    let Some(value) = std::env::var_os("SIFT_MEDIA_DOWNLOAD_SECONDS") else {
        return Ok(Some(DEFAULT_MEDIA_DOWNLOAD_SECONDS));
    };
    let seconds = value
        .to_string_lossy()
        .parse::<u64>()
        .map_err(|error| format!("SIFT_MEDIA_DOWNLOAD_SECONDS must be an integer: {error}"))?;
    Ok(Some(seconds))
}

fn list_media_artifacts(job_dir: &Path, media_dir: &Path) -> Result<Vec<MediaArtifact>, String> {
    let mut artifacts = Vec::new();
    for entry in fs::read_dir(media_dir).map_err(|error| {
        format!(
            "could not read media directory {}: {error}",
            media_dir.display()
        )
    })? {
        let entry = entry.map_err(|error| {
            format!(
                "could not read media directory entry in {}: {error}",
                media_dir.display()
            )
        })?;
        let path = entry.path();
        let metadata = entry.metadata().map_err(|error| {
            format!("could not stat media artifact {}: {error}", path.display())
        })?;
        if metadata.is_file() && metadata.len() > 0 {
            artifacts.push(MediaArtifact {
                path: relative_to_job(job_dir, &path),
                bytes: metadata.len(),
            });
        }
    }
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(artifacts)
}

fn relative_to_job(job_dir: &Path, path: &Path) -> String {
    path.strip_prefix(job_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn write_job_state(job_dir: &Path, metadata: &JobMetadata) -> Result<(), String> {
    write_json_file(&job_dir.join("state.json"), metadata)
}

fn write_json_file(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("could not serialize JSON for {}: {error}", path.display()))?;
    fs::write(path, body)
        .map_err(|error| format!("could not write JSON file {}: {error}", path.display()))
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct TwitchIngestionArtifact {
    pub job_id: String,
    pub source_url: String,
    pub twitch_vod_id: String,
    pub action: String,
    pub state: JobStatus,
    pub media_dir: String,
    pub created_at_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct MediaDownloadRequestArtifact {
    pub job_id: String,
    pub source_url: String,
    pub downloader: String,
    pub args: Vec<String>,
    pub output_template: String,
    pub mode: String,
    pub download_seconds: Option<u64>,
    pub created_at_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct MediaDownloadResultArtifact {
    pub job_id: String,
    pub source_url: String,
    pub downloader: String,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub media_artifacts: Vec<MediaArtifact>,
    pub finished_at_ms: u128,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct MediaArtifact {
    pub path: String,
    pub bytes: u64,
}

fn unix_timestamp_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_millis()
}

pub fn error_response(code: impl Into<String>, message: impl Into<String>) -> ErrorResponse {
    ErrorResponse {
        error: ErrorBody {
            code: code.into(),
            message: message.into(),
        },
    }
}

pub fn init_tracing() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer())
        .init();
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_MEDIA_DOWNLOAD_SECONDS, JobId, JobStatus, extract_twitch_vod,
        media_download_seconds, persist_initial_job_state, prepare_twitch_ingestion,
        validate_twitch_vod_url,
    };
    use std::fs;

    #[test]
    fn accepts_twitch_vod_urls() {
        assert!(validate_twitch_vod_url("https://www.twitch.tv/videos/123456789").is_ok());
        assert!(validate_twitch_vod_url("https://twitch.tv/videos/123456789").is_ok());
    }

    #[test]
    fn extracts_twitch_vod_id() {
        let vod = extract_twitch_vod("https://www.twitch.tv/videos/123456789")
            .expect("Twitch VOD id should extract");

        assert_eq!(vod.id, "123456789");
    }

    #[test]
    fn rejects_non_twitch_urls() {
        let error = validate_twitch_vod_url("https://example.com/videos/123456789")
            .expect_err("non-Twitch URL should be rejected");

        assert_eq!(error, "expected a Twitch URL");
    }

    #[test]
    fn rejects_twitch_urls_without_numeric_vod_id() {
        let error = validate_twitch_vod_url("https://www.twitch.tv/videos/not-a-vod")
            .expect_err("non-numeric VOD id should be rejected");

        assert_eq!(error, "expected a numeric Twitch VOD id");
    }

    #[test]
    fn full_download_override_disables_debug_download_limit() {
        assert_eq!(
            media_download_seconds(true).expect("mode should resolve"),
            None
        );
    }

    #[test]
    fn debug_build_defaults_to_bounded_download_without_override() {
        let expected = if cfg!(debug_assertions) {
            Some(DEFAULT_MEDIA_DOWNLOAD_SECONDS)
        } else {
            None
        };

        assert_eq!(
            media_download_seconds(false).expect("mode should resolve"),
            expected
        );
    }

    #[test]
    fn parses_worker_listen_addresses() {
        let addr: std::net::SocketAddr =
            "0.0.0.0:7387".parse().expect("listen address should parse");

        assert_eq!(addr.to_string(), "0.0.0.0:7387");
    }

    #[test]
    fn persists_initial_job_state_as_inspectable_json() {
        let root = std::env::temp_dir().join(format!("sift-test-jobs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let metadata = persist_initial_job_state(
            &root,
            JobId("job_test".to_string()),
            "https://www.twitch.tv/videos/123456789".to_string(),
            "test-worker".to_string(),
            false,
        )
        .expect("job state should persist");

        let state_file = root.join("job_test").join("state.json");
        let persisted = fs::read_to_string(&state_file).expect("state file should be readable");

        assert_eq!(metadata.state, JobStatus::Accepted);
        assert!(persisted.contains("\"id\": \"job_test\""));
        assert!(persisted.contains("\"source_url\": \"https://www.twitch.tv/videos/123456789\""));
        assert!(persisted.contains("\"twitch_vod_id\": null"));
        assert!(persisted.contains("\"worker\": \"test-worker\""));
        assert!(persisted.contains("\"full_download\": false"));
        assert!(persisted.contains("\"state\": \"accepted\""));

        fs::remove_dir_all(&root).expect("test jobs directory should clean up");
    }

    #[test]
    fn preparing_twitch_ingestion_marks_job_downloading_and_writes_artifact() {
        let root = std::env::temp_dir().join(format!("sift-test-ingestion-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let metadata = persist_initial_job_state(
            &root,
            JobId("job_ingest".to_string()),
            "https://www.twitch.tv/videos/123456789".to_string(),
            "test-worker".to_string(),
            true,
        )
        .expect("job state should persist");

        let metadata =
            prepare_twitch_ingestion(&root, metadata).expect("job should enter Twitch ingestion");

        let state_file = root.join("job_ingest").join("state.json");
        let state = fs::read_to_string(&state_file).expect("state file should be readable");
        let artifact_file = root
            .join("job_ingest")
            .join("ingestion")
            .join("twitch-vod.json");
        let artifact =
            fs::read_to_string(&artifact_file).expect("artifact file should be readable");
        let media_dir = root.join("job_ingest").join("media");

        assert_eq!(metadata.state, JobStatus::Downloading);
        assert_eq!(metadata.twitch_vod_id.as_deref(), Some("123456789"));
        assert!(state.contains("\"state\": \"downloading\""));
        assert!(state.contains("\"twitch_vod_id\": \"123456789\""));
        assert!(state.contains("\"full_download\": true"));
        assert!(artifact.contains("\"action\": \"download_twitch_vod_media\""));
        assert!(artifact.contains("\"media_dir\": \"media\""));
        assert!(artifact.contains("\"twitch_vod_id\": \"123456789\""));
        assert!(media_dir.is_dir());

        fs::remove_dir_all(&root).expect("test jobs directory should clean up");
    }
}
