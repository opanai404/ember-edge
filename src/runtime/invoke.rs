// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Component invocation and per-tenant instance pooling.
//!
//! Each tenant keeps a bounded pool of **prepared** instances — a
//! `wasmtime::Store<HostCtx>` plus its `EmberGuest` handle — keyed by
//! component digest. A request that finds a matching prepared instance skips
//! instantiation entirely; a request for a newer digest lazily discards the
//! stale instances and instantiates fresh. Pools make the warm path
//! allocation-free while hot reload stays costless: swap the `ArcSwap`
//! digest, and the next request builds the new instance.

use std::collections::VecDeque;
use std::time::Instant;

use axum::body::Body;
use bytes::Bytes;
use tokio::sync::Mutex;
use tokio::time::timeout;
use wasmtime::Store;

use crate::component::linker::{self, HostCtx, Request};
use crate::component::loader::Loaded;
use crate::error::{Error, Result};
use crate::runtime::Runtime;
use crate::runtime::engine::EngineCap;
use crate::telemetry;

/// A prepared (store, guest) pair that can serve one request without any
/// instantiation work.
struct Prepared {
    /// Digest of the component this instance was built from.
    digest: String,
    /// The store, holding the tenant's `HostCtx`.
    store: Store<HostCtx>,
    /// The guest entrypoint bound to `store`.
    guest: linker::EmberGuest,
}

/// Per-tenant pool of prepared instances.
#[derive(Debug)]
pub struct InstancePool {
    inner: Mutex<VecDeque<Prepared>>,
    cap: usize,
}

impl InstancePool {
    /// A pool that holds at most `cap` prepared instances.
    pub fn new(cap: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::new()),
            cap: cap.max(1),
        }
    }

    /// Take a prepared instance matching `loaded.digest`, or build one.
    ///
    /// Stale-digest instances encountered while scanning are dropped rather
    /// than returned, so a reload naturally drains the old generation.
    async fn acquire(
        &self,
        engine: &wasmtime::Engine,
        linker: &wasmtime::component::Linker<HostCtx>,
        compiled: &wasmtime::component::Component,
        loaded: &Loaded,
        ctx: HostCtx,
    ) -> Result<Prepared> {
        let mut queue = self.inner.lock().await;
        while let Some(p) = queue.pop_front() {
            if p.digest == loaded.digest {
                return Ok(p);
            }
            // Stale instance: fall through and let it drop.
        }

        let mut store = Store::new(engine, ctx);
        store.set_epoch_deadline(store.data().sandbox.limits.max_epoch_delta.max(1));
        let (guest, _) = linker::EmberGuest::instantiate(&mut store, linker, compiled).await?;
        Ok(Prepared {
            digest: loaded.digest.clone(),
            store,
            guest,
        })
    }

    /// Return a used instance to the pool if there is room.
    async fn release(&self, prepared: Prepared) {
        let mut queue = self.inner.lock().await;
        if queue.len() < self.cap {
            queue.push_back(prepared);
        }
    }
}

/// Deliver `request` to `tenant`'s active component and return its response.
///
/// Enforcement points, in order: tenant existence, wall-clock budget
/// (`timeout`), epoch budget (deadline set on the store), and the component's
/// own contract. Errors are mapped to [`Error`] so the ingress can render
/// them; the single success path records invocation metrics.
pub async fn invoke(
    runtime: &Runtime,
    tenant_id: crate::runtime::TenantId,
    request: http::Request<Bytes>,
) -> Result<http::Response<Body>> {
    let tenant = runtime
        .tenant(&tenant_id)
        .ok_or_else(|| Error::TenantNotFound(tenant_id.to_string()))?;
    let active = tenant.active.load_full();
    let started = Instant::now();

    let guest_request = Request::try_from(request).map_err(Error::Http)?;

    let mut prepared = tenant
        .pool
        .acquire(
            &runtime.engine,
            &runtime.linker,
            &active.compiled,
            &active.loaded,
            HostCtx::new(&tenant.sandbox, tenant.bridge.clone())?,
        )
        .await?;

    // The component call runs under the tenant's wall-clock budget. The
    // epoch deadline is already armed on the store; hitting it surfaces as a
    // `wasmtime` trap converted below.
    let call = async {
        let handler = prepared.guest.handler(&mut prepared.store)?;
        handler
            .call_handle(&mut prepared.store, guest_request)
            .await
    };

    let result = timeout(tenant.sandbox.limits.max_wall_duration, call).await;

    let response = match result {
        Ok(Ok(resp)) => {
            tenant.pool.release(prepared).await;
            http::Response::from(resp)
        }
        Ok(Err(e)) => return Err(Error::Wasm(e)),
        Err(_elapsed) => {
            return Err(Error::LimitExceeded(format!(
                "tenant `{tenant_id}` exceeded its {}ms wall-clock budget",
                tenant.sandbox.limits.max_wall_duration.as_millis()
            )));
        }
    };

    tenant
        .invocations
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let status = response.status().as_u16();
    telemetry::record_invoke(&active.digest, started.elapsed(), status);
    Ok(response)
}

/// Reconcile engine capacity against a pool's requested size.
pub fn pool_capacity(limits: &crate::wasi::sandbox::SandboxLimits, cap: &EngineCap) -> usize {
    limits.max_instances.min(cap.max_instances)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_capacity_is_bounded_by_engine() {
        let limits = crate::wasi::sandbox::SandboxLimits::default();
        let cap = EngineCap {
            max_memory_bytes: 1 << 30,
            max_instances: 4,
            threads: 1,
        };
        assert_eq!(pool_capacity(&limits, &cap), 4);
    }

    #[test]
    fn pool_never_creates_zero_capacity() {
        let pool = InstancePool::new(0);
        assert_eq!(pool.cap, 1);
    }
}
