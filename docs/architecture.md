<!--
  EMBER · WebAssembly edge runtime for the component model
  SPDX-License-Identifier: MIT
-->

# Architecture

Ember is a small Wasmtime-based runtime for edge workloads. This document
walks the module map, the request path, and the hot-reload flow.

## Module map

```
                            ┌───────────────────────────────────────────────────┐
                            │ ember-edge (crate root: lib.rs / main.rs)         │
                            │   config.rs · error.rs · telemetry.rs              │
                            └───────┬───────────────────────────────┬───────────┘
                                    │                               │
               ┌────────────────────▼───────────────┐   ┌──────────▼─────────────┐
               │ component/                          │   │ runtime/                │
               │  loader.rs   validate bytes,        │   │  engine.rs  pooling,    │
               │              digest, contract       │   │             epochs      │
               │  linker.rs   bindgen! + Linker      │   │  invoke.rs  instance    │
               │              (WASI p2 + wasi:http + │   │             pool + call │
               │              ember:runtime)         │   └───────┬────────┬───────┘
               └────────────────────┬───────────────┘           │        │
                                    │ instantiate                │        │ swap
               ┌────────────────────▼───────────────┐   ┌────────▼────────▼───────┐
               │ wasi/                               │   │ registry/               │
               │  sandbox.rs  limits · MemFs scratch │   │  oci.rs   distribution  │
               │              · egress allowlist     │   │  store.rs  CAS + PinSet │
               │  (mod)       WasiCtx construction   │   │           + reload      │
               └────────────────────┬───────────────┘   └──────────┬──────────────┘
                                    │ read ctx out of Store         │ poll / pin
               ┌────────────────────▼───────────────┐   ┌──────────▼─────────────┐
               │ http/                               │   │ wit/ember.wit           │
               │  ingress.rs  axum router + tower    │   │  the ember-guest world  │
               │  bridge.rs   wasi:http egress       │   └────────────────────────┘
               └─────────────────────────────────────┘
```

Ownership arrows:

- `runtime/` owns the `Engine`, the `Linker`, and the tenant registry. It is
  the only module that can swap a tenant's active component.
- `component/linker.rs` owns the ABI boundary. It is the only place where
  `bindgen!` output and WASI p2 host state are wired together.
- `registry/` owns bytes (OCI and the content store). `runtime/` owns live
  state (instances, stores).
- `http/ingress.rs` owns the inbound path up to the `Dispatcher` trait;
  `runtime/` implements that trait, which is what keeps the ingress
  pluggable.

## Request path

1. A request arrives at the ingress: `GET /{tenant}/*rest`.
2. Tower applies request-id, tracing, catch-panic, timeout, and the
   concurrency ceiling.
3. The handler collects the body (bounded by `http.max_body_bytes`), converts
   it to the component ABI `Request` record, and resolves the tenant id.
4. The dispatcher forwards to `runtime::invoke::invoke`, which looks up the
   tenant, loads its active `ArcSwap<ActiveComponent>`, and asks the tenant's
   `InstancePool` for a prepared `Store<HostCtx>` matching the active digest.
5. The handler is called under a wall-clock `timeout`. The store's epoch
   deadline is already armed; the engine's background ticker enforces the
   cooperative compute budget.
6. The ABI `Response` record is materialized as an `http::Response`, metrics
   are recorded, and the store returns to the pool.

## Hot reload

1. The reload loop ticks every `registry.pull_interval_secs`.
2. For each OCI-sourced tenant it re-resolves the reference and pulls the
   artifact, then verifies the digest against the pin (if any) and the
   `ember-guest` contract.
3. If the digest changed and the debounce has elapsed, `Runtime::swap`
   compiles the new component and atomically replaces the `ActiveComponent`
   via `ArcSwap`. The next request builds a fresh instance; in-flight
   requests finish on the old generation.
4. Pull failures are counted per tenant; after `max_failed_polls` the tenant
   is parked on its last known good digest and the condition is surfaced in
   the logs and `/metrics`.

See `design/hot-reload.md` for the failure-mode analysis behind this design.

## Sandbox model

Ember gives a tenant a *host-accounted* sandbox rather than a raw WASI
filesystem preopen:

- Guest linear memory, tables, and instance counts are bounded by the pooled
  allocator (configured per tenant in `SandboxLimits` and clamped into the
  engine budget at startup).
- Compute is bounded cooperatively (epochs) and absolutely (wall clock).
- State that must survive across invocations lives in `MemFs`, a byte-quota'd
  in-memory store reached through `ember:runtime/tenant` — never a shared
  host directory.
- Outbound network is deny-by-default through `EgressPolicy`; the egress
  bridge refuses disallowed authorities before any connection is made.
