// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! WASI preview 2 context construction.
//!
//! [`Wasi`] assembles the `wasmtime_wasi` preview 2 context for one tenant
//! from its [`Sandbox`]: stdio inheritance, tenant environment, read-only
//! preopens, and the `wasi:http` outbound context backed by the egress
//! bridge. The assembled context is what the component [`Linker`] reads out
//! of the store state.
//!
//! [`Linker`]: crate::component::linker

pub mod sandbox;

use std::path::Path;
use std::sync::Arc;

use wasmtime_wasi::WasiCtxBuilder;
use wasmtime_wasi_http::WasiHttpCtx;

use crate::error::{Error, Result};
use crate::http::bridge::HttpBridge;
use crate::wasi::sandbox::Sandbox;

pub use sandbox::{MemFs, Sandbox, SandboxLimits};

/// The WASI preview 2 host context for a single tenant.
#[derive(Debug)]
pub struct Wasi {
    /// Preview 2 context (clocks, io, filesystem preopens, random).
    pub ctx: wasmtime_wasi::WasiCtx,
    /// `wasi:http` context with the tenant's egress policy applied.
    pub http: WasiHttpCtx,
}

impl Wasi {
    /// Build the preview 2 context for `sandbox`, routing outbound HTTP
    /// through `egress`.
    pub fn new(sandbox: &Sandbox, egress: Arc<HttpBridge>) -> Result<Self> {
        let mut builder = WasiCtxBuilder::new();
        builder.inherit_stdio();

        // Tenant identity is visible to the guest via environment.
        builder.env("EMBER_TENANT", sandbox.tenant.as_str());
        builder.env(
            "EMBER_SCRATCH_QUOTA",
            &sandbox.limits.memfs_quota_bytes.to_string(),
        );

        for (guest_path, host_root) in &sandbox.readonly_preopens {
            let file = open_readonly_dir(host_root.as_path())?;
            builder
                .preopened_dir(file, guest_path)
                .map_err(|e| Error::Config(format!("preopen `{guest_path}` failed: {e}")))?;
        }

        Ok(Self {
            ctx: builder.build(),
            http: WasiHttpCtx::new().with_client(egress),
        })
    }
}

/// Open `root` as a read-only preopen backing file. A read-only handle is a
/// deliberate choice: tenants get visibility, not mutation, of host data.
fn open_readonly_dir(root: &Path) -> Result<std::fs::File> {
    let meta = std::fs::metadata(root)
        .map_err(|e| Error::Config(format!("preopen `{}` unreadable: {e}", root.display())))?;
    if !meta.is_dir() {
        return Err(Error::Config(format!(
            "preopen `{}` is not a directory",
            root.display()
        )));
    }
    std::fs::File::open(root)
        .map_err(|e| Error::Config(format!("preopen `{}` open failed: {e}", root.display())))
}
