// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! # Ember
//!
//! A lightweight WebAssembly runtime for edge workloads built on Wasmtime.
//! Ember runs [component-model] components under the WASI preview 2 surface,
//! isolates each tenant inside a resource-accounted sandbox, hot-reloads
//! components from an OCI registry by pinned digest, and exposes a pluggable
//! HTTP ingress.
//!
//! The crate is structured around five modules that mirror the runtime
//! architecture (see `docs/architecture.md`):
//!
//! - [`component`] — component artifact validation and the host [`linker`]
//! - [`wasi`] — WASI preview 2 context construction and per-tenant sandboxes
//! - [`http`] — the axum ingress and the outbound egress bridge
//! - [`registry`] — the OCI distribution client and content-addressable store
//! - [`runtime`] — engine assembly, tenant registry, invocation, hot reload
//!
//! The typical entry point is [`runtime::Runtime::build`], or the packaged
//! [`main`] binary which drives it from a TOML config file.
//!
//! [component-model]: https://component-model.bytecodealliance.org/

pub mod component;
pub mod config;
pub mod error;
pub mod http;
pub mod registry;
pub mod runtime;
pub mod telemetry;
pub mod wasi;

pub use config::Config;
pub use error::{Error, Result};
pub use runtime::Runtime;

/// Runtime version reported in diagnostics and the metrics endpoint.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The world id a component must target to be scheduled by Ember. See
/// [`component::loader::EMBER_WORLD`].
pub const EMBER_WORLD: &str = component::loader::EMBER_WORLD;

/// Max length of a tenant id, enforced by [`runtime::TenantId`].
pub const MAX_TENANT_ID_LEN: usize = 64;
