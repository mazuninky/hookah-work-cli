//! Retry policy: 429 waits `X-Rate-Limit-Reset` seconds, 5xx and transport failures back off
//! exponentially. Every request `hw` sends is read-only, so every request may be retried.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// Wait after a 429 without a usable `X-Rate-Limit-Reset` header.
pub const RATE_LIMIT_DEFAULT: Duration = Duration::from_secs(1);
/// Upper bound for a 429 wait, whatever the server asks for.
pub const RATE_LIMIT_CAP: Duration = Duration::from_secs(60);
/// First backoff step for 5xx and transport failures; doubles per attempt.
pub const BACKOFF_BASE: Duration = Duration::from_millis(200);
/// Upper bound for the exponential backoff.
pub const BACKOFF_CAP: Duration = Duration::from_secs(10);

/// Source of delays between attempts, injectable so tests never really sleep.
pub trait Sleeper: Send + Sync {
    /// Blocks for `duration`.
    fn sleep(&self, duration: Duration);
}

/// Production [`Sleeper`]: `std::thread::sleep`.
#[derive(Debug, Default, Clone, Copy)]
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// Null-object [`Sleeper`]: retries run back to back.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSleep;

impl Sleeper for NoSleep {
    fn sleep(&self, _duration: Duration) {}
}

/// How many times to retry and how to wait in between.
#[derive(Clone)]
pub struct RetryPolicy {
    max_retries: u32,
    sleeper: Arc<dyn Sleeper>,
}

impl RetryPolicy {
    /// Retries up to `max_retries` times with real sleeps.
    #[must_use]
    pub fn new(max_retries: u32) -> Self {
        Self::with_sleeper(max_retries, Arc::new(ThreadSleeper))
    }

    /// Retries up to `max_retries` times, waiting through `sleeper`.
    #[must_use]
    pub fn with_sleeper(max_retries: u32, sleeper: Arc<dyn Sleeper>) -> Self {
        Self {
            max_retries,
            sleeper,
        }
    }

    /// Maximum number of retries after the first attempt.
    #[must_use]
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// The wait before retrying a response with `status`, or `None` when it must not be retried.
    /// `attempt` counts retries already made.
    #[must_use]
    pub fn delay_after_status(
        &self,
        status: u16,
        reset: Option<&str>,
        attempt: u32,
    ) -> Option<Duration> {
        (attempt < self.max_retries && is_retryable_status(status)).then(|| {
            if status == 429 {
                rate_limit_delay(reset)
            } else {
                backoff_delay(attempt)
            }
        })
    }

    /// The wait before retrying after a transport error, or `None` when it must not be retried.
    #[must_use]
    pub fn delay_after_transport(&self, err: &ureq::Error, attempt: u32) -> Option<Duration> {
        (attempt < self.max_retries && is_retryable_transport(err)).then(|| backoff_delay(attempt))
    }

    /// Waits through the configured [`Sleeper`].
    pub fn sleep(&self, duration: Duration) {
        self.sleeper.sleep(duration);
    }
}

impl fmt::Debug for RetryPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RetryPolicy")
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

/// 429 and every 5xx are worth another attempt.
#[must_use]
pub fn is_retryable_status(status: u16) -> bool {
    status == 429 || (500..=599).contains(&status)
}

/// Network-level failures that may succeed on a second try; malformed requests never do.
#[must_use]
pub fn is_retryable_transport(err: &ureq::Error) -> bool {
    matches!(
        err,
        ureq::Error::Io(_)
            | ureq::Error::Timeout(_)
            | ureq::Error::ConnectionFailed
            | ureq::Error::HostNotFound
            | ureq::Error::Protocol(_)
    )
}

/// Wait for a 429: `X-Rate-Limit-Reset` seconds capped at [`RATE_LIMIT_CAP`],
/// [`RATE_LIMIT_DEFAULT`] when the header is missing or unparsable.
#[must_use]
pub fn rate_limit_delay(reset: Option<&str>) -> Duration {
    reset
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|secs| secs.is_finite() && *secs >= 0.0)
        .map_or(RATE_LIMIT_DEFAULT, |secs| {
            Duration::from_secs_f64(secs.min(RATE_LIMIT_CAP.as_secs_f64()))
        })
}

/// Exponential backoff: `BACKOFF_BASE · 2^attempt`, capped at [`BACKOFF_CAP`].
#[must_use]
pub fn backoff_delay(attempt: u32) -> Duration {
    let factor = 1u32.checked_shl(attempt.min(31)).unwrap_or(u32::MAX);
    BACKOFF_BASE.saturating_mul(factor).min(BACKOFF_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limit_uses_header_with_cap_and_default() {
        assert_eq!(rate_limit_delay(Some("5")), Duration::from_secs(5));
        assert_eq!(rate_limit_delay(Some("600")), RATE_LIMIT_CAP);
        assert_eq!(rate_limit_delay(None), RATE_LIMIT_DEFAULT);
        assert_eq!(rate_limit_delay(Some("soon")), RATE_LIMIT_DEFAULT);
        assert_eq!(rate_limit_delay(Some("-3")), RATE_LIMIT_DEFAULT);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_delay(0), Duration::from_millis(200));
        assert_eq!(backoff_delay(1), Duration::from_millis(400));
        assert_eq!(backoff_delay(3), Duration::from_millis(1600));
        assert_eq!(backoff_delay(10), BACKOFF_CAP);
        assert_eq!(backoff_delay(u32::MAX), BACKOFF_CAP);
    }

    #[test]
    fn status_retry_decisions() {
        let policy = RetryPolicy::with_sleeper(2, Arc::new(NoSleep));
        assert_eq!(
            policy.delay_after_status(429, Some("2"), 0),
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            policy.delay_after_status(503, None, 1),
            Some(Duration::from_millis(400))
        );
        assert_eq!(
            policy.delay_after_status(503, None, 2),
            None,
            "budget spent"
        );
        assert_eq!(policy.delay_after_status(404, None, 0), None);
        assert_eq!(policy.delay_after_status(200, None, 0), None);
    }

    #[test]
    fn transport_retry_decisions() {
        let policy = RetryPolicy::with_sleeper(1, Arc::new(NoSleep));
        assert!(
            policy
                .delay_after_transport(&ureq::Error::ConnectionFailed, 0)
                .is_some()
        );
        assert!(
            policy
                .delay_after_transport(&ureq::Error::ConnectionFailed, 1)
                .is_none()
        );
        assert!(
            policy
                .delay_after_transport(&ureq::Error::BadUri("x".into()), 0)
                .is_none()
        );
    }

    #[test]
    fn zero_retries_never_retries() {
        let policy = RetryPolicy::new(0);
        assert_eq!(policy.max_retries(), 0);
        assert_eq!(policy.delay_after_status(429, None, 0), None);
    }
}
