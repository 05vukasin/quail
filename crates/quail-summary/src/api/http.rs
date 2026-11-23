//! One HTTP POST with deadlines, bounded retries and a process-wide concurrency cap.
//!
//! - **Transport**: `ureq` with rustls (pure Rust, blocking, no async runtime), no
//!   redirects (a request body and its key never go to a second host), responses read up to
//!   [`MAX_RESPONSE_BYTES`].
//! - **Time**: each attempt gets `min(call timeout, time left before the deadline)`; no
//!   attempt starts and no backoff sleeps past the deadline.
//! - **Retries**: on 408, 409, 429, 5xx (529 included), timeouts and connection errors, at
//!   most [`RetryPolicy::max_retries`] times, exponential backoff with jitter, at least what
//!   `retry-after` (seconds) or `retry-after-ms` asks for, at most [`RetryPolicy::max_delay`].
//!   A 429 that says the quota or credit is exhausted is not retried.
//! - **Concurrency**: a [`Limiter`] permit is held for the whole call, retries included.

use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use quail_core::{Error, Result};

/// Largest response body read.
pub const MAX_RESPONSE_BYTES: u64 = 4 << 20;

/// A counting semaphore over concurrent API calls.
#[derive(Debug)]
pub struct Limiter {
    state: Mutex<(usize, usize)>, // (in use, cap)
    freed: Condvar,
}

static GLOBAL: OnceLock<Arc<Limiter>> = OnceLock::new();

impl Limiter {
    /// At most `cap` calls at a time (at least one).
    pub fn new(cap: usize) -> Self {
        Self {
            state: Mutex::new((0, cap.max(1))),
            freed: Condvar::new(),
        }
    }

    /// The process-wide limiter every [`ApiSummarizer`](super::ApiSummarizer) shares unless
    /// given its own: `AI_MAX_CONCURRENT_LLM` caps calls across all of them, like hedgehog's
    /// global semaphore. Created on first use with
    /// [`DEFAULT_MAX_CONCURRENT`](super::endpoint::DEFAULT_MAX_CONCURRENT); this and the
    /// llama.cpp backend are the crate's only process-wide state.
    pub fn global() -> Arc<Limiter> {
        Arc::clone(
            GLOBAL.get_or_init(|| Arc::new(Limiter::new(super::endpoint::DEFAULT_MAX_CONCURRENT))),
        )
    }

    /// Changes the cap; calls in flight keep their permits.
    pub fn set_cap(&self, cap: usize) {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        g.1 = cap.max(1);
        drop(g);
        self.freed.notify_all();
    }

    /// The cap.
    pub fn cap(&self) -> usize {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).1
    }

    /// Waits for a permit until `deadline` ([`Error::Timeout`] past it).
    pub fn acquire(&self, deadline: Option<Instant>) -> Result<Permit<'_>> {
        let mut g = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while g.0 >= g.1 {
            g = match deadline {
                None => self.freed.wait(g).unwrap_or_else(PoisonError::into_inner),
                Some(d) => {
                    let left = d.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Err(Error::Timeout);
                    }
                    self.freed
                        .wait_timeout(g, left)
                        .unwrap_or_else(PoisonError::into_inner)
                        .0
                }
            };
        }
        g.0 += 1;
        Ok(Permit(self))
    }
}

/// One call's permit; released on drop.
#[derive(Debug)]
pub struct Permit<'a>(&'a Limiter);

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut g = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        g.0 = g.0.saturating_sub(1);
        drop(g);
        self.0.freed.notify_one();
    }
}

/// How often and how long to retry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Retries after the first attempt.
    pub max_retries: u32,
    /// First backoff; doubled on each retry, jittered to 50–100 %.
    pub base: Duration,
    /// Longest single wait (also caps `retry-after`).
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    /// 3 retries, from 1 s, at most 30 s apart (hedgehog's values).
    fn default() -> Self {
        Self {
            max_retries: 3,
            base: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
        }
    }
}

/// An HTTP response, read whole.
#[derive(Clone, Debug)]
pub struct Response {
    /// Status code.
    pub status: u16,
    /// `retry-after-ms` or `retry-after` (seconds), when given.
    pub retry_after: Option<Duration>,
    /// Body text (at most [`MAX_RESPONSE_BYTES`]).
    pub body: String,
}

/// What went wrong with one attempt.
#[derive(Debug)]
pub enum Failure {
    /// The server answered with a non-success status.
    Status(Response),
    /// The attempt timed out.
    Timeout,
    /// No connection, or it broke.
    Connection(String),
}

impl Failure {
    /// Whether another attempt may succeed.
    fn retryable(&self) -> bool {
        match self {
            Self::Timeout | Self::Connection(_) => true,
            Self::Status(r) => {
                (matches!(r.status, 408 | 409 | 429) || r.status >= 500)
                    && !quota_exhausted(&r.body)
            }
        }
    }

    /// The server's suggested wait.
    fn hint(&self) -> Option<Duration> {
        match self {
            Self::Status(r) => r.retry_after,
            _ => None,
        }
    }
}

/// `insufficient_quota` and friends come as 429 but retrying cannot help.
fn quota_exhausted(body: &str) -> bool {
    ["insufficient_quota", "credit_balance", "billing_hard_limit"]
        .iter()
        .any(|c| body.contains(c))
}

/// A blocking HTTP client for one endpoint.
#[derive(Debug)]
pub struct Client {
    agent: ureq::Agent,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// No redirects, status codes as values, `quail/<version>` as user agent.
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("quail/", env!("CARGO_PKG_VERSION")))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }

    /// One POST of `body` (JSON) to `url` with `headers`, within `timeout`.
    pub fn post_once(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &str,
        timeout: Duration,
    ) -> std::result::Result<Response, Failure> {
        let mut req = self
            .agent
            .post(url)
            .config()
            .timeout_global(Some(timeout))
            .build()
            .header("content-type", "application/json");
        for (k, v) in headers {
            req = req.header(*k, v.as_str());
        }
        let mut resp = req.send(body).map_err(classify)?;
        let status = resp.status().as_u16();
        let header = |name: &str| {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .map(str::to_owned)
        };
        let retry_after = header("retry-after-ms")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|ms| ms / 1000.0)
            .or_else(|| header("retry-after").and_then(|v| v.parse::<f64>().ok()))
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(|s| Duration::from_secs_f64(s.min(3600.0)));
        let body = resp
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BYTES)
            .read_to_string()
            .map_err(classify)?;
        let r = Response {
            status,
            retry_after,
            body,
        };
        if (200..300).contains(&status) {
            Ok(r)
        } else {
            Err(Failure::Status(r))
        }
    }

    /// [`Self::post_once`] with retries (see the module docs): each attempt gets
    /// `min(call_timeout, time left)`; `sleep` waits between attempts (tests pass a recorder).
    #[allow(clippy::too_many_arguments)] // one request, its time limits and its retry policy
    pub fn post(
        &self,
        url: &str,
        headers: &[(&str, String)],
        body: &str,
        call_timeout: Duration,
        deadline: Instant,
        policy: RetryPolicy,
        sleep: &dyn Fn(Duration),
    ) -> std::result::Result<Response, Failure> {
        let mut attempt = 0u32;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Failure::Timeout);
            }
            let err = match self.post_once(url, headers, body, call_timeout.min(left)) {
                Ok(r) => return Ok(r),
                Err(e) => e,
            };
            if !err.retryable() || attempt >= policy.max_retries {
                return Err(err);
            }
            let mut delay = backoff(policy, attempt);
            if let Some(h) = err.hint() {
                delay = delay.max(h);
            }
            let delay = delay.min(policy.max_delay);
            if Instant::now() + delay >= deadline {
                return Err(err);
            }
            attempt += 1;
            sleep(delay);
        }
    }
}

/// `base * 2^attempt`, jittered to 50–100 % (no RNG dependency: the clock's nanoseconds).
fn backoff(policy: RetryPolicy, attempt: u32) -> Duration {
    let full = policy.base.saturating_mul(1u32 << attempt.min(16));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let frac = 0.5 + f64::from(nanos % 1000) / 2000.0;
    full.mul_f64(frac)
}

fn classify(e: ureq::Error) -> Failure {
    match e {
        ureq::Error::Timeout(_) => Failure::Timeout,
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => Failure::Timeout,
        other => Failure::Connection(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_caps_and_times_out() {
        let l = Limiter::new(1);
        let p = l.acquire(None).unwrap();
        let soon = Instant::now() + Duration::from_millis(30);
        assert!(matches!(l.acquire(Some(soon)), Err(Error::Timeout)));
        drop(p);
        assert!(l.acquire(Some(soon + Duration::from_secs(1))).is_ok());
        l.set_cap(3);
        assert_eq!(l.cap(), 3);
    }

    #[test]
    fn backoff_doubles_with_jitter() {
        let p = RetryPolicy {
            base: Duration::from_millis(100),
            ..RetryPolicy::default()
        };
        for a in 0..4 {
            let d = backoff(p, a);
            let full = Duration::from_millis(100 << a);
            assert!(d >= full / 2 && d <= full, "{a}: {d:?}");
        }
    }

    #[test]
    fn what_is_retried() {
        let st = |status, body: &str| {
            Failure::Status(Response {
                status,
                retry_after: Some(Duration::from_secs(2)),
                body: body.into(),
            })
        };
        assert!(st(429, "{}").retryable());
        assert_eq!(st(429, "{}").hint(), Some(Duration::from_secs(2)));
        assert!(st(529, "overloaded").retryable());
        assert!(st(503, "").retryable());
        assert!(!st(400, "bad").retryable());
        assert!(!st(401, "").retryable());
        assert!(!st(429, r#"{"error":{"code":"insufficient_quota"}}"#).retryable());
        assert!(Failure::Timeout.retryable());
        assert_eq!(Failure::Timeout.hint(), None);
    }
}
