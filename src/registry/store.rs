// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Content-addressable module store and hot-reload policy.
//!
//! [`ContentStore`] persists component blobs keyed by `sha256` digest with
//! atomic writes (temp file + rename + fsync), so a crash can never expose a
//! half-written artifact. [`PinSet`] maps tenant → pinned digest so deploys
//! are reproducible. [`HotReloadPolicy`] decides when a refreshed digest is
//! worth swapping in and when to back off after repeated pull failures.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Compute the `sha256:<hex>` content digest of `blob`.
pub fn digest(blob: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(blob)))
}

/// On-disk content-addressable store.
#[derive(Debug)]
pub struct ContentStore {
    root: PathBuf,
}

impl ContentStore {
    /// Open (creating if needed) the store rooted at `root`.
    pub fn open(root: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Path a digest's blob lives at.
    fn blob_path(&self, digest: &str) -> Result<PathBuf> {
        let path = self.root.join(sanitize_digest(digest)?);
        Ok(path)
    }

    /// Write `blob` into the store and return its content digest. Writing an
    /// already-present digest is a no-op (dedupe by content).
    pub async fn put(&self, blob: &[u8]) -> Result<String> {
        let digest = digest(blob);
        let path = self.blob_path(&digest)?;
        if path.exists() {
            return Ok(digest);
        }
        // temp file in the same directory guarantees rename is atomic.
        let tmp = self.root.join(format!(".tmp-{}", std::process::id()));
        {
            use std::io::Write;
            let mut file = std::fs::File::create(&tmp)?;
            file.write_all(blob)?;
            file.sync_all()?;
        }
        match std::fs::rename(&tmp, &path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(Error::Io(e)),
        }
        Ok(digest)
    }

    /// Read the blob for `digest` out of the store.
    pub fn get(&self, digest: &str) -> Result<Vec<u8>> {
        let path = self.blob_path(digest)?;
        std::fs::read(&path).map_err(|_| Error::ComponentNotFound(digest.to_string()))
    }

    /// Whether `digest` is present locally.
    pub fn contains(&self, digest: &str) -> bool {
        self.blob_path(digest).map(|p| p.exists()).unwrap_or(false)
    }

    /// Total bytes on disk, for metrics and GC sizing.
    pub fn disk_usage(&self) -> Result<u64> {
        let mut total = 0u64;
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_name().to_string_lossy().starts_with(".tmp") {
                continue;
            }
            total += entry.metadata()?.len();
        }
        Ok(total)
    }
}

/// Normalize a digest string to a safe filename. Digests are already
/// constrained to `sha256:<hex>`, but we refuse anything else outright.
fn sanitize_digest(digest: &str) -> Result<&str> {
    let hex = digest.strip_prefix("sha256:").ok_or_else(|| {
        Error::ComponentNotFound(format!("unsupported digest scheme in `{digest}`"))
    })?;
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::ComponentNotFound(format!(
            "malformed sha256 digest `{digest}`"
        )));
    }
    Ok(hex)
}

/// tenant/reference → pinned digest. A pin freezes a deployment: the runtime
/// will refuse to activate a component whose digest does not match.
#[derive(Debug, Clone, Default)]
pub struct PinSet {
    pins: HashMap<String, String>,
}

impl PinSet {
    /// Record a pin for `key`.
    pub fn pin(&mut self, key: impl Into<String>, digest: impl Into<String>) {
        self.pins.insert(key.into(), digest.into());
    }

    /// The pinned digest for `key`, if any.
    pub fn resolve(&self, key: &str) -> Option<&str> {
        self.pins.get(key).map(String::as_str)
    }

    /// Remove the pin for `key` if it currently equals `digest` (compare-and-
    /// unpin so a racing redeploy cannot clear a newer pin).
    pub fn unpin(&mut self, key: &str, digest: &str) -> bool {
        if self.pins.get(key).map(String::as_str) == Some(digest) {
            self.pins.remove(key);
            true
        } else {
            false
        }
    }
}

/// When and how the runtime re-resolves and swaps tenant components.
#[derive(Debug, Clone)]
pub struct HotReloadPolicy {
    /// How often the reload loop re-resolves OCI references.
    pub poll_interval: Duration,
    /// Minimum gap between two swaps for the same tenant (rate limit).
    pub debounce: Duration,
    /// Refuse to activate an unpinned tag once a tenant has a pin.
    pub require_pinned: bool,
    /// Consecutive pull failures before the tenant is parked with an error.
    pub max_failed_polls: usize,
}

impl Default for HotReloadPolicy {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(30),
            debounce: Duration::from_secs(5),
            require_pinned: false,
            max_failed_polls: 3,
        }
    }
}

impl HotReloadPolicy {
    /// Whether switching from `prev` to `next` is worth doing.
    pub fn should_reload(&self, prev: Option<&str>, next: &str) -> bool {
        match prev {
            Some(prev) => prev != next,
            None => true,
        }
    }

    /// Whether `failures` consecutive failed polls exhaust the budget.
    pub fn exhausted(&self, failures: usize) -> bool {
        failures >= self.max_failed_polls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_stable_and_unique() {
        let a = digest(b"hello");
        let b = digest(b"hello");
        assert_eq!(a, b);
        assert_ne!(a, digest(b"hellp"));
        assert!(a.starts_with("sha256:"));
        assert_eq!(a.len(), 7 + 64);
    }

    #[test]
    fn pin_resolve_compare_unpin() {
        let mut pins = PinSet::default();
        assert_eq!(pins.resolve("echo"), None);
        pins.pin("echo", "sha256:aaaa");
        assert_eq!(pins.resolve("echo"), Some("sha256:aaaa"));
        assert!(!pins.unpin("echo", "sha256:bbbb"));
        assert!(pins.unpin("echo", "sha256:aaaa"));
        assert_eq!(pins.resolve("echo"), None);
    }

    #[test]
    fn reload_policy_debounces_identical_digest() {
        let policy = HotReloadPolicy::default();
        assert!(policy.should_reload(None, "sha256:new"));
        assert!(!policy.should_reload(Some("sha256:same"), "sha256:same"));
        assert!(policy.should_reload(Some("sha256:old"), "sha256:new"));
    }

    #[test]
    fn reload_policy_parks_after_failure_budget() {
        let policy = HotReloadPolicy::default();
        assert!(!policy.exhausted(0));
        assert!(!policy.exhausted(policy.max_failed_polls - 1));
        assert!(policy.exhausted(policy.max_failed_polls));
    }

    #[tokio::test]
    async fn store_roundtrips_and_dedupes() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentStore::open(dir.path().to_path_buf()).unwrap();

        let d1 = store.put(b"payload-a").await.unwrap();
        let d2 = store.put(b"payload-a").await.unwrap();
        assert_eq!(d1, d2, "identical content must dedupe");
        assert_eq!(store.get(&d1).unwrap(), b"payload-a");

        assert!(store.contains(&d1));
        assert!(
            !store.contains(
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
            )
        );
        assert_eq!(store.disk_usage().unwrap(), 9);
    }

    #[tokio::test]
    async fn store_rejects_malformed_digests() {
        let dir = tempfile::tempdir().unwrap();
        let store = ContentStore::open(dir.path().to_path_buf()).unwrap();
        assert!(store.get("md5:abc").is_err());
        assert!(store.get("sha256:short").is_err());
        assert!(store.get("sha256:zz".repeat(32).as_str()).is_err());
    }
}
