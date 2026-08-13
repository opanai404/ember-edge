// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! HTTP ingress.
//!
//! The ingress is a small axum/tower stack that maps `GET /{tenant}/*` (and
//! any other method) onto the runtime's dispatcher. Tenant resolution,
//! sandbox enforcement, and component invocation live behind the
//! [`Dispatcher`] trait so the router is fully testable with a mock
//! dispatcher and so alternative ingresses (gRPC-web, WebSocket) can reuse
//! the same boundary.
//!
//! The tower layers are, in order: request-id generation/propagation,
//! `tracing`, catch-panic → 500, per-request timeout, and a concurrency
//! ceiling. All of them are global shaping knobs; per-tenant budgets are
//! enforced inside the dispatcher.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderName, Request as HttpRequest, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{any, get};
use http_body_util::BodyExt;
use tower::ServiceBuilder;
use tower::limit::ConcurrencyLimitLayer;
use tower::timeout::TimeoutLayer;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use crate::config::HttpConfig;
use crate::error::{Error, Result};
use crate::http::bridge::HttpBridge;
use crate::runtime::TenantId;
use crate::telemetry;

/// Abstraction the ingress dispatches requests through. The production
/// implementation is [`crate::runtime::Runtime`]; tests inject a mock.
#[async_trait]
pub trait Dispatcher: Send + Sync {
    /// Deliver one fully-collected request to the tenant's component and
    /// return the component's response.
    async fn dispatch(
        &self,
        tenant: TenantId,
        request: HttpRequest<Bytes>,
    ) -> Result<Response, Error>;
}

/// Shared state mounted on the axum router.
#[derive(Clone)]
pub struct RuntimeState {
    /// The dispatcher that owns tenants, components, and sandboxes.
    pub dispatcher: Arc<dyn Dispatcher>,
    /// The shared outbound HTTP bridge.
    pub bridge: Arc<HttpBridge>,
    /// Ingress tuning.
    pub config: Arc<HttpConfig>,
}

/// Build the axum router for a runtime state.
pub fn router(state: RuntimeState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/metrics", get(metrics))
        .route("/{tenant}/*rest", any(dispatch))
        .layer(
            ServiceBuilder::new()
                .layer(SetRequestIdLayer::new(
                    HeaderName::from_static("x-request-id"),
                    MakeRequestUuid,
                ))
                .layer(PropagateRequestIdLayer::new(HeaderName::from_static(
                    "x-request-id",
                )))
                .layer(TraceLayer::new_for_http())
                .layer(CatchPanicLayer::new())
                .layer(TimeoutLayer::new(state.config.timeout()))
                .layer(ConcurrencyLimitLayer::new(state.config.concurrency)),
        )
        .with_state(state)
}

/// Bind the ingress to `bind` and serve until a shutdown signal arrives.
pub async fn serve(state: RuntimeState, bind: SocketAddr) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .map_err(Error::Io)?;
    let local = listener.local_addr().map_err(Error::Io)?;
    tracing::info!(%local, "ember ingress listening");
    axum::serve(listener, router(state).into_make_service())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|e| Error::Io(std::io::Error::other(e)))
}

async fn healthz() -> &'static str {
    "ok"
}

async fn metrics() -> impl IntoResponse {
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        telemetry::render_metrics(),
    )
}

/// Route one tenant request into the dispatcher.
async fn dispatch(
    State(state): State<RuntimeState>,
    Path(tenant): Path<String>,
    request: HttpRequest<Body>,
) -> Result<Response, IngressError> {
    // Destructure before reading the body so the parts survive the move.
    let (parts, body) = request.into_parts();
    let collected = body
        .limited(state.config.max_body_bytes)
        .collect()
        .await
        .map_err(|e| Error::Http(http::Error::from(e)))
        .map_err(IngressError)?;
    let bytes = collected.to_bytes();

    let mut builder = HttpRequest::builder();
    builder = builder.method(parts.method).map_err(Error::Http)?;
    builder = builder.uri(parts.uri).map_err(Error::Http)?;
    for (name, value) in &parts.headers {
        builder = builder.header(name, value).unwrap();
    }
    let converted = builder.body(bytes).map_err(Error::Http)?;

    let tenant_id = TenantId::parse(&tenant)?;
    let response = state.dispatcher.dispatch(tenant_id, converted).await?;
    Ok(response)
}

/// Error type for the ingress that maps [`Error`] onto HTTP responses.
#[derive(Debug)]
pub struct IngressError(Error);

impl From<Error> for IngressError {
    fn from(e: Error) -> Self {
        Self(e)
    }
}

impl IntoResponse for IngressError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = Json(serde_json::json!({
            "error": self.0.to_string(),
            "status": status.as_u16(),
        }));
        (status, body).into_response()
    }
}

/// Wait for Ctrl-C or SIGTERM, then stop accepting requests.
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received, draining in-flight requests");
}
