// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! End-to-end HTTP ingress tests against a mock dispatcher.
//!
//! The ingress routes through the [`Dispatcher`] trait, so these tests cover
//! the full tower stack (routing, request-id, timeout, catch-panic, error
//! mapping) without a real component.

use std::sync::Arc;

use async_trait::async_trait;
use axum::body::{Body, Bytes, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use ember::config::HttpConfig;
use ember::error::Error;
use ember::http::bridge::{EgressPolicy, HttpBridge};
use ember::http::ingress::{Dispatcher, RuntimeState, router};
use ember::runtime::TenantId;

#[derive(Clone)]
struct MockDispatcher {
    inner: Arc<std::sync::Mutex<MockBehavior>>,
}

#[derive(Default)]
struct MockBehavior {
    responses: std::collections::HashMap<String, (StatusCode, &'static str)>,
    errors: std::collections::HashMap<String, Error>,
    panic: bool,
}

impl MockDispatcher {
    fn new() -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(MockBehavior::default())),
        }
    }

    fn with_response(mut self, tenant: &str, status: StatusCode, body: &'static str) -> Self {
        self.inner
            .lock()
            .unwrap()
            .responses
            .insert(tenant.to_string(), (status, body));
        self
    }

    fn with_error(mut self, tenant: &str, error: Error) -> Self {
        self.inner
            .lock()
            .unwrap()
            .errors
            .insert(tenant.to_string(), error);
        self
    }

    fn with_panic(mut self) -> Self {
        self.inner.lock().unwrap().panic = true;
        self
    }
}

#[async_trait]
impl Dispatcher for MockDispatcher {
    async fn dispatch(
        &self,
        tenant: TenantId,
        _request: Request<Bytes>,
    ) -> Result<axum::http::Response<Body>, Error> {
        if self.inner.lock().unwrap().panic {
            panic!("mock handler panicked");
        }
        let behavior = self.inner.lock().unwrap();
        if let Some(error) = behavior.errors.get(tenant.as_str()) {
            // Error is not Clone; reconstruct the variant we assert on.
            return Err(match error {
                Error::LimitExceeded(_) => Error::LimitExceeded("out of fuel".into()),
                other => Error::ComponentNotFound(other.to_string()),
            });
        }
        match behavior.responses.get(tenant.as_str()) {
            Some((status, body)) => Ok(axum::http::Response::builder()
                .status(*status)
                .header("x-mock", "1")
                .body(Body::from(*body))
                .unwrap()),
            None => Err(Error::TenantNotFound(tenant.to_string())),
        }
    }
}

fn state(dispatcher: MockDispatcher) -> RuntimeState {
    let bridge = HttpBridge::new(EgressPolicy::default(), 1024 * 1024).unwrap();
    RuntimeState {
        dispatcher: Arc::new(dispatcher),
        bridge: Arc::new(bridge),
        config: Arc::new(HttpConfig::default()),
    }
}

#[tokio::test]
async fn healthz_is_ok() {
    let app = router(state(MockDispatcher::new()));
    let response = app
        .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"ok");
}

#[tokio::test]
async fn metrics_route_serves_text() {
    let app = router(state(MockDispatcher::new()));
    let response = app
        .oneshot(Request::get("/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(content_type.starts_with("text/plain"));
}

#[tokio::test]
async fn unknown_tenant_yields_404_json() {
    let app = router(state(MockDispatcher::new()));
    let response = app
        .oneshot(Request::get("/ghost/ping").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], 404);
    assert!(json["error"].as_str().unwrap().contains("ghost"));
}

#[tokio::test]
async fn known_tenant_dispatches_and_echoes() {
    let dispatcher = MockDispatcher::new().with_response("echo", StatusCode::OK, "pong");
    let app = router(state(dispatcher));
    let response = app
        .oneshot(Request::get("/echo/ping").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["x-mock"], "1");
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"pong");
}

#[tokio::test]
async fn request_id_is_generated_and_propagated() {
    let dispatcher = MockDispatcher::new().with_response("echo", StatusCode::OK, "pong");
    let app = router(state(dispatcher));
    let response = app
        .oneshot(
            Request::post("/echo/things")
                .header("content-type", "text/plain")
                .body(Body::from("payload"))
                .unwrap(),
        )
        .await
        .unwrap();
    let request_id = response
        .headers()
        .get("x-request-id")
        .expect("request id set");
    let _ = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert!(!request_id.is_empty());
}

#[tokio::test]
async fn panicking_dispatcher_yields_500() {
    let app = router(state(MockDispatcher::new().with_panic()));
    let response = app
        .oneshot(Request::get("/echo/boom").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn sandbox_limit_error_maps_to_429() {
    let dispatcher =
        MockDispatcher::new().with_error("throttled", Error::LimitExceeded("fuel".into()));
    let app = router(state(dispatcher));
    let response = app
        .oneshot(Request::get("/throttled/x").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], 429);
}
