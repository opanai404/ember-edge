<!--
  EMBER · WebAssembly edge runtime for the component model
  SPDX-License-Identifier: MIT
-->

# Component contract (`ember-guest`)

Every component Ember schedules must target the `ember-guest` world defined
in `wit/ember.wit`. This document is the normative reference for the surface
a guest may rely on and the obligations it must meet.

## World summary

```wit
world ember-guest {
    export handler;
    import tenant;
    import telemetry;

    // WASI preview 2 subset exposed by the sandbox:
    import wasi:io/error@0.2.0;
    import wasi:io/streams@0.2.0;
    import wasi:clocks/monotonic-clock@0.2.0;
    import wasi:clocks/wall-clock@0.2.0;
    import wasi:filesystem/types@0.2.0;
    import wasi:filesystem/preopens@0.2.0;
    import wasi:random/random@0.2.0;
    import wasi:http/types@0.2.0;
    import wasi:http/outgoing-handler@0.2.0;
}
```

## Exports

### `handler` (required)

| Function | Signature | Notes |
|----------|-----------|-------|
| `handle` | `func(req: request) -> response` | One request in, one response out. The runtime guarantees exactly one call per request and enforces the wall-clock budget around it. |

```wit
record request {
    method: string,
    path: string,
    query: option<string>,
    headers: list<tuple<string, string>>,
    body: option<list<u8>>,
}

record response {
    status: u16,
    headers: list<tuple<string, string>>,
    body: list<u8>,
}
```

`headers` preserves order and multiplicity: a multi-valued request header
appears as repeated tuples. Response headers the host cannot materialize as
valid HTTP header names/values are dropped rather than forwarded (see the
HTTP contract in `docs/api/reference.md`).

## Imports

### `tenant`

| Function | Returns | Contract |
|----------|---------|----------|
| `tenant-id` | `string` | Stable per tenant across reloads. |
| `get-config(key)` | `option<string>` | Tenant-scoped config from `[tenants.config]`; `none` when unset. |
| `scratch-write(path, data)` | `result<_, scratch-error>` | Append/replace under a per-tenant byte quota. `quota-exceeded` when the write would exceed `memfs_quota_bytes`; `invalid-path` for absolute or empty paths. |
| `scratch-read(path)` | `option<list<u8>>` | `none` when the file does not exist. |
| `scratch-bytes` | `u64` | Current usage, for guest-side quota management. |

### `telemetry`

`emit(level, message)` routes a log line into the host's `tracing` pipeline
under `ember::guest::{tenant}`. It is best-effort and never fails the
request.

### WASI preview 2 subset

The sandbox exposes a *curated* WASI p2 subset: io, clocks, filesystem
**types only** (no raw preopens beyond the read-only host dirs a tenant is
provisioned with), random, and the HTTP types plus `outgoing-handler`.

- `wasi:http/outgoing-handler` is routed through `EgressPolicy`. Egress is
  **deny-by-default**: a tenant with no allow rules cannot reach the network.
- No `wasi:sockets` and no writable filesystem preopens: durable state flows
  through `scratch-*` so the host can account for every byte.

## Building a guest

Minimal guest `Cargo.toml`:

```toml
[package]
name = "echo"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
wit-bindgen = "0.40"

[build-dependencies]
wit-bindgen = "0.40"
```

`build.rs`:

```rust
fn main() {
    println!("cargo:rerun-if-changed=../../../wit");
    wit_bindgen::generate!("echo", "../../../wit", "ember-guest");
}
```

Guest crate root:

```rust
use wit_bindgen::generate;

generate!("echo", "../../../wit", "ember-guest");

struct Echo;

impl Guest for Echo {
    fn handle(req: Request) -> Response {
        let body = match req.body {
            Some(b) if b == b"ping" => b"pong".to_vec(),
            _ => b"hello from ember".to_vec(),
        };
        Response {
            status: 200,
            headers: vec![("content-type".to_string(), "text/plain".to_string())],
            body,
        }
    }
}

export!(Echo);
```

Build with `cargo component build` (target `wasm32-wasip2`) and push the
resulting `.wasm` to your registry as described in `getting-started.md`.

## Versioning

The world id `ember-guest` and the package `ember:runtime` follow semantic
versioning at the *interface* level:

- Additive changes (new host function with a default, new WASI import that
  guests may ignore) bump `ember:runtime`'s minor.
- Breaking changes (renamed records, removed host functions, narrowed WASI
  surface) bump the major and are announced in `CHANGELOG.md`; the runtime
  keeps a one-version compatibility window so pinned deployments can migrate
  without a coordinated cutover.
