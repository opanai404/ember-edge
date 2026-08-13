// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Host-side component linker and bindings.
//!
//! [`bindings`] is generated from `wit/ember.wit` at compile time by the
//! `wasmtime::component::bindgen!` macro. It provides the `ember-guest`
//! world host struct ([`EmberGuest`]) plus the [`imports::Tenant`] and
//! [`imports::Telemetry`] traits the runtime implements.
//!
//! [`build_linker`] assembles the linker the runtime instantiates components
//! with: the WASI preview 2 root, the `wasi:http` surface, and Ember's host
//! imports — all reading state out of the per-tenant [`HostCtx`] stored in
//! the `wasmtime::Store`.

use std::sync::Arc;

use wasmtime::Engine;
use wasmtime::component::{Linker, bindgen};

use crate::error::{Error, Result};
use crate::http::bridge::HttpBridge;
use crate::runtime::TenantId;
use crate::wasi::sandbox::{MemFs, Sandbox};

bindgen!({
    path: "wit",
    world: "ember-guest",
    async: true,
    with: {
        "wasi:io/error@0.2.0": wasmtime_wasi::bindings::io::error,
        "wasi:io/streams@0.2.0": wasmtime_wasi::bindings::io::streams,
        "wasi:clocks/monotonic-clock@0.2.0": wasmtime_wasi::bindings::clocks::monotonic_clock,
        "wasi:clocks/wall-clock@0.2.0": wasmtime_wasi::bindings::clocks::wall_clock,
        "wasi:filesystem/types@0.2.0": wasmtime_wasi::bindings::filesystem::types,
        "wasi:filesystem/preopens@0.2.0": wasmtime_wasi::bindings::filesystem::preopens,
        "wasi:random/random@0.2.0": wasmtime_wasi::bindings::random::random,
        "wasi:http/types@0.2.0": wasmtime_wasi_http::bindings::wasi::http::types,
        "wasi:http/outgoing-handler@0.2.0": wasmtime_wasi_http::bindings::wasi::http::outgoing_handler,
    }
});

/// State stored in every tenant's `wasmtime::Store`. The linker reads its
/// WASI and Ember imports out of this value.
#[derive(Debug)]
pub struct HostCtx {
    /// Tenant identity.
    pub tenant: TenantId,
    /// Sandbox ceilings and scratch storage.
    pub sandbox: Sandbox,
    /// Tenant-scoped configuration for `ember:runtime/tenant`.
    pub config: Arc<std::collections::HashMap<String, String>>,
    /// WASI preview 2 context.
    pub wasi: wasmtime_wasi::WasiCtx,
    /// `wasi:http` context (egress).
    pub http: wasmtime_wasi_http::WasiHttpCtx,
    /// Shared outbound bridge used for egress accounting.
    pub egress: Arc<HttpBridge>,
}

impl HostCtx {
    /// Build a fresh context for `sandbox` from its WASI surface.
    pub fn new(sandbox: &Sandbox, egress: Arc<HttpBridge>) -> Result<Self> {
        let wasi = crate::wasi::Wasi::new(sandbox, egress.clone())?;
        Ok(Self {
            tenant: sandbox.tenant.clone(),
            sandbox: sandbox.clone(),
            config: Arc::new(sandbox.config.clone()),
            wasi: wasi.ctx,
            http: wasi.http,
            egress,
        })
    }
}

/// Assemble the linker used to instantiate every component.
pub fn build_linker(engine: &Engine) -> Result<Linker<HostCtx>> {
    let mut linker = Linker::<HostCtx>::new(engine);
    wasmtime_wasi::add_to_linker_sync(&mut linker)?;
    wasmtime_wasi_http::add_to_linker(&mut linker)?;
    EmberGuest::add_to_linker(&mut linker, |ctx: &mut HostCtx| ctx)?;
    Ok(linker)
}

// ── Host implementations of `ember:runtime` imports ────────────

impl imports::Tenant for HostCtx {
    async fn tenant_id(&mut self) -> wasmtime::Result<String> {
        Ok(self.tenant.to_string())
    }

    async fn get_config(&mut self, key: String) -> wasmtime::Result<Option<String>> {
        Ok(self.config.get(&key).cloned())
    }

    async fn scratch_write(
        &mut self,
        path: String,
        data: Vec<u8>,
    ) -> wasmtime::Result<std::result::Result<(), ScratchError>> {
        let outcome = match self.scratch().write(std::path::Path::new(&path), data) {
            Ok(()) => Ok(()),
            Err(Error::LimitExceeded(_)) => Err(ScratchError::QuotaExceeded),
            Err(_) => Err(ScratchError::InvalidPath),
        };
        Ok(outcome)
    }

    async fn scratch_read(&mut self, path: String) -> wasmtime::Result<Option<Vec<u8>>> {
        Ok(self.scratch().read(std::path::Path::new(&path)))
    }

    async fn scratch_bytes(&mut self) -> wasmtime::Result<u64> {
        Ok(self.scratch().used_bytes())
    }
}

impl HostCtx {
    /// The tenant's accounted scratch filesystem.
    fn scratch(&self) -> &MemFs {
        &self.sandbox.scratch
    }
}

impl imports::Telemetry for HostCtx {
    async fn emit(&mut self, level: LogLevel, message: String) -> wasmtime::Result<()> {
        let target = format!("ember::guest::{}", self.tenant);
        match level {
            LogLevel::Debug => tracing::debug!(target = target.as_str(), "{message}"),
            LogLevel::Info => tracing::info!(target = target.as_str(), "{message}"),
            LogLevel::Warn => tracing::warn!(target = target.as_str(), "{message}"),
            LogLevel::Error => tracing::error!(target = target.as_str(), "{message}"),
        }
        Ok(())
    }
}

// ── Request/response conversion at the ABI boundary ────────────

impl TryFrom<http::Request<bytes::Bytes>> for Request {
    type Error = http::Error;

    /// Normalize an HTTP request into the component ABI [`Request`] record,
    /// preserving multi-valued headers as repeated key/value pairs.
    fn try_from(req: http::Request<bytes::Bytes>) -> std::result::Result<Self, Self::Error> {
        let headers = req
            .headers()
            .iter()
            .flat_map(|(name, value)| {
                value
                    .to_str()
                    .map(|v| (name.as_str().to_string(), v.to_string()))
                    .ok()
            })
            .collect::<Vec<_>>();

        let (path, query) = match req.uri().query() {
            Some(q) => (req.uri().path().to_string(), Some(q.to_string())),
            None => (req.uri().path().to_string(), None),
        };

        Ok(Request {
            method: req.method().to_string(),
            path,
            query,
            headers,
            body: Some(req.into_body().to_vec()),
        })
    }
}

impl From<Response> for http::Response<axum::body::Body> {
    /// Materialize a component ABI [`Response`] record as an HTTP response.
    fn from(resp: Response) -> Self {
        let mut builder = http::Response::builder()
            .status(resp.status)
            .expect("component response status is a valid HTTP status");
        for (name, value) in resp.headers {
            if let (Ok(name), Ok(value)) = (
                http::header::HeaderName::from_bytes(name.as_bytes()),
                http::header::HeaderValue::from_str(&value),
            ) {
                builder = builder.header(name, value).unwrap();
            }
        }
        builder
            .body(axum::body::Body::from(resp.body))
            .unwrap_or_else(|_| http::Response::new(axum::body::Body::from(Vec::new())))
    }
}
