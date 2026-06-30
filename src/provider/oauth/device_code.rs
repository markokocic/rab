//! OAuth device code flow poller — matching pi's pollOAuthDeviceCodeFlow.
//!
//! Polls the token endpoint at the configured interval until the user
//! completes the login in their browser, the flow times out, or is cancelled.

use std::future::Future;
use std::pin::Pin;
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;

const CANCEL_MESSAGE: &str = "Login cancelled";
const TIMEOUT_MESSAGE: &str = "Device flow timed out";
const SLOW_DOWN_TIMEOUT_MESSAGE: &str = "Device flow timed out after one or more slow_down responses. \
     This is often caused by clock drift in WSL or VM environments. \
     Please sync or restart the VM clock and try again.";
const MINIMUM_INTERVAL_MS: u64 = 1000;
const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 5;
const SLOW_DOWN_INTERVAL_INCREMENT_MS: u64 = 5000;

/// Result from a single poll attempt.
pub enum PollStatus<T> {
    Complete(T),
    Pending,
    SlowDown,
    Failed(String),
}

/// Async poll function type for device code flow.
pub type PollFn<'a, T> = Box<
    dyn FnMut() -> Pin<Box<dyn Future<Output = Result<PollStatus<T>, String>> + Send>> + Send + 'a,
>;

/// Options for the device code poller.
pub struct PollOptions<'a, T> {
    pub interval_seconds: Option<u32>,
    pub expires_in_seconds: Option<u32>,
    pub poll: PollFn<'a, T>,
    pub cancel: Option<CancellationToken>,
}

/// Poll the token endpoint until the user completes login or the flow fails.
pub async fn poll_device_code_flow<T>(mut options: PollOptions<'_, T>) -> Result<T, String> {
    let deadline = match options.expires_in_seconds {
        Some(secs) => std::time::Instant::now() + std::time::Duration::from_secs(secs as u64),
        None => std::time::Instant::now() + std::time::Duration::from_secs(300), // 5 min default
    };

    let mut interval_ms = std::cmp::max(
        MINIMUM_INTERVAL_MS,
        (options
            .interval_seconds
            .unwrap_or(DEFAULT_POLL_INTERVAL_SECONDS as u32) as u64)
            * 1000,
    );

    let mut slow_down_responses = 0;

    while std::time::Instant::now() < deadline {
        if let Some(ref cancel) = options.cancel
            && cancel.is_cancelled()
        {
            return Err(CANCEL_MESSAGE.to_string());
        }

        let result = (options.poll)().await?;
        match result {
            PollStatus::Complete(value) => return Ok(value),
            PollStatus::Failed(msg) => return Err(msg),
            PollStatus::SlowDown => {
                slow_down_responses += 1;
                interval_ms = std::cmp::max(
                    MINIMUM_INTERVAL_MS,
                    interval_ms + SLOW_DOWN_INTERVAL_INCREMENT_MS,
                );
            }
            PollStatus::Pending => {}
        }

        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.as_millis() == 0 {
            break;
        }

        let sleep_ms = std::cmp::min(interval_ms, remaining.as_millis() as u64);
        sleep(Duration::from_millis(sleep_ms)).await;
    }

    Err(if slow_down_responses > 0 {
        SLOW_DOWN_TIMEOUT_MESSAGE.to_string()
    } else {
        TIMEOUT_MESSAGE.to_string()
    })
}
