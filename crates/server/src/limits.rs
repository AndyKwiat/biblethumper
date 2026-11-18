//! Abuse protection: per-client and global token buckets, plus a cap on how
//! many searches run the embedding model at once.
//!
//! Layers (all in-memory, no deps):
//! 1. Per-client buckets, keyed by client IP (see `client_key`):
//!    search: `search_burst`, refilled at `search_per_min`
//!    other API: `api_burst`, refilled at `api_per_min`
//! 2. A global search bucket, so many IPs (a botnet, or spoofed keys) together
//!    still can't exceed `global_search_per_sec` on average.
//! 3. `inference`: a semaphore of `max_concurrent_searches`. Searches wait at most
//!    `queue_wait` for a slot, then get 503. This bounds CPU and memory no matter
//!    what gets past the buckets.
//!
//! Over the limit -> 429 with Retry-After. Static files and /healthz are not limited.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use tokio::sync::Semaphore;

#[derive(Debug, Clone)]
pub struct LimitConfig {
    pub search_burst: f64,
    pub search_per_min: f64,
    pub api_burst: f64,
    pub api_per_min: f64,
    pub global_search_burst: f64,
    pub global_search_per_sec: f64,
    pub max_concurrent_searches: usize,
    pub queue_wait: Duration,
    /// Max tracked clients; idle (full) buckets are dropped beyond this.
    pub max_clients: usize,
}

impl Default for LimitConfig {
    /// Sized for a 2-core box: one uncached search is ~25 ms of CPU.
    /// A person typing (live search, 300 ms debounce) uses ~5-10 searches per query.
    fn default() -> Self {
        LimitConfig {
            search_burst: 30.0,
            search_per_min: 30.0,
            api_burst: 120.0,
            api_per_min: 300.0,
            global_search_burst: 100.0,
            global_search_per_sec: 20.0,
            max_concurrent_searches: 2,
            queue_wait: Duration::from_secs(3),
            max_clients: 50_000,
        }
    }
}

#[derive(Clone, Copy)]
struct Bucket {
    tokens: f64,
    last: Instant,
}

/// Token bucket per key. `check` takes one token or says how long to wait.
pub struct RateLimiter {
    burst: f64,
    per_sec: f64,
    max_keys: usize,
    buckets: Mutex<HashMap<String, Bucket>>,
}

impl RateLimiter {
    pub fn new(burst: f64, per_sec: f64, max_keys: usize) -> Self {
        RateLimiter {
            burst,
            per_sec,
            max_keys,
            buckets: Mutex::new(HashMap::new()),
        }
    }

    pub fn check(&self, key: &str) -> Result<(), Duration> {
        self.check_at(key, Instant::now())
    }

    pub fn check_at(&self, key: &str, now: Instant) -> Result<(), Duration> {
        let mut map = self.buckets.lock().unwrap();
        if map.len() >= self.max_keys && !map.contains_key(key) {
            self.prune(&mut map, now);
            if map.len() >= self.max_keys {
                // Table full of active clients: treat newcomers as over the limit.
                return Err(Duration::from_secs(60));
            }
        }
        let b = map.entry(key.to_string()).or_insert(Bucket {
            tokens: self.burst,
            last: now,
        });
        let elapsed = now.saturating_duration_since(b.last).as_secs_f64();
        b.tokens = (b.tokens + elapsed * self.per_sec).min(self.burst);
        b.last = now;
        if b.tokens >= 1.0 {
            b.tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - b.tokens) / self.per_sec))
        }
    }

    /// Drop buckets that would be full by now (clients that went quiet).
    fn prune(&self, map: &mut HashMap<String, Bucket>, now: Instant) {
        let (burst, per_sec) = (self.burst, self.per_sec);
        map.retain(|_, b| {
            b.tokens + now.saturating_duration_since(b.last).as_secs_f64() * per_sec < burst
        });
    }

    pub fn len(&self) -> usize {
        self.buckets.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

pub struct Limits {
    pub search: RateLimiter,
    pub api: RateLimiter,
    pub global_search: RateLimiter,
    pub inference: Arc<Semaphore>,
    pub queue_wait: Duration,
}

impl Limits {
    pub fn new(c: &LimitConfig) -> Self {
        Limits {
            search: RateLimiter::new(c.search_burst, c.search_per_min / 60.0, c.max_clients),
            api: RateLimiter::new(c.api_burst, c.api_per_min / 60.0, c.max_clients),
            global_search: RateLimiter::new(c.global_search_burst, c.global_search_per_sec, 1),
            inference: Arc::new(Semaphore::new(c.max_concurrent_searches)),
            queue_wait: c.queue_wait,
        }
    }
}

/// Who is asking. Behind radify, requests come Cloudflare -> Caddy -> app;
/// Cloudflare sets `CF-Connecting-IP` to the real client (overwriting any
/// client-sent value). Without it, use the last `X-Forwarded-For` hop (added
/// by Caddy), then the socket peer. A client that bypasses Cloudflare could
/// spoof these headers to dodge its per-IP bucket, but the global bucket and
/// the inference semaphore still bound the total load.
pub fn client_key(headers: &HeaderMap, peer: Option<SocketAddr>) -> String {
    let hdr = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
    };
    if let Some(ip) = hdr("cf-connecting-ip").filter(|s| !s.is_empty()) {
        return normalize_ip(ip);
    }
    if let Some(ip) = hdr("x-forwarded-for")
        .and_then(|v| v.rsplit(',').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return normalize_ip(ip);
    }
    peer.map(|p| p.ip().to_string())
        .unwrap_or_else(|| "unknown".into())
}

/// Cap key length (headers are attacker-controlled) and group IPv6 clients by
/// /64, since one host typically owns a whole /64.
fn normalize_ip(s: &str) -> String {
    match s.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V6(v6)) => {
            let seg = v6.segments();
            format!("{:x}:{:x}:{:x}:{:x}::/64", seg[0], seg[1], seg[2], seg[3])
        }
        Ok(ip) => ip.to_string(),
        Err(_) => s.chars().take(64).collect(),
    }
}

pub fn too_many(retry: Duration, what: &str) -> Response {
    let secs = retry.as_secs_f64().ceil().max(1.0) as u64;
    let mut res = (
        StatusCode::TOO_MANY_REQUESTS,
        axum::Json(crate::ErrorBody {
            error: format!("Whoa, slow down! Too many {what}. Try again in {secs}s."),
        }),
    )
        .into_response();
    res.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(secs));
    res
}

/// Middleware for /api/*: per-client buckets (+ global bucket for search).
pub async fn rate_limit(
    State(state): State<crate::AppState>,
    req: Request,
    next: Next,
) -> Response {
    let peer = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let key = client_key(req.headers(), peer);
    let limits = &state.limits;
    if req.uri().path() == "/api/search" {
        if let Err(retry) = limits.search.check(&key) {
            tracing::warn!(client = %key, "search rate limit");
            return too_many(retry, "searches");
        }
        if let Err(retry) = limits.global_search.check("") {
            tracing::warn!("global search rate limit");
            return too_many(retry, "searches right now (site is busy)");
        }
    } else if let Err(retry) = limits.api.check(&key) {
        tracing::warn!(client = %key, "api rate limit");
        return too_many(retry, "requests");
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_allows_burst_then_refills() {
        let rl = RateLimiter::new(3.0, 1.0, 100);
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(rl.check_at("a", t0).is_ok());
        }
        let wait = rl.check_at("a", t0).unwrap_err();
        assert!(wait <= Duration::from_secs(1) && wait > Duration::ZERO);
        // other keys are independent
        assert!(rl.check_at("b", t0).is_ok());
        // one token back after a second
        assert!(rl.check_at("a", t0 + Duration::from_millis(1001)).is_ok());
        assert!(rl.check_at("a", t0 + Duration::from_millis(1002)).is_err());
        // never refills past burst
        let later = t0 + Duration::from_secs(1000);
        for _ in 0..3 {
            assert!(rl.check_at("a", later).is_ok());
        }
        assert!(rl.check_at("a", later).is_err());
    }

    #[test]
    fn table_is_bounded_and_prunes_idle_clients() {
        let rl = RateLimiter::new(2.0, 1.0, 3);
        let t0 = Instant::now();
        for k in ["a", "b", "c"] {
            rl.check_at(k, t0).unwrap();
        }
        // full of active clients -> newcomer refused, table does not grow
        assert!(rl.check_at("d", t0).is_err());
        assert_eq!(rl.len(), 3);
        // after they go idle, newcomers get in
        assert!(rl.check_at("d", t0 + Duration::from_secs(10)).is_ok());
        assert!(rl.len() <= 3);
    }

    #[test]
    fn client_key_prefers_cloudflare_then_forwarded_then_peer() {
        let peer: SocketAddr = "10.0.0.9:5555".parse().unwrap();
        let mut h = HeaderMap::new();
        assert_eq!(client_key(&h, Some(peer)), "10.0.0.9");
        assert_eq!(client_key(&h, None), "unknown");
        h.insert("x-forwarded-for", "6.6.6.6, 1.2.3.4".parse().unwrap());
        assert_eq!(client_key(&h, Some(peer)), "1.2.3.4");
        h.insert("cf-connecting-ip", "9.9.9.9".parse().unwrap());
        assert_eq!(client_key(&h, Some(peer)), "9.9.9.9");
    }

    #[test]
    fn ipv6_grouped_by_64_and_junk_is_capped() {
        let mut h = HeaderMap::new();
        h.insert(
            "cf-connecting-ip",
            "2001:db8:1:2:aaaa:bbbb:cccc:dddd".parse().unwrap(),
        );
        assert_eq!(client_key(&h, None), "2001:db8:1:2::/64");
        h.insert("cf-connecting-ip", "x".repeat(500).parse().unwrap());
        assert_eq!(client_key(&h, None).len(), 64);
    }
}
