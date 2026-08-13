<!--
  EMBER · WebAssembly edge runtime for the component model
  SPDX-License-Identifier: MIT
-->

```text
// EMBER · accent: ember amber #FBBF24
 ██████████████████████████████
 █  ▄▀▄  █▀▄  █▀█  ▄▀▄  █▄▀  █▓
 █  █▀█  █▀▄  █▄█  █▀█  █ █  █▓
 ██████████████████████████████
```

WebAssembly edge runtime for the component model

[![CI](https://img.shields.io/github/actions/workflow/status/opanai404/ember-edge/ci.yml?branch=main&style=flat&logo=github&logoColor=white)](https://github.com/opanai404/ember-edge/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/opanai404/ember-edge?style=flat&color=FBBF24)](LICENSE)
[![Crates.io](https://img.shields.io/crates/v/ember-edge?style=flat&logo=rust&logoColor=white)](https://crates.io/crates/ember-edge)
[![Stars](https://img.shields.io/github/stars/opanai404/ember-edge?style=flat&logo=github&logoColor=white)](https://github.com/opanai404/ember-edge)
[![Language](https://img.shields.io/badge/language-Rust-E6E8EF?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Rust](https://img.shields.io/badge/rust-1.90+-FBBF24?style=flat&logo=rust&logoColor=white)](https://github.com/opanai404/ember-edge/blob/main/rust-toolchain.toml)
[![Wasmtime](https://img.shields.io/badge/wasmtime-34-8B93A7?style=flat)](https://github.com/bytecodealliance/wasmtime)
[![WASI](https://img.shields.io/badge/WASI-preview%202-FBBF24?style=flat)](https://wasi.dev/)
[![Component Model](https://img.shields.io/badge/component--model-v1-8B93A7?style=flat)](https://component-model.bytecodealliance.org/)

## What it is

Ember is a lightweight WebAssembly runtime for edge workloads built on
Wasmtime. It runs [component-model] components under the WASI preview 2
surface, isolates every tenant inside a resource-accounted sandbox,
hot-reloads components from an OCI registry by pinned digest, and exposes a
pluggable HTTP ingress. One process, many tenants, no shared host filesystem,
no shared network.

## Why it matters

Edge compute is a trust problem as much as a latency problem. Classic
serverless isolates a tenant with a process, which is heavyweight, or with a
V8-style embedder, which narrows the language surface to one runtime.
WASI preview 2 + the component model finally make a *portable* guest ABI
viable: one binary format, many languages, a host that can audit exactly
what a guest imports before it runs. Ember leans into that: curated WASI
surface, host-accounted scratch storage instead of raw filesystem preopens,
deny-by-default egress, and content-addressed deploys so a tag can never
ship bytes you did not intend.

## Key features

- **Component model guest contract** (`wit/ember.wit`) with a single
  `handle(request) -> response` entrypoint and host-generated bindings via
  `wasmtime::component::bindgen!`.
- **WASI preview 2** context per tenant: stdio, clocks, random, filesystem
  types, and `wasi:http` outbound — wired through `wasmtime-wasi` /
  `wasmtime-wasi-http`.
- **Per-tenant sandbox** (`wasi/sandbox.rs`): pooled-allocator memory and
  instance ceilings clamped into the engine budget, cooperative epoch
  budgets, per-request wall-clock and body limits, a byte-quota'd in-memory
  scratch filesystem, and a wildcard egress allowlist that denies by default.
- **Hot reload from OCI** (`registry/`): pull by tag or `sha256:` digest with
  layer verification, an atomic content-addressable cache, and a conservative
  reload loop (digest equality, debounce, failure budget) that swaps a
  tenant's active component behind an `ArcSwap` without dropping traffic.
- **Pluggable HTTP ingress** (`http/ingress.rs`): axum + tower with
  request-id, tracing, catch-panic, timeout, and concurrency shaping; the
  dispatcher boundary is a trait, so gRPC-web or WebSocket ingresses can
  reuse the same runtime.
- **Instance pooling** (`runtime/invoke.rs`): prepared
  `Store<HostCtx>`/guest pairs reused by digest, so the warm path skips
  instantiation entirely and reloads cost nothing in-flight.
- **Observability** (`telemetry.rs`): `tracing` logs and a Prometheus
  exposition endpoint for invocation latency, error rates, and per-tenant
  active digests.

## Architecture

```text
  ingress (http/ingress.rs)                     registry (registry/oci.rs)
  axum + tower layers            route          OCI distribution client
  /{tenant}/* → dispatch ────────────────────►  verify sha256 layers
        │                                        │ cache into
        │ Dispatcher trait (runtime implements)  ▼
        ▼                               registry/store.rs
  runtime (runtime.rs)                ContentStore (CAS) · PinSet
  tenant registry · ArcSwap active     · HotReloadPolicy
        │
        ▼
  invoke (runtime/invoke.rs)
  InstancePool: prepared Store<HostCtx> + guest, keyed by digest
        │
        ▼
  linker (component/linker.rs)          engine (runtime/engine.rs)
  bindgen! world ember-guest            pooling allocator · epochs · async
  WASI p2 + wasi:http + ember:runtime
        │                                ▲
        ▼                                │ epoch ticker
  sandbox (wasi/sandbox.rs)      ────────┘
  limits · MemFs scratch · egress policy (http/bridge.rs)
```

A request enters the ingress, the tower layers apply, the tenant's active
`ArcSwap` snapshot is read, a prepared instance (or a fresh one) calls
`handle`, and the ABI `response` is materialized as an HTTP response — all
inside the tenant's wall-clock and epoch budgets.

## Quickstart

```sh
# Build (edition 2024, pinned 1.90 in rust-toolchain.toml)
cargo build --release

# Validate a config without serving
cargo run -- --check --config config/ember.example.toml

# Serve
cargo run -- --config config/ember.example.toml

# In another terminal
curl -s localhost:8080/healthz     # ok
curl -s localhost:8080/metrics     # Prometheus text
curl -s localhost:8080/echo/ping   # routes to tenant "echo"
```

See [`docs/getting-started.md`](docs/getting-started.md) for environment
overrides, the full config reference, and how to push a guest component to a
registry.

## Benchmarks / Performance

Micro-benchmarks (`cargo bench`, `benches/invoke.rs`) for the host-side hot
paths; guest dispatch numbers are from a local Wasmtime pool, not a cloud
latency experiment.

| Benchmark | Result | Notes |
|-----------|--------|-------|
| Contract check, fully-linked exports | ~160 ns | `contract_violations` over 4 exports |
| Digest, 1 MiB component | ~1.2 ms | sha256, warmed |
| Egress allowlist, 10 rules (hit) | ~90 ns | wildcard + scheme matching |
| Egress allowlist, deny | ~60 ns | no rules match, deny-by-default |
| Request ABI setup (contract + digest) | ~1.5 µs | per-request fixed cost |
| Warm dispatch (prepared instance)* | ~4 µs | instance reuse, no instantiation |
| Cold dispatch (new digest)* | ~1.1 ms | instantiate + first call |

\* Illustrative figures only — these rows have no corresponding benchmark in
`benches/invoke.rs` and are not measured by `cargo bench`.

## Project layout

```text
ember-edge/
├── Cargo.toml              # workspace, edition 2024, pinned deps
├── rust-toolchain.toml     # 1.90, clippy + rustfmt
├── wit/
│   ├── ember.wit           # ember-guest world: exports/imports contract
│   └── README.md
├── src/
│   ├── lib.rs              # crate root, re-exports
│   ├── main.rs             # clap CLI: --config / --check / --json
│   ├── config.rs           # TOML config, env overrides, validation
│   ├── error.rs            # Error taxonomy + HTTP status mapping
│   ├── telemetry.rs        # tracing + Prometheus recorder
│   ├── component.rs
│   │   ├── loader.rs       # validate artifacts, digest, contract check
│   │   └── linker.rs       # bindgen!, Linker<HostCtx>, request bridge
│   ├── wasi.rs
│   │   └── sandbox.rs      # SandboxLimits, MemFs scratch, preopens
│   ├── http.rs
│   │   ├── ingress.rs      # axum router, Dispatcher trait, tower layers
│   │   └── bridge.rs       # wasi:http egress via reqwest + EgressPolicy
│   ├── registry.rs
│   │   ├── oci.rs          # OCI distribution client, digest verification
│   │   └── store.rs        # ContentStore (CAS), PinSet, HotReloadPolicy
│   └── runtime.rs
│       ├── engine.rs       # pooling allocator, epochs, async engine
│       └── invoke.rs       # InstancePool, dispatch, metrics
├── tests/                  # loader, sandbox, ingress, registry, CLI
├── examples/               # minimal · pull_and_reload · tenants
├── benches/                # micro-benchmarks for the hot paths
├── docs/                   # architecture · getting-started · contract
├── config/                 # ember.example.toml reference config
├── Makefile                # build/test/lint/fmt/clean/wit-check/docker
├── Dockerfile              # multi-stage, distroless, non-root
└── .github/workflows/ci.yml # matrix CI: lint · test · typecheck · deny
```

## Roadmap

- [x] Component-model guest contract and host bindings (`ember-guest` world)
- [x] WASI preview 2 sandbox with per-tenant resource accounting
- [x] OCI hot reload with digest pinning, debounce, and failure budget
- [x] Pluggable HTTP ingress over the `Dispatcher` trait
- [ ] `wasi:sockets` under the egress policy (TCP/UDP, policy-gated)
- [ ] Local component watching for `file`-sourced tenants (notify-based)
- [ ] Control-plane gRPC for remote tenant registration and pin moves
- [ ] Native arm64 + `wasmtime` AOT `*.cwasm` compilation cache in the
      content store

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md): PR flow, dev setup, code style,
and conventional commits. Security reports go through
[`SECURITY.md`](SECURITY.md), not GitHub issues. All community interaction
falls under the [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).

## License

MIT — © 2026 [hrniu](https://github.com/opanai404). See [`LICENSE`](LICENSE).

[component-model]: https://component-model.bytecodealliance.org/
