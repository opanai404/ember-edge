// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! The runtime facade: engine, linker, tenant registry, and hot reload.
//!
//! [`Runtime`] owns everything needed to serve a tenant's traffic and is
//! assembled by [`Runtime::build`] from a [`Config`]. Tenants are held in a
//! registry; each tenant atomically points at its active component via an
//! [`ArcSwap`], so a hot reload never blocks an in-flight request. The
//! background reload loop re-resolves OCI references on an interval and swaps
//! in new digests subject to [`HotReloadPolicy`].

pub mod engine;
pub mod invoke;

use std::collections::HashMap;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::time::MissedTickBehavior;
use wasmtime::Engine;
use wasmtime::component::{Component, Linker};

use crate::component::linker::HostCtx;
use crate::component::linker::build_linker;
use crate::component::loader::{Loaded, ensure_contract};
use crate::config::{ComponentSource, Config, TenantConfig};
use crate::error::{Error, Result};
use crate::http::bridge::HttpBridge;
use crate::http::ingress::{Dispatcher, RuntimeState};
use crate::registry::{HotReloadPolicy, Registry};
use crate::runtime::engine::EngineBuilder;
use crate::runtime::invoke::InstancePool;
use crate::wasi::sandbox::Sandbox;
use crate::{MAX_TENANT_ID_LEN, telemetry};

/// A tenant id: lowercase-alphanumeric plus `-` and `_`, up to
/// [`MAX_TENANT_ID_LEN`] characters. Also usable directly as a URL path
/// segment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TenantId(String);

impl TenantId {
    /// Validate and construct a tenant id.
    pub fn new(name: &str) -> Result<Self, Error> {
        if name.is_empty() {
            return Err(Error::Config("tenant id must not be empty".into()));
        }
        if name.len() > MAX_TENANT_ID_LEN {
            return Err(Error::Config(format!(
                "tenant id `{name}` exceeds {MAX_TENANT_ID_LEN} characters"
            )));
        }
        if !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(Error::Config(format!(
                "tenant id `{name}` may only contain [a-zA-Z0-9-_]"
            )));
        }
        Ok(Self(name.to_string()))
    }

    /// Parse a tenant id from a URL path segment.
    pub fn parse(s: &str) -> Result<Self, Error> {
        Self::new(s)
    }

    /// The tenant id as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for TenantId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The component a tenant is currently running, plus its compiled form.
pub struct ActiveComponent {
    /// Validated artifact metadata and bytes.
    pub loaded: Arc<Loaded>,
    /// Compiled component bound to the runtime engine.
    pub compiled: Arc<Component>,
    /// Active content digest (equals `loaded.digest`).
    pub digest: String,
    /// When this generation became active.
    pub activated_at: Instant,
}

/// A registered tenant.
pub struct Tenant {
    /// Unique id, also the ingress route prefix.
    pub id: TenantId,
    /// The tenant's resource sandbox.
    pub sandbox: Sandbox,
    /// The tenant's egress bridge (policy-scoped).
    pub bridge: Arc<HttpBridge>,
    /// Where this tenant's component is sourced from.
    pub source: ComponentSource,
    /// The currently active component; swapped atomically on reload.
    pub active: ArcSwap<ActiveComponent>,
    /// Pool of prepared instances for the active generation.
    pub pool: InstancePool,
    /// Consecutive failed pulls (reload loop bookkeeping).
    pub failures: AtomicUsize,
    /// Invocations delivered since activation.
    pub invocations: AtomicU64,
    /// Time of the last swap, for reload debouncing.
    pub last_swap: parking_lot::Mutex<Instant>,
}

impl fmt::Debug for Tenant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tenant")
            .field("id", &self.id)
            .field("digest", &self.active.load().digest)
            .field("failures", &self.failures.load(Ordering::Relaxed))
            .finish()
    }
}

/// The assembled runtime.
pub struct Runtime {
    /// Shared Wasmtime engine (pooling allocator, epochs).
    pub engine: Engine,
    /// Shared linker (WASI p2 + `wasi:http` + Ember imports).
    pub linker: Arc<Linker<HostCtx>>,
    /// Default outbound bridge for the ingress.
    pub bridge: Arc<HttpBridge>,
    /// Module distribution channel.
    pub registry: Registry,
    /// Ingress tuning.
    pub http: Arc<crate::config::HttpConfig>,
    /// Address the HTTP ingress binds to.
    pub bind: SocketAddr,
    /// Reload governance.
    policy: HotReloadPolicy,
    /// Tenant registry.
    tenants: RwLock<HashMap<TenantId, Arc<Tenant>>>,
    /// Background tasks (epoch ticker, reload loop).
    background: parking_lot::Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

impl fmt::Debug for Runtime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Runtime")
            .field("tenants", &self.tenants.read().keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Runtime {
    /// Assemble a runtime from a validated [`Config`]: engine, linker,
    /// registry, and all configured tenants.
    pub async fn build(config: Config) -> Result<Arc<Self>> {
        let engine = EngineBuilder::from_config(&config.engine).build()?;
        let linker = Arc::new(build_linker(&engine)?);
        let bridge = Arc::new(HttpBridge::new(
            crate::http::bridge::EgressPolicy::default(),
            config.http.max_body_bytes,
        )?);

        let registry = Registry::new(
            crate::registry::OciClientConfig {
                endpoint: config.registry.endpoint.clone(),
                insecure: config.registry.insecure,
                cache_dir: config.registry.cache_dir.clone(),
                manifest_cache_ttl: config.registry.pull_interval(),
            },
            HotReloadPolicy {
                poll_interval: config.registry.pull_interval(),
                ..HotReloadPolicy::default()
            },
        )?;

        let runtime = Arc::new(Self {
            engine,
            linker,
            bridge,
            registry,
            http: Arc::new(config.http.clone()),
            bind: config.bind,
            policy: HotReloadPolicy {
                poll_interval: config.registry.pull_interval(),
                ..HotReloadPolicy::default()
            },
            tenants: RwLock::new(HashMap::new()),
            background: parking_lot::Mutex::new(Vec::new()),
        });

        for tenant in &config.tenants {
            runtime.load_tenant(tenant.clone()).await?;
        }

        runtime.start_background(&config);
        Ok(runtime)
    }

    fn start_background(self: &Arc<Self>, config: &Config) {
        // Epoch ticker drives cooperative time budgets.
        let ticker = EngineBuilder::from_config(&config.engine).spawn_epoch_ticker(&self.engine);
        let reload = self.spawn_reload_loop();
        self.background.lock().push(ticker);
        self.background.lock().push(reload);
    }

    /// Register one tenant and load its initial component.
    pub async fn load_tenant(&self, cfg: TenantConfig) -> Result<()> {
        let id = TenantId::new(&cfg.name)?;
        let sandbox = Sandbox::new(id.clone(), cfg.limits.clone(), cfg.config)?;
        let bridge = Arc::new(HttpBridge::new(
            sandbox.egress_policy(),
            cfg.limits.max_body_bytes,
        )?);

        let loaded = match &cfg.component {
            ComponentSource::Oci { reference, pin } => {
                self.registry.pull(reference, pin.as_deref()).await?
            }
            ComponentSource::File { path } => Loaded::from_path(path)?,
        };
        ensure_contract(&loaded)?;

        let compiled = Arc::new(Component::from_binary(&self.engine, &loaded.bytes)?);
        let digest = loaded.digest.clone();
        let active = Arc::new(ActiveComponent {
            loaded: Arc::new(loaded),
            compiled,
            digest,
            activated_at: Instant::now(),
        });

        let tenant = Arc::new(Tenant {
            id: id.clone(),
            sandbox,
            bridge,
            source: cfg.component,
            active: ArcSwap::from_pointee(active),
            pool: InstancePool::new(cfg.limits.max_instances),
            failures: AtomicUsize::new(0),
            invocations: AtomicU64::new(0),
            last_swap: parking_lot::Mutex::new(Instant::now()),
        });

        self.tenants.write().insert(id.clone(), tenant.clone());
        telemetry::record_reload(&id.to_string(), None, &tenant.active.load().digest);
        tracing::info!(tenant = %id, digest = %tenant.active.load().digest, "tenant activated");
        Ok(())
    }

    /// Look up a tenant by id.
    pub fn tenant(&self, id: &TenantId) -> Option<Arc<Tenant>> {
        self.tenants.read().get(id).cloned()
    }

    /// Snapshot the tenant registry (for metrics and the reload loop).
    pub fn snapshot_tenants(&self) -> Vec<Arc<Tenant>> {
        self.tenants.read().values().cloned().collect()
    }

    /// Number of registered tenants.
    pub fn tenant_count(&self) -> usize {
        self.tenants.read().len()
    }

    /// Swap `tenant` to a freshly validated component, atomically.
    async fn swap(&self, tenant: &Tenant, loaded: Loaded) -> Result<()> {
        let compiled = Arc::new(Component::from_binary(&self.engine, &loaded.bytes)?);
        let digest = loaded.digest.clone();
        let new = Arc::new(ActiveComponent {
            loaded: Arc::new(loaded),
            compiled,
            digest,
            activated_at: Instant::now(),
        });

        let old = tenant.active.swap(new);
        *tenant.last_swap.lock() = Instant::now();
        tenant.failures.store(0, Ordering::Relaxed);

        telemetry::record_reload(
            &tenant.id.to_string(),
            Some(&old.digest),
            &tenant.active.load().digest,
        );
        tracing::info!(
            tenant = %tenant.id,
            from = %old.digest,
            to = %tenant.active.load().digest,
            "component reloaded"
        );
        Ok(())
    }

    /// One reload pass: re-resolve every OCI-sourced tenant and swap when the
    /// digest moved and the debounce has elapsed.
    pub async fn poll_once(&self) -> Result<()> {
        let now = Instant::now();
        let tenants = self.snapshot_tenants();
        for tenant in tenants {
            let ComponentSource::Oci { reference, pin } = &tenant.source else {
                continue;
            };

            // Debounce: at most one swap per tenant per interval window.
            let since_last_swap = now.saturating_duration_since(*tenant.last_swap.lock());
            if since_last_swap < self.policy.debounce {
                continue;
            }

            match self.registry.pull(reference, pin.as_deref()).await {
                Ok(loaded) => {
                    ensure_contract(&loaded)?;
                    let current = tenant.active.load().digest.clone();
                    if self.policy.should_reload(Some(&current), &loaded.digest) {
                        self.swap(&tenant, loaded).await?;
                    }
                    tenant.failures.store(0, Ordering::Relaxed);
                }
                Err(e) => {
                    let failures = tenant.failures.fetch_add(1, Ordering::Relaxed) + 1;
                    if self.policy.exhausted(failures) {
                        telemetry::record_parked(&tenant.id.to_string(), failures);
                        tracing::error!(
                            tenant = %tenant.id,
                            error = %e,
                            "tenant parked after {} failed pulls; keeping active digest",
                            failures
                        );
                    } else {
                        tracing::warn!(tenant = %tenant.id, error = %e, "pull failed; will retry");
                    }
                }
            }
        }
        Ok(())
    }

    /// Spawn the background reload loop.
    pub fn spawn_reload_loop(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let runtime = self.clone();
        let interval = self.policy.poll_interval;
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if let Err(e) = runtime.poll_once().await {
                    tracing::warn!(error = %e, "hot reload pass failed");
                }
            }
        })
    }

    /// Serve the HTTP ingress until a shutdown signal arrives.
    pub async fn serve(self: Arc<Self>, bind: SocketAddr) -> Result<()> {
        let state = RuntimeState {
            dispatcher: self.clone(),
            bridge: self.bridge.clone(),
            config: self.http.clone(),
        };
        crate::http::ingress::serve(state, bind).await
    }
}

#[async_trait]
impl Dispatcher for Runtime {
    async fn dispatch(
        &self,
        tenant: TenantId,
        request: http::Request<bytes::Bytes>,
    ) -> Result<axum::response::Response, Error> {
        crate::runtime::invoke::invoke(self, tenant, request).await
    }
}
