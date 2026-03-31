//! Retry logic and streaming fallback handling.
//!
//! When streaming fails mid-response, the retry handler can:
//! - Discard partial tool executions with synthetic error blocks
//! - Fall back to a smaller model on repeated overload errors
//! - Apply exponential backoff with jitter

use std::time::Duration;

/// Retry configuration.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum retry attempts for transient errors.
    pub max_retries: u32,
    /// Initial backoff duration.
    pub initial_backoff: Duration,
    /// Maximum backoff duration.
    pub max_backoff: Duration,
    /// Backoff multiplier (exponential).
    pub multiplier: f64,
    /// Maximum 529 (overloaded) retries before falling back.
    pub max_overload_retries: u32,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_backoff: Duration::from_millis(1000),
            max_backoff: Duration::from_secs(60),
            multiplier: 2.0,
            max_overload_retries: 3,
        }
    }
}

/// State tracker for retry logic across multiple attempts.
#[derive(Debug, Default)]
pub struct RetryState {
    /// Number of consecutive failures.
    pub consecutive_failures: u32,
    /// Number of 429 (rate limit) retries.
    pub rate_limit_retries: u32,
    /// Number of 529 (overload) retries.
    pub overload_retries: u32,
    /// Whether we've fallen back to the smaller model.
    pub using_fallback: bool,
}

impl RetryState {
    /// Determine the next action after a failure.
    pub fn next_action(&mut self, error: &RetryableError, config: &RetryConfig) -> RetryAction {
        self.consecutive_failures += 1;

        match error {
            RetryableError::RateLimited { retry_after } => {
                self.rate_limit_retries += 1;
                if self.rate_limit_retries > config.max_retries {
                    return RetryAction::Abort("Rate limit retries exhausted".into());
                }
                RetryAction::Retry {
                    after: Duration::from_millis(*retry_after),
                }
            }
            RetryableError::Overloaded => {
                self.overload_retries += 1;
                if self.overload_retries > config.max_overload_retries {
                    if !self.using_fallback {
                        self.using_fallback = true;
                        self.overload_retries = 0;
                        return RetryAction::FallbackModel;
                    }
                    return RetryAction::Abort("Overload retries exhausted on fallback".into());
                }
                let backoff = calculate_backoff(
                    self.overload_retries,
                    config.initial_backoff,
                    config.max_backoff,
                    config.multiplier,
                );
                RetryAction::Retry { after: backoff }
            }
            RetryableError::StreamInterrupted => {
                if self.consecutive_failures > config.max_retries {
                    return RetryAction::Abort("Stream retry limit reached".into());
                }
                let backoff = calculate_backoff(
                    self.consecutive_failures,
                    config.initial_backoff,
                    config.max_backoff,
                    config.multiplier,
                );
                RetryAction::Retry { after: backoff }
            }
            RetryableError::NonRetryable(msg) => RetryAction::Abort(msg.clone()),
        }
    }

    /// Reset counters after a successful call.
    pub fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.rate_limit_retries = 0;
        // Don't reset overload_retries or using_fallback — those persist.
    }
}

/// Categorized error for retry logic.
pub enum RetryableError {
    RateLimited { retry_after: u64 },
    Overloaded,
    StreamInterrupted,
    NonRetryable(String),
}

/// Action the caller should take after a failure.
pub enum RetryAction {
    /// Wait and retry with the same model.
    Retry { after: Duration },
    /// Switch to the fallback model and retry.
    FallbackModel,
    /// Give up — unrecoverable.
    Abort(String),
}

/// Calculate exponential backoff with jitter.
fn calculate_backoff(attempt: u32, initial: Duration, max: Duration, multiplier: f64) -> Duration {
    let base = initial.as_millis() as f64 * multiplier.powi(attempt as i32 - 1);
    let capped = base.min(max.as_millis() as f64);
    // Add 10% jitter.
    let jitter = capped * 0.1 * rand_f64();
    Duration::from_millis((capped + jitter) as u64)
}

/// Simple pseudo-random f64 in [0, 1) using timestamp.
fn rand_f64() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 1000) as f64 / 1000.0
}
