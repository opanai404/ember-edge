<!--
  EMBER · WebAssembly edge runtime for the component model
  SPDX-License-Identifier: MIT
-->

# API reference

Two surfaces are documented here: the **HTTP contract** external callers
talk to, and the **public Rust API** Rust integrators build against.

## HTTP contract

### Routes

| Route | Method | Purpose |
|-------|--------|---------|
| `/healthz` | GET | Liveness. Always 200 with body `ok` while the process is up. |
| `/metrics` | GET | Prometheus text exposition (v0.0.4). |
| `/{tenant}/*` | any | Dispatch to the named tenant's component. |

### Request dispatch

- The first path segment is the tenant id (`[a-zA-Z0-9-_]{1,64}`); the
  remainder is passed through as the component's request path.
- The request body is collected up to `http.max_body_bytes`; larger requests
  are refused before dispatch.
- `x-request-id` is generated when absent and echoed on the response. The
  header name is configurable via `http.request_id_header`.
- Component responses map 1:1 onto HTTP: status, headers, body. Response
  headers the host cannot materialize are dropped.

### Errors

Errors are returned as `application/json` with a stable shape:

```json
{ "error": "tenant not found: ghost", "status": 404 }
```

| Condition | Status |
|-----------|--------|
| Tenant not found / component not found | 404 |
| Invalid component / contract violation | 400 |
| Digest mismatch (pin) | 409 |
| Sandbox limit exceeded (timeout, quota, egress 403) | 429 |
| Engine, registry, validation failures | 500 |

Panics inside the component or the tower stack are converted to 500 by the
`CatchPanicLayer`.

## Metrics

| Metric | Type | Labels |
|--------|------|--------|
| `ember.invoke.total` | counter | `digest`, `status` |
| `ember.invoke.errors` | counter | `digest` |
| `ember.invoke.duration_us` | histogram | `digest` |
| `ember.reload.total` | counter | `tenant`, `from`, `to` |
| `ember.reload.parked` | counter | `tenant`, `failures` |
| `ember.tenant.active_digests` | gauge | `tenant`, `digest` |

## Public Rust API

The crate is `ember` (package `ember-edge`). Entry point:

```rust
let config = ember::Config::load(Some(std::path::Path::new("config.toml")))?;
let runtime = ember::Runtime::build(config).await?;
runtime.serve(runtime.bind).await?;
```

### Core types

| Type | Notes |
|------|-------|
| `Runtime` | Owns the engine, linker, tenant registry, and reload loop. Implements `http::ingress::Dispatcher`. |
| `TenantId` | Validated tenant identifier (newtype over `String`). |
| `Config` / `TenantConfig` / `EngineConfig` / `RegistryConfig` / `HttpConfig` | Serde-configurable; `Config::load` validates and clamps tenant limits. |
| `Sandbox` / `SandboxLimits` / `MemFs` | Per-tenant isolation: limits, egress policy, byte-quota scratch. |
| `Loaded` | Validated, content-addressed component artifact. |
| `Registry` / `OciClient` / `ContentStore` / `PinSet` / `HotReloadPolicy` | Distribution and reload governance. |
| `Error` | Single error type with an `http`-status mapping (`Error::status`). |

### Extension points

- **Ingress.** Implement `http::ingress::Dispatcher`
  (`async fn dispatch(&self, tenant, request) -> Result<Response, Error>`)
  and mount it in a `RuntimeState`. The production implementation is
  `Runtime` itself.
- **Egress.** Implement `wasmtime_wasi::http::HttpClient` to replace
  `HttpBridge` (e.g. an outbound proxy, a mock, or a carrier gateway).
- **Module source.** `ComponentSource::Oci` and `ComponentSource::File` are
  the two built-ins; the `Registry` facade is the seam for adding others.

### Concurrency model

- Tenants are read under a `parking_lot::RwLock`; the map is mutated only at
  activation time.
- The active component is an `ArcSwap` — lock-free reads on the hot path.
- Per-tenant instance pools use a `tokio::sync::Mutex<VecDeque<Prepared>>`;
  `Store<HostCtx>` is `Send`, so moving a prepared instance into the current
  task is sound.
- Reload and epoch-tick loops run on the same tokio runtime and abort when
  the runtime is dropped.
