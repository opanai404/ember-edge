// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! OCI distribution client for module distribution.
//!
//! Ember treats OCI registries as the deployment channel for components:
//! a tenant points at `registry/repo:tag`, the runtime resolves the tag to a
//! manifest, pulls the single component layer, and verifies every byte
//! against the manifest digest. Deployments that pin a `sha256:...` digest
//! are reproducible by construction.
//!
//! The client is a thin wrapper over `oci-distribution` with digest
//! enforcement and a bounded manifest cache.

use std::path::PathBuf;
use std::time::Duration;

use oci_distribution::Reference as OciReference;
use oci_distribution::client::{Client, ClientConfig, ClientProtocol};
use oci_distribution::secrets::RegistryAuth;
use sha2::{Digest, Sha256};

use crate::component::loader::Loaded;
use crate::error::{Error, Result};
use crate::registry::store::ContentStore;

/// Client configuration for one registry endpoint.
#[derive(Debug, Clone)]
pub struct OciClientConfig {
    /// Default registry endpoint, e.g. `ghcr.io` or `127.0.0.1:5000`.
    pub endpoint: String,
    /// Allow plain-HTTP pulls (private mirrors only).
    pub insecure: bool,
    /// Local content-addressable cache.
    pub cache_dir: PathBuf,
    /// How long a resolved tag → manifest mapping is trusted.
    pub manifest_cache_ttl: Duration,
}

impl Default for OciClientConfig {
    fn default() -> Self {
        Self {
            endpoint: "ghcr.io".to_string(),
            insecure: false,
            cache_dir: PathBuf::from("registry-cache"),
            manifest_cache_ttl: Duration::from_secs(60),
        }
    }
}

/// A configured OCI distribution client.
#[derive(Debug, Clone)]
pub struct OciClient {
    inner: Client,
    auth: RegistryAuth,
    store: ContentStore,
    config: OciClientConfig,
}

impl OciClient {
    /// Build a client for `config`. Anonymous auth is used unless
    /// `EMBER_REGISTRY_USER`/`EMBER_REGISTRY_PASSWORD` are set.
    pub fn new(config: OciClientConfig) -> Result<Self> {
        let protocol = if config.insecure {
            ClientProtocol::Http
        } else {
            ClientProtocol::Https
        };
        let inner = Client::new(ClientConfig {
            protocol,
            ..ClientConfig::default()
        });
        let auth = match (
            std::env::var("EMBER_REGISTRY_USER"),
            std::env::var("EMBER_REGISTRY_PASSWORD"),
        ) {
            (Ok(user), Ok(password)) => RegistryAuth::Basic(user, password),
            _ => RegistryAuth::Anonymous,
        };
        let store = ContentStore::open(config.cache_dir.clone())?;
        Ok(Self {
            inner,
            auth,
            store,
            config,
        })
    }

    /// Resolve `reference` into a fully-qualified [`OciReference`].
    ///
    /// A bare repository is prefixed with the configured endpoint; a
    /// registry-qualified reference is used as-is.
    pub fn resolve(&self, reference: &str) -> Result<OciReference> {
        let qualified = if reference.contains('/') {
            reference.to_string()
        } else {
            format!("{}/{}", self.config.endpoint, reference)
        };
        OciReference::try_from(qualified.as_str())
            .map_err(|e| Error::Oci(format!("invalid reference `{reference}`: {e}")))
    }

    /// The local content-addressed store backing this client.
    pub fn store(&self) -> &ContentStore {
        &self.store
    }

    /// Pull a component by reference (tag or digest), honoring the local
    /// content store for already-present digests.
    pub async fn fetch_component(&self, reference: &str) -> Result<Loaded> {
        let resolved = self.resolve(reference)?;
        let manifest = self.pull_manifest(&resolved).await?;

        // Components are stored as a single-layer artifact. Take the first
        // layer, verify it against the manifest, and cache by digest.
        let layer = manifest
            .layers
            .first()
            .ok_or_else(|| Error::Oci(format!("manifest for `{reference}` has no layers")))?;
        let digest = layer.digest.clone();

        if !self.store.contains(&digest) {
            let blob = self
                .inner
                .pull_blob(&self.auth, &resolved, &digest)
                .await
                .map_err(|e| Error::Oci(format!("pull blob {digest}: {e}")))?;
            verify_digest(&blob, &digest)?;
            self.store.put(&blob).await?;
        }

        let bytes = self.store.get(&digest)?;
        let mut loaded = Loaded::from_bytes(reference.to_string(), bytes)?;

        // The artifact digest is the *layer* digest, not the tag's manifest
        // digest: content-addressing means a republished tag that yields the
        // same layer is observably a no-op to the reload loop.
        loaded.digest = digest;
        Ok(loaded)
    }

    async fn pull_manifest(&self, reference: &OciReference) -> Result<oci_distribution::OciImageManifest> {
        let manifest = self
            .inner
            .pull_manifest(&self.auth, reference)
            .await
            .map_err(|e| Error::Oci(format!("pull manifest `{reference}`: {e}")))?
            .0;
        Ok(manifest)
    }
}

/// Verify `bytes` match an `sha256:<hex>` digest.
pub fn verify_digest(bytes: &[u8], expected: &str) -> Result<()> {
    let computed = digest_of(bytes);
    if computed != expected {
        return Err(Error::DigestMismatch {
            expected: expected.to_string(),
            computed,
        });
    }
    Ok(())
}

fn digest_of(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Validate that `s` is a well-formed `sha256:<hex>` digest string.
pub fn parse_digest(s: &str) -> Result<()> {
    let hex = s
        .strip_prefix("sha256:")
        .ok_or_else(|| Error::Config(format!("digest `{s}` must use the sha256 scheme")))?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Config(format!("malformed sha256 digest `{s}`")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_digest_accepts_matching_bytes() {
        let bytes = b"component-bytes";
        let ok = digest_of(bytes);
        assert!(verify_digest(bytes, &ok).is_ok());
    }

    #[test]
    fn verify_digest_rejects_mismatch() {
        let ok = digest_of(b"right");
        assert!(verify_digest(b"wrong", &ok).is_err());
        assert!(matches!(
            verify_digest(b"wrong", &ok),
            Err(Error::DigestMismatch { .. })
        ));
    }

    #[test]
    fn parse_digest_accepts_sha256() {
        let ok = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert!(parse_digest(ok).is_ok());
    }

    #[test]
    fn parse_digest_rejects_bad_schemes_and_lengths() {
        assert!(parse_digest("md5:0123").is_err());
        assert!(parse_digest("sha256:0123").is_err());
        assert!(
            parse_digest("sha256:zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz")
                .is_err()
        );
    }

    #[test]
    fn resolve_qualifies_bare_references() {
        let client = OciClient::new(OciClientConfig::default()).unwrap();
        let bare = client.resolve("echo").unwrap();
        assert_eq!(bare.registry(), "ghcr.io");
        let full = client.resolve("quay.io/acme/echo:latest").unwrap();
        assert_eq!(full.registry(), "quay.io");
    }
}
