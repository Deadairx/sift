use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use url::Url;

pub const DEFAULT_WORKER_ENDPOINT: &str = "http://127.0.0.1:7387";
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
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must be after Unix epoch")
        .as_millis();
    let counter = JOB_COUNTER.fetch_add(1, Ordering::Relaxed);

    JobId(format!("job_{timestamp}_{counter}"))
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
    use super::validate_twitch_vod_url;

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
}
