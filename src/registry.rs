// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Module distribution: OCI pulls, the content store, and reload policy.
//!
//! [`OciClient`] is the distribution channel, [`ContentStore`] is the local
//! content-addressed cache, and [`HotReloadPolicy`] governs when a freshly
//! resolved digest replaces the active component. The runtime's reload loop
//! is the only consumer of all three.

pub mod oci;
pub mod store;

pub use oci::{OciClient, OciClientConfig, parse_digest, verify_digest};
pub use store::{ContentStore, HotReloadPolicy, PinSet, digest};

use crate::component::loader::Loaded;
use crate::error::Result;

/// A registry facade that couples the OCI client with the local store.
#[derive(Debug, Clone)]
pub struct Registry {
    /// The distribution client.
    pub oci: OciClient,
    /// The local content-addressed store.
    pub store: ContentStore,
    /// Reload governance.
    pub policy: store::HotReloadPolicy,
}

impl Registry {
    /// Build a registry from client configuration and a reload policy.
    pub fn new(config: OciClientConfig, policy: store::HotReloadPolicy) -> Result<Self> {
        let oci = OciClient::new(config)?;
        Ok(Self {
            store: oci.store().clone(),
            oci,
            policy,
        })
    }

    /// Pull `reference` from the registry, applying an optional `pin`.
    ///
    /// When `pin` is set the resolved artifact must match it; a mismatch
    /// surfaces as [`crate::error::Error::DigestMismatch`].
    pub async fn pull(&self, reference: &str, pin: Option<&str>) -> Result<Loaded> {
        let mut loaded = self.oci.fetch_component(reference).await?;
        if let Some(pin) = pin {
            crate::registry::verify_digest(&loaded.bytes, pin)?;
            loaded.digest = pin.to_string();
        }
        Ok(loaded)
    }
}
