<!--
  EMBER · WebAssembly edge runtime for the component model
  SPDX-License-Identifier: MIT
-->

# `wit/` — the Ember guest contract

This directory holds the WIT (WebAssembly Interface Types) world that defines
the boundary between the Ember runtime (host) and edge components (guests).

| File           | Purpose                                                       |
|----------------|---------------------------------------------------------------|
| `ember.wit`    | The `ember-guest` world: exported `handler`, imported host capabilities and WASI preview 2 surface. |

## Regenerating host bindings

The host crate generates its bindings with the `wasmtime::component::bindgen!`
macro, resolved against this directory at compile time
(`src/component/linker.rs`). No checked-in generated code is needed.

Validate the WIT and the world any time you edit it:

```sh
wasm-tools component wit wit/          # type-check the world
make wit-check                         # same, via Makefile
```

## Building a guest against this world

Guests are ordinary Rust components built with `cargo component` (or raw
`wit-bindgen`). See `docs/component-contract.md` for the guest-side
`build.rs` and a full worked example.
