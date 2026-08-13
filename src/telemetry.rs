// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Tracing and metrics plumbing.
//!
//! [`init`] installs a `tracing_subscriber` filtered by `RUST_LOG` and a
//! Prometheus recorder whose handle is exposed on the ingress `/metrics`
//! route. All invocation accounting flows through [`record_invoke`].

use std::sync::OnceLock;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tracing_subscriber::{EnvFilter, fmt::format::FmtSpan};

use crate::error::{Error, Result};

/// Lazily-initialized Prometheus handle, shared with the ingress `/metrics`
/// route. Only installs once; repeated calls return the same handle.
static PROMETHEUS: OnceLock<PrometheusHandle> = OnceLock::new();

fn install_recorder() -> Result<&'static PrometheusHandle> {
    Ok(PROMETHEUS.get_or_try_init(|| {
        PrometheusBuilder::new()
            .install_recorder()
            .map_err(|e| Error::Config(format!("metrics recorder: {e}")))
    })?)
}

/// Initialize tracing and the metrics recorder.
///
/// `env_filter` defaults to `EMBER_LOG` or `RUST_LOG`; pass `--json` to emit
/// JSON-formatted logs (useful in the container image).
pub fn init(env_filter: Option<&str>, json: bool) -> Result<()> {
    let filter = env_filter
        .map(str::to_owned)
        .or_else(|| std::env::var("EMBER_LOG").ok())
        .or_else(|| std::env::var("RUST_LOG").ok())
        .unwrap_or_else(|| "ember=info,tower_http=info".to_string());
    let filter = EnvFilter::try_new(&filter)
        .map_err(|e| Error::Config(format!("invalid log filter `{filter}`: {e}")))?;

    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::CLOSE)
        .with_target(false);
    if json {
        builder
            .json()
            .try_init()
            .map_err(|e| Error::Config(format!("tracing: {e}")))?;
    } else {
        builder
            .try_init()
            .map_err(|e| Error::Config(format!("tracing: {e}")))?;
    }

    install_recorder()?;
    Ok(())
}

/// Register histogram/counter descriptions once at first use.
pub fn describe() {
    metrics::describe_histogram!(
        "ember.invoke.duration_us",
        metrics::Unit::Microseconds,
        "wall-clock time from request receipt to component response"
    );
    metrics::describe_counter!(
        "ember.invoke.total",
        metrics::Unit::Count,
        "invocations delivered to component handlers"
    );
    metrics::describe_counter!(
        "ember.invoke.errors",
        metrics::Unit::Count,
        "invocations that failed inside the component or the sandbox"
    );
    metrics::describe_counter!(
        "ember.reload.parked",
        metrics::Unit::Count,
        "tenants parked after exhausting the pull failure budget"
    );
    metrics::describe_gauge!(
        "ember.tenant.active_digests",
        metrics::Unit::Count,
        "1 while a tenant has an active component digest"
    );
}

/// Render the Prometheus text exposition format for the `/metrics` route.
pub fn render_metrics() -> String {
    describe();
    install_recorder()
        .map(|h| h.render())
        .unwrap_or_else(|_| "# ember metrics unavailable\n".to_string())
}

/// Record one invocation outcome.
pub fn record_invoke(digest: &str, duration: std::time::Duration, status: u16) {
    describe();
    metrics::histogram!(
        "ember.invoke.duration_us",
        duration.as_micros() as f64,
        "digest" => digest.to_string()
    );
    metrics::counter!(
        "ember.invoke.total",
        1,
        "digest" => digest.to_string(),
        "status" => status.to_string()
    );
    if status >= 500 {
        metrics::counter!(
            "ember.invoke.errors",
            1,
            "digest" => digest.to_string()
        );
    }
}

/// Record that a tenant hit its consecutive-failure budget and was parked.
pub fn record_parked(tenant: &str, failures: usize) {
    describe();
    metrics::counter!(
        "ember.reload.parked",
        1,
        "tenant" => tenant.to_string(),
        "failures" => failures.to_string()
    );
}

/// Record that a tenant switched to a new component digest.
pub fn record_reload(tenant: &str, from: Option<&str>, to: &str) {
    describe();
    metrics::counter!(
        "ember.reload.total",
        1,
        "tenant" => tenant.to_string(),
        "from" => from.unwrap_or("none").to_string(),
        "to" => to.to_string()
    );
    metrics::gauge!(
        "ember.tenant.active_digests",
        1.0,
        "tenant" => tenant.to_string(),
        "digest" => to.to_string()
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_installs_once() {
        let _ = init(Some("ember=error"), false);
        let first = render_metrics();
        let second = render_metrics();
        assert!(first.contains("ember.invoke.total"));
        assert_eq!(first, second, "recorder handle must be stable");
    }

    #[test]
    fn invalid_filter_rejected() {
        assert!(init(Some("not-an-encoded-filter=="), false).is_err());
    }
}
