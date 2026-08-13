// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Pluggable HTTP surface.
//!
//! [`ingress`] is the inbound side (axum + tower) and [`bridge`] is the
//! outbound side (`wasi:http` egress). Both are replaceable: the ingress
//! dispatches through the [`ingress::Dispatcher`] trait and the bridge
//! implements `wasmtime_wasi::http::HttpClient`, so a gRPC-web ingress or a
//! policy-proxied egress can be swapped in without touching the runtime.

pub mod bridge;
pub mod ingress;

pub use bridge::{EgressPolicy, HttpBridge};
pub use ingress::{Dispatcher, IngressError, RuntimeState};
