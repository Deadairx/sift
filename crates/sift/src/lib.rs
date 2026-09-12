use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use url::Url;

pub const DEFAULT_WORKER_ENDPOINT: &str = "http://127.0.0.1:7387";
pub const DEFAULT_JOBS_DIR: &str = "sift-jobs";
static JOB_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Hash, Deserialize, Serialize)]
pub struct JobId(pub String);

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct SubmitJobRequest {
    pub url: String,
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
    pub worker: String,
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
    Queued,
}

impl JobStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Queued => "queued",
        }
    }
}

pub fn validate_twitch_vod_url(raw_url: &str) -> Result<(), String> {
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

    Ok(())
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

pub fn persist_initial_job_state(
    jobs_root: impl AsRef<Path>,
    id: JobId,
    source_url: String,
    worker: String,
) -> Result<JobMetadata, String> {
    let now = unix_timestamp_millis();
    let metadata = JobMetadata {
        id,
        source_url,
        worker,
        created_at_ms: now,
        updated_at_ms: now,
        state: JobStatus::Queued,
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
    use super::{JobId, JobStatus, persist_initial_job_state, validate_twitch_vod_url};
    use std::fs;

    #[test]
    fn accepts_twitch_vod_urls() {
        assert!(validate_twitch_vod_url("https://www.twitch.tv/videos/123456789").is_ok());
        assert!(validate_twitch_vod_url("https://twitch.tv/videos/123456789").is_ok());
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
    fn persists_initial_job_state_as_inspectable_json() {
        let root = std::env::temp_dir().join(format!("sift-test-jobs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let metadata = persist_initial_job_state(
            &root,
            JobId("job_test".to_string()),
            "https://www.twitch.tv/videos/123456789".to_string(),
            "test-worker".to_string(),
        )
        .expect("job state should persist");

        let state_file = root.join("job_test").join("state.json");
        let persisted = fs::read_to_string(&state_file).expect("state file should be readable");

        assert_eq!(metadata.state, JobStatus::Queued);
        assert!(persisted.contains("\"id\": \"job_test\""));
        assert!(persisted.contains("\"source_url\": \"https://www.twitch.tv/videos/123456789\""));
        assert!(persisted.contains("\"worker\": \"test-worker\""));
        assert!(persisted.contains("\"state\": \"queued\""));

        fs::remove_dir_all(&root).expect("test jobs directory should clean up");
    }
}
