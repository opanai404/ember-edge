# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-08

### Added

- **Component model runtime** built on Wasmtime: `ember-guest` world contract
  (`wit/ember.wit`), host-side `bindgen!` bindings, and a cached instance pool
  that reuses prepared `Store`/instance pairs per tenant and digest.
- **WASI preview 2** context construction (`src/wasi.rs`) with stdio
  inheritance, tenant environment, read-only preopens, and `wasi:http`
  outbound support wired through `wasmtime-wasi-http`.
- **Per-tenant sandboxing** (`src/wasi/sandbox.rs`): memory / instance /
  epoch / wall-clock / body-size limits, an in-memory scratch filesystem with
  per-tenant byte quota, and an egress allowlist with wildcard matching.
- **OCI registry client** (`src/registry/oci.rs`) against the OCI
  distribution spec: reference resolution, bearer-token auth, manifest and
  blob pulls, and `sha256` digest verification.
- **Content-addressable store** (`src/registry/store.rs`) with atomic writes
  and a pinned-digest index, plus a hot-reload policy (poll interval, debounce,
  failure budget) used by the runtime's reload loop.
- **Pluggable HTTP ingress** (`src/http/ingress.rs`) built on axum/tower:
  `/{tenant}` routing, request-id, tracing, catch-panic, timeout and
  concurrency-limit layers, plus `/healthz` and `/metrics` endpoints.
- **Egress bridge** (`src/http/bridge.rs`): a `wasmtime-wasi` `HttpClient`
  backed by reqwest with connection pooling and allowlist enforcement.
- **Runtime facade** (`src/runtime.rs`): engine + linker assembly, tenant
  registry, atomic component swap via `ArcSwap`, and a background reload loop.
- **Telemetry** (`src/telemetry.rs`): `tracing` logging and a Prometheus
  recorder exposed on `/metrics`.

### Documentation

- Architecture, getting-started, component-contract, hot-reload design, and
  API reference docs under `docs/`.

### Examples

- `examples/minimal.rs` — single-tenant local serve.
- `examples/pull_and_reload.rs` — OCI pull plus digest-driven hot reload.
- `examples/tenants.rs` — multi-tenant sandbox isolation walkthrough.

[0.1.0]: https://github.com/opanai404/ember-edge/releases/tag/v0.1.0
