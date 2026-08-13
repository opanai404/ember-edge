<!--
  EMBER · WebAssembly edge runtime for the component model
  SPDX-License-Identifier: MIT
-->

# Getting started

## Prerequisites

- Rust 1.90+ (edition 2024). `rust-toolchain.toml` pins 1.90.
- `wasm-tools` for WIT and component tooling: `cargo install wasm-tools`.
- `wit-bindgen-cli` only if you build guest components by hand:
  `cargo install wit-bindgen-cli`.
- An OCI registry if you want the hot-reload path (any distribution-spec
  registry works; `ghcr.io`, Docker Hub, or a local `registry:2`).

## Build and run

```sh
make build            # cargo build
cargo run -- --check --config config/ember.example.toml   # validate config
cargo run -- --config config/ember.example.toml           # serve
```

In another terminal:

```sh
curl -s localhost:8080/healthz          # → ok
curl -s localhost:8080/metrics          # → Prometheus exposition
curl -s localhost:8080/echo/ping        # → routes to tenant `echo`
```

Environment overrides:

| Variable                      | Overrides                 |
|-------------------------------|---------------------------|
| `EMBER_BIND`                  | `config.bind`             |
| `EMBER_REGISTRY_ENDPOINT`     | `registry.endpoint`       |
| `EMBER_PULL_INTERVAL_SECS`    | `registry.pull_interval_secs` |
| `EMBER_REGISTRY_USER` / `EMBER_REGISTRY_PASSWORD` | OCI basic auth |
| `EMBER_LOG` / `RUST_LOG`      | log filter                |

## Configuration reference

```toml
bind = "0.0.0.0:8080"

[engine]
max_memory_bytes = "1024m"   # global guest memory ceiling
max_instances = 64           # pooled instance ceiling
threads = 4
epoch_interval_ms = 100

[registry]
endpoint = "ghcr.io"
insecure = false
pull_interval_secs = 30
cache_dir = "registry-cache"

[http]
timeout_secs = 30
concurrency = 512
max_body_bytes = "8m"
request_id_header = "x-request-id"

[[tenants]]
name = "echo"

[tenants.component]
kind = "oci"                       # or "file"
reference = "ghcr.io/opanai404/ember/echo:latest"
pin = "sha256:..."                 # optional, reproducible deploys

[tenants.limits]
max_memory_bytes = "256m"
max_instances = 8
max_wall_duration_ms = 10_000
max_body_bytes = "1m"
memfs_quota_bytes = "4m"
egress_allow = ["https://api.example.com", "https://*.static.example.net"]

[tenants.config]
UPSTREAM_BASE = "https://api.example.com"
```

Memory fields accept plain bytes or `k`/`m`/`g` suffixes. Every tenant limit
is clamped into the engine budget at startup, so an over-budget tenant is
silently reduced rather than crashing the runtime.

## Deploying a guest component

1. Build a component against the `ember-guest` world
   (`docs/component-contract.md` has a full worked example).
2. Push it to your registry. With `wasm-tools`:
   ```sh
   wasm-tools component new guest/target/wasm32-wasip2/release/echo.wasm \
     -o echo-component.wasm
   # push with crane or your registry client, e.g.
   crane push echo-component.wasm ghcr.io/opanai404/ember/echo:latest
   ```
3. Point a tenant at the reference, optionally pin the digest, and reload is
   automatic: republish the tag and Ember picks up the new digest within
   `pull_interval_secs`, debounced to at most one swap per tenant.

## Container image

```sh
make docker            # docker build -t ember-edge:0.1.0 .
make docker-run        # serve on :8080 with config/ mounted read-only
```

The image is a two-stage build: `rust:1.90` compile stage, distroless
`cc-debian12:nonroot` runtime stage running as non-root.

## Running the test suite

```sh
make test        # cargo test --all-features
make lint        # clippy -D warnings
make fmt-check   # rustfmt --check
make wit-check   # wasm-tools component wit wit/
```

CI runs the lint and test jobs on Linux, macOS, and Windows, plus a
`cargo check` + `cargo doc` job and a `cargo-deny` license audit on Linux.
