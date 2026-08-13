// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Error taxonomy for the runtime.
//!
//! [`Error`] is the single error type that crosses module boundaries. The
//! [`Error::status`] mapping drives the HTTP status an ingress failure
//! surfaces to a caller, so tenant/configuration failures read as 4xx and
//! sandbox/runtime failures read as 5xx.

use std::path::PathBuf;

use thiserror::Error;

/// Crate-wide result alias. Defaults the error type to [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Errors that can occur anywhere inside Ember.
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid or contradictory configuration.
    #[error("configuration error: {0}")]
    Config(String),

    /// A component artifact failed structural validation.
    #[error("invalid component artifact `{path}`: {reason}")]
    InvalidComponent {
        /// Source name of the artifact (file path or OCI reference).
        path: String,
        /// Validator detail.
        reason: String,
    },

    /// A valid component does not satisfy the `ember-guest` contract.
    #[error("component contract violated: {0}")]
    Contract(String),

    /// No artifact is available for a requested reference or digest.
    #[error("component not found: {0}")]
    ComponentNotFound(String),

    /// No tenant is registered under the given id.
    #[error("tenant not found: {0}")]
    TenantNotFound(String),

    /// Generic registry-layer failure.
    #[error("registry error: {0}")]
    Registry(String),

    /// OCI distribution protocol failure.
    #[error("oci distribution error: {0}")]
    Oci(String),

    /// Wasmtime engine/component failure.
    #[error("wasmtime error: {0}")]
    Wasm(#[from] wasmtime::Error),

    /// `wasm-tools` validation failure.
    #[error("validation error: {0}")]
    Validation(String),

    /// HTTP layer failure (header construction, body conversion, ...).
    #[error("http error: {0}")]
    Http(#[from] http::Error),

    /// I/O failure.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// A blob's sha256 did not match its claimed digest.
    #[error("digest mismatch: expected {expected}, computed {computed}")]
    DigestMismatch {
        /// Digests must be `sha256:<hex>`.
        expected: String,
        /// Computed value, also `sha256:<hex>`.
        computed: String,
    },

    /// A per-tenant sandbox limit was exceeded.
    #[error("sandbox limit exceeded: {0}")]
    LimitExceeded(String),

    /// Graceful shutdown requested while serving.
    #[error("shutdown requested")]
    Shutdown,
}

impl Error {
    /// Map an error to the HTTP status it should be surfaced as.
    pub fn status(&self) -> u16 {
        match self {
            Error::Config(_) => 500,
            Error::InvalidComponent { .. } | Error::Contract(_) => 400,
            Error::ComponentNotFound(_) | Error::TenantNotFound(_) => 404,
            Error::Registry(_) | Error::Oci(_) | Error::Wasm(_) | Error::Validation(_) => 500,
            Error::Http(_) => 400,
            Error::Io(_) => 500,
            Error::DigestMismatch { .. } => 409,
            Error::LimitExceeded(_) => 429,
            Error::Shutdown => 503,
        }
    }
}
