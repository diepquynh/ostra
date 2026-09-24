use crate::ProviderError;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// Exponential backoff with jitter on retryable errors, never after output started.
#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        RetryPolicy {
            max_retries: 4,
            base_delay: Duration::from_millis(1000),
            max_delay: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// Tiny delays for tests.
    pub fn fast(max_retries: u32) -> Self {
        RetryPolicy {
            max_retries,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
        }
    }

    fn delay(&self, attempt: u32, retry_after: Option<u64>) -> Duration {
        let exp = self.base_delay.saturating_mul(1u32 << attempt.min(16));
        let capped = exp.min(self.max_delay);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        let jitter = 0.5 + (nanos % 1000) as f64 / 2000.0;
        let jittered = capped.mul_f64(jitter);
        match retry_after {
            Some(secs) => jittered.max(Duration::from_secs(secs)),
            None => jittered,
        }
    }
}

pub(crate) async fn run<T, F, Fut>(
    policy: RetryPolicy,
    cancel: &CancellationToken,
    started: &AtomicBool,
    mut attempt: F,
) -> Result<T, ProviderError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, ProviderError>>,
{
    let mut n = 0;
    loop {
        let result = tokio::select! {
            _ = cancel.cancelled() => Err(ProviderError::Cancelled),
            r = attempt() => r,
        };
        match result {
            Err(e)
                if e.is_retryable()
                    && !started.load(Ordering::SeqCst)
                    && n < policy.max_retries =>
            {
                let wait = policy.delay(n, e.retry_after());
                tracing::warn!(attempt = n + 1, wait_ms = wait.as_millis() as u64, error = %e, "provider call failed; retrying");
                tokio::select! {
                    _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
                    _ = tokio::time::sleep(wait) => {}
                }
                n += 1;
            }
            Err(e) if e.is_retryable() && started.load(Ordering::SeqCst) => {
                return Err(ProviderError::Stream(e.to_string()));
            }
            other => return other,
        }
    }
}

fn retry_after_header(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map(|secs| secs.ceil().max(0.0) as u64)
}

/// Message from an error body: `error.message` for both providers, else a clipped raw body.
pub(crate) fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| body.chars().take(300).collect())
}

pub(crate) fn error_type(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.pointer("/error/type")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
}

pub(crate) fn status_error(status: u16, body: &str, retry_after: Option<u64>) -> ProviderError {
    let message = error_message(body);
    let overloaded = error_type(body).as_deref() == Some("overloaded_error");
    match status {
        401 | 403 => ProviderError::Auth { status, message },
        429 => ProviderError::RateLimited {
            message,
            retry_after,
        },
        529 => ProviderError::Overloaded {
            message,
            retry_after,
        },
        _ if overloaded => ProviderError::Overloaded {
            message,
            retry_after,
        },
        408 | 409 | 500..=599 => ProviderError::Server {
            status,
            message,
            retry_after,
        },
        _ => ProviderError::InvalidRequest { status, message },
    }
}

pub(crate) async fn error_from_response(resp: reqwest::Response) -> ProviderError {
    let status = resp.status().as_u16();
    let retry_after = retry_after_header(resp.headers());
    let body = resp.text().await.unwrap_or_default();
    status_error(status, &body, retry_after)
}

pub(crate) fn network_error(e: reqwest::Error) -> ProviderError {
    ProviderError::Network(e.without_url().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    #[test]
    fn maps_statuses() {
        assert!(matches!(
            status_error(429, "{}", Some(2)),
            ProviderError::RateLimited {
                retry_after: Some(2),
                ..
            }
        ));
        assert!(matches!(
            status_error(529, "", None),
            ProviderError::Overloaded { .. }
        ));
        assert!(matches!(
            status_error(
                500,
                r#"{"error":{"type":"overloaded_error","message":"busy"}}"#,
                None
            ),
            ProviderError::Overloaded { .. }
        ));
        assert!(matches!(
            status_error(401, "", None),
            ProviderError::Auth { .. }
        ));
        let e = status_error(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"bad"}}"#,
            None,
        );
        assert!(
            matches!(e, ProviderError::InvalidRequest { status: 400, ref message } if message == "bad")
        );
    }

    #[tokio::test]
    async fn retries_then_succeeds() {
        let calls = AtomicU32::new(0);
        let started = AtomicBool::new(false);
        let r = run(
            RetryPolicy::fast(3),
            &CancellationToken::new(),
            &started,
            || async {
                if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err(ProviderError::Network("reset".into()))
                } else {
                    Ok(7)
                }
            },
        )
        .await;
        assert_eq!(r.unwrap(), 7);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn no_retry_after_output_started() {
        let calls = AtomicU32::new(0);
        let started = AtomicBool::new(true);
        let r: Result<(), _> = run(
            RetryPolicy::fast(3),
            &CancellationToken::new(),
            &started,
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::Overloaded {
                    message: "x".into(),
                    retry_after: None,
                })
            },
        )
        .await;
        assert!(matches!(r, Err(ProviderError::Stream(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn gives_up_on_non_retryable() {
        let calls = AtomicU32::new(0);
        let started = AtomicBool::new(false);
        let r: Result<(), _> = run(
            RetryPolicy::fast(3),
            &CancellationToken::new(),
            &started,
            || async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(ProviderError::InvalidRequest {
                    status: 400,
                    message: "x".into(),
                })
            },
        )
        .await;
        assert!(r.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
