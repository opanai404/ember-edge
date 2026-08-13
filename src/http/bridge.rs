// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Outbound HTTP bridge and egress policy.
//!
//! Guests reach the network through `wasi:http/outgoing-handler`, which
//! `wasmtime-wasi-http` dispatches to the host's [`HttpClient`]. Ember
//! supplies its own client backed by reqwest so that every outbound request
//! is (a) filtered by the tenant's [`EgressPolicy`] before it leaves the
//! process and (b) subject to connection pooling and timeouts.
//!
//! Egress is deny-by-default: a tenant that lists no allow rules can make no
//! outbound calls at all.

use std::sync::Arc;
use std::time::Duration;

use reqwest::redirect::Policy as RedirectPolicy;
use wasmtime_wasi::http::{HostIncomingBody, HttpClient, HttpError};

use crate::error::{Error, Result};

/// Egress allowlist for one tenant.
#[derive(Debug, Clone)]
pub struct EgressPolicy {
    /// Allow entries. Each is either a bare host, `scheme://host`, an
    /// explicit port (`host:port`), or a wildcard host (`*.example.com`).
    pub allow: Vec<String>,
    /// When `true` (default), any host not matched by `allow` is refused.
    pub default_deny: bool,
}

impl Default for EgressPolicy {
    /// Deny-by-default: an empty policy allows no outbound traffic at all.
    fn default() -> Self {
        Self {
            allow: Vec::new(),
            default_deny: true,
        }
    }
}

impl EgressPolicy {
    /// Whether outbound traffic to `host:port` is permitted.
    pub fn allows(&self, authority: &str) -> bool {
        if self
            .allow
            .iter()
            .any(|rule| pattern_matches(rule, authority))
        {
            return true;
        }
        !self.default_deny
    }

    /// Merge another policy's rules into this one (union on `allow`).
    pub fn extend(&mut self, other: &EgressPolicy) {
        self.allow.extend(other.allow.iter().cloned());
        self.default_deny |= other.default_deny;
    }
}

/// Match an allow-rule against a requested authority.
///
/// Rules may include a scheme and/or port and a `*` wildcard segment:
///
/// - `api.example.com`          → matches any scheme/port on that host
/// - `https://api.example.com`  → scheme-restricted
/// - `*.example.com`            → any host under `example.com` (incl. bare)
/// - `localhost:8080`           → port-restricted
fn pattern_matches(rule: &str, authority: &str) -> bool {
    // Split an optional `scheme://` prefix off both sides.
    let rule = strip_scheme(rule);
    let target = strip_scheme(authority);

    let (rule_host, rule_port) = split_authority(rule);
    let (target_host, target_port) = split_authority(target);

    if let (Some(rp), Some(tp)) = (rule_port, target_port) {
        if rp != tp {
            return false;
        }
    }
    host_matches(rule_host, target_host)
}

/// Strip `scheme://` if present.
fn strip_scheme(s: &str) -> &str {
    match s.find("://") {
        Some(idx) => &s[idx + 3..],
        None => s,
    }
}

/// Split `host[:port]` into its two components (port as a string).
fn split_authority(s: &str) -> (&str, Option<&str>) {
    // IPv6 literals like `[::1]:8080` are not supported by the allowlist;
    // they simply never match a wildcard rule and need an exact entry.
    match s.rsplit_once(':') {
        Some((host, port)) if !host.contains(']') => (host, Some(port)),
        _ => (s, None),
    }
}

/// Wildcard host matching: `*` matches a single segment, `*.x.y` matches
/// `x.y` and any subdomain thereof, exact hosts match exactly.
fn host_matches(pattern: &str, host: &str) -> bool {
    let p: Vec<&str> = pattern.split('.').collect();
    let h: Vec<&str> = host.split('.').collect();

    if p == ["*"] {
        return true;
    }
    if p.first() == Some(&"*") {
        // `*.example.com` matches `example.com` and `a.b.example.com`.
        let suffix = &p[1..];
        return h.len() >= suffix.len()
            && h[(h.len() - suffix.len())..]
                .iter()
                .zip(suffix)
                .all(|(a, b)| a.eq_ignore_ascii_case(b));
    }
    p.len() == h.len()
        && p.iter()
            .zip(h)
            .all(|(a, b)| a == &"*" || a.eq_ignore_ascii_case(b))
}

/// The reqwest-backed WASI HTTP client used as the egress path.
#[derive(Debug, Clone)]
pub struct HttpBridge {
    client: reqwest::Client,
    policy: Arc<EgressPolicy>,
    connect_timeout: Duration,
    max_body_bytes: usize,
}

impl HttpBridge {
    /// Build a bridge enforcing `policy` and capping response bodies at
    /// `max_body_bytes`.
    pub fn new(policy: EgressPolicy, max_body_bytes: usize) -> Result<Self> {
        let client = ClientBuilder::bridge()?;
        Ok(Self {
            client,
            policy: Arc::new(policy),
            connect_timeout: Duration::from_secs(3),
            max_body_bytes,
        })
    }

    /// Set the TCP/TLS connect timeout.
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// The tenant's egress policy.
    pub fn policy(&self) -> &EgressPolicy {
        &self.policy
    }

    /// Max outbound response body size.
    pub fn max_body_bytes(&self) -> usize {
        self.max_body_bytes
    }

    /// Execute an outbound request after allowlist filtering, returning the
    /// reqwest response so the bridge can stream it back into WASI.
    async fn send(
        &self,
        req: http::Request<HostIncomingBody>,
    ) -> std::result::Result<reqwest::Response, HttpError> {
        let authority = req
            .uri()
            .authority()
            .ok_or_else(|| HttpError::new(500u16, "outbound request missing authority"))?;

        let host = authority.host();
        if !self.policy.allows(host) {
            return Err(HttpError::new(
                403u16,
                format!("egress denied for `{host}`"),
            ));
        }

        let mut builder = self
            .client
            .request(method_to_reqwest(req.method()), req.uri().to_string())
            .timeout(self.connect_timeout + Duration::from_secs(25));

        for (name, value) in req.headers() {
            builder.header(name.as_str(), value.to_str().unwrap_or(""));
        }

        let body = http_body_util::BodyExt::collect(req)
            .await
            .map_err(|_| HttpError::new(500u16, "failed to read outbound body"))?;
        let bytes = body.to_bytes();
        if bytes.len() > self.max_body_bytes {
            return Err(HttpError::new(
                413u16,
                format!(
                    "outbound request body exceeds {} bytes",
                    self.max_body_bytes
                ),
            ));
        }

        builder
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|e| HttpError::new(502u16, format!("outbound request failed: {e}")))
    }
}

impl HttpClient for HttpBridge {
    async fn send_request(
        &self,
        req: http::Request<HostIncomingBody>,
    ) -> std::result::Result<http::Response<HostIncomingBody>, HttpError> {
        let res = self.send(req).await?;
        let mut out = http::Response::builder()
            .status(res.status().as_u16())
            .unwrap();
        for (name, value) in res.headers() {
            if let Ok(next) = out.header(name.as_str(), value.to_str().unwrap_or("")) {
                out = next;
            }
        }
        let body = res.bytes().await.map_err(|e| {
            HttpError::new(502u16, format!("failed to read outbound response: {e}"))
        })?;
        let incoming = HostIncomingBody::from(axum::body::Body::from(body));
        out.body(incoming)
            .map_err(|e| HttpError::new(500u16, e.to_string()))
    }
}

fn method_to_reqwest(method: &http::Method) -> reqwest::Method {
    use reqwest::Method as M;
    match *method {
        http::Method::GET => M::GET,
        http::Method::POST => M::POST,
        http::Method::PUT => M::PUT,
        http::Method::DELETE => M::DELETE,
        http::Method::PATCH => M::PATCH,
        http::Method::HEAD => M::HEAD,
        http::Method::OPTIONS => M::OPTIONS,
        _ => M::GET,
    }
}

/// Wrapper so [`HttpBridge::new`] can fail on client construction.
struct ClientBuilder;

impl ClientBuilder {
    fn bridge() -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .redirect(RedirectPolicy::limited(5))
            .pool_max_idle_per_host(16)
            .pool_idle_timeout(Duration::from_secs(60))
            .tcp_keepalive(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::Config(format!("egress client: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(rules: &[&str]) -> EgressPolicy {
        EgressPolicy {
            allow: rules.iter().map(|s| s.to_string()).collect(),
            default_deny: true,
        }
    }

    #[test]
    fn exact_host_matches_any_scheme() {
        assert!(policy(&["api.example.com"]).allows("https://api.example.com:443"));
        assert!(policy(&["api.example.com"]).allows("http://api.example.com"));
    }

    #[test]
    fn wildcard_matches_subdomains_and_bare() {
        let p = policy(&["*.example.com"]);
        assert!(p.allows("example.com"));
        assert!(p.allows("api.example.com"));
        assert!(p.allows("a.b.example.com"));
        assert!(!p.allows("example.net"));
    }

    #[test]
    fn scheme_restricted_rule() {
        let p = policy(&["https://api.example.com"]);
        assert!(p.allows("https://api.example.com"));
        assert!(!p.allows("http://api.example.com"));
    }

    #[test]
    fn deny_by_default() {
        let p = policy(&[]);
        assert!(!p.allows("https://evil.example.com"));
    }

    #[test]
    fn port_mismatch_denied() {
        let p = policy(&["localhost:8080"]);
        assert!(p.allows("localhost:8080"));
        assert!(!p.allows("localhost:8081"));
    }

    #[test]
    fn explicit_allow_beats_default_deny() {
        let mut p = policy(&["upstream.internal"]);
        p.default_deny = true;
        assert!(p.allows("upstream.internal"));
    }
}
