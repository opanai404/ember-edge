// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Per-tenant sandbox construction and resource accounting.
//!
//! Every tenant runs inside a [`Sandbox`] that carries three independent
//! enforcement mechanisms:
//!
//! 1. **Engine-level ceilings** ([`SandboxLimits`]) — clamped into the
//!    pooled allocator at engine build time (memory, instances, tables).
//! 2. **Dispatch-level budgets** — wall clock and body size are enforced
//!    with `tokio::time::timeout` and body limiting in the ingress.
//! 3. **Host-accounted scratch storage** ([`MemFs`]) — a byte-quota'd
//!    in-memory filesystem exposed to guests through the
//!    `ember:runtime/tenant` interface rather than a raw WASI preopen.
//!
//! The runtime deliberately does *not* grant a WASI filesystem preopen to
//! tenants. State that must persist across invocations lives in [`MemFs`],
//! which the host can account for, evict, and never spills to disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::http::bridge::EgressPolicy;
use crate::runtime::TenantId;

/// Per-tenant resource ceiling. Limits are enforced by the engine, the
/// dispatcher, or the host depending on the row in `docs/architecture.md`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SandboxLimits {
    /// Guest linear-memory ceiling (pooled allocator).
    #[serde(deserialize_with = "crate::config::deserialize_memory")]
    pub max_memory_bytes: usize,
    /// Concurrent instances this tenant may keep in its pool.
    pub max_instances: usize,
    /// Tables (function/reference tables) per instance.
    pub max_tables: usize,
    /// Max epoch budget per invocation (ticks of the engine epoch counter).
    pub max_epoch_delta: u64,
    /// Max wall-clock time per invocation.
    #[serde(rename = "max_wall_duration_ms", with = "serde_millis")]
    pub max_wall_duration: Duration,
    /// Max request/response body size for this tenant.
    #[serde(deserialize_with = "crate::config::deserialize_memory")]
    pub max_body_bytes: usize,
    /// Byte quota for the tenant scratch filesystem.
    #[serde(deserialize_with = "crate::config::deserialize_memory")]
    pub memfs_quota_bytes: usize,
    /// Logical CPU weight (reserved; currently informational).
    pub cpus: u32,
    /// Egress allowlist entries (`https://host`, `*.example.com`).
    #[serde(default)]
    pub egress_allow: Vec<String>,
    /// Deny outbound HTTP not matched by `egress_allow`.
    #[serde(default = "default_deny")]
    pub default_deny_egress: bool,
}

fn default_deny() -> bool {
    true
}

impl Default for SandboxLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 128 * 1024 * 1024,
            max_instances: 8,
            max_tables: 32,
            max_epoch_delta: 10_000_000,
            max_wall_duration: Duration::from_secs(10),
            max_body_bytes: 8 * 1024 * 1024,
            memfs_quota_bytes: 4 * 1024 * 1024,
            cpus: 1,
            egress_allow: Vec::new(),
            default_deny_egress: true,
        }
    }
}

impl SandboxLimits {
    /// Reject self-contradictory limits before they reach the engine.
    pub fn validate(&self) -> Result<()> {
        if self.max_memory_bytes == 0 {
            return Err(Error::LimitExceeded("max_memory_bytes must be > 0".into()));
        }
        if self.max_wall_duration.is_zero() {
            return Err(Error::LimitExceeded("max_wall_duration must be > 0".into()));
        }
        if self.max_instances == 0 {
            return Err(Error::LimitExceeded("max_instances must be > 0".into()));
        }
        if self.memfs_quota_bytes == 0 {
            return Err(Error::LimitExceeded("memfs_quota_bytes must be > 0".into()));
        }
        Ok(())
    }

    /// Clamp this tenant's limits into the engine's global budget so a single
    /// tenant can never over-provision the shared instance pool.
    pub fn clamp_to(&mut self, engine_max_mem: usize, engine_max_instances: usize) {
        self.max_memory_bytes = self.max_memory_bytes.min(engine_max_mem);
        self.max_instances = self.max_instances.min(engine_max_instances);
        self.max_epoch_delta = self.max_epoch_delta.max(1);
    }

    /// Derive the egress policy object used by the HTTP bridge.
    pub fn egress_policy(&self) -> EgressPolicy {
        EgressPolicy {
            allow: self.egress_allow.clone(),
            default_deny: self.default_deny_egress,
        }
    }
}

mod serde_millis {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer};

    pub fn deserialize<'de, D>(d: D) -> std::result::Result<Duration, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ms = u64::deserialize(d)?;
        Ok(Duration::from_millis(ms))
    }
}

/// A byte-quota'd in-memory filesystem used as tenant scratch storage.
///
/// Every write counts against [`MemFs::quota_bytes`]; exceeding the quota is
/// a hard error surfaced to the guest as `scratch-error::quota-exceeded`.
/// The host can read current usage via [`MemFs::used_bytes`] for metrics and
/// eviction decisions.
#[derive(Debug)]
pub struct MemFs {
    files: std::sync::Mutex<BTreeMap<PathBuf, Vec<u8>>>,
    quota_bytes: usize,
    used: AtomicU64,
}

impl MemFs {
    /// Create an empty scratch filesystem with the given byte quota.
    pub fn new(quota_bytes: usize) -> Self {
        Self {
            files: std::sync::Mutex::new(BTreeMap::new()),
            quota_bytes,
            used: AtomicU64::new(0),
        }
    }

    /// The configured byte ceiling.
    pub fn quota_bytes(&self) -> usize {
        self.quota_bytes
    }

    /// Bytes currently held.
    pub fn used_bytes(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    /// Whether `path` has a file.
    pub fn exists(&self, path: &Path) -> bool {
        self.files.lock().unwrap().contains_key(path)
    }

    /// Overwrite (or create) the file at `path`, accounting the byte delta
    /// against the quota. Returns `quota-exceeded` if the write would push
    /// usage over the ceiling.
    pub fn write(&self, path: &Path, bytes: Vec<u8>) -> Result<()> {
        if path.as_os_str().is_empty() || path.is_absolute() {
            return Err(Error::LimitExceeded(format!(
                "invalid scratch path `{}`",
                path.display()
            )));
        }
        let mut files = self.files.lock().unwrap();
        let prev = files.get(path).map(|b| b.len() as i64).unwrap_or(0);
        let next = bytes.len() as i64;
        let delta = next - prev;
        if delta > 0 && (self.used.load(Ordering::Relaxed) as i64) + delta > self.quota_bytes as i64
        {
            return Err(Error::LimitExceeded(format!(
                "memfs quota of {} bytes exceeded writing `{}`",
                self.quota_bytes,
                path.display()
            )));
        }
        if delta >= 0 {
            self.used.fetch_add(delta as u64, Ordering::Relaxed);
        } else {
            self.used.fetch_sub((-delta) as u64, Ordering::Relaxed);
        }
        files.insert(path.to_path_buf(), bytes);
        Ok(())
    }

    /// Read the file at `path`, if present.
    pub fn read(&self, path: &Path) -> Option<Vec<u8>> {
        self.files.lock().unwrap().get(path).cloned()
    }

    /// Remove the file at `path`, returning its contents if it existed.
    pub fn remove(&self, path: &Path) -> Option<Vec<u8>> {
        let mut files = self.files.lock().unwrap();
        let removed = files.remove(path)?;
        self.used.fetch_sub(removed.len() as u64, Ordering::Relaxed);
        Some(removed)
    }
}

/// A fully-assembled sandbox for one tenant.
#[derive(Debug, Clone)]
pub struct Sandbox {
    /// Tenant this sandbox confines.
    pub tenant: TenantId,
    /// Resource ceilings.
    pub limits: SandboxLimits,
    /// Tenant-scoped configuration exposed via `ember:runtime/tenant`.
    pub config: std::collections::HashMap<String, String>,
    /// Read-only host directories preopened into WASI, `(guest_path, host_root)`.
    pub readonly_preopens: Vec<(String, PathBuf)>,
    /// Accounted scratch storage.
    pub scratch: std::sync::Arc<MemFs>,
}

impl Sandbox {
    /// Construct a sandbox for `tenant`, honoring `limits` and the optional
    /// tenant `config` map.
    pub fn new(
        tenant: TenantId,
        limits: SandboxLimits,
        config: std::collections::HashMap<String, String>,
    ) -> Result<Self> {
        limits.validate()?;
        let scratch = std::sync::Arc::new(MemFs::new(limits.memfs_quota_bytes));
        Ok(Self {
            tenant,
            limits,
            config,
            readonly_preopens: Vec::new(),
            scratch,
        })
    }

    /// Add a read-only host directory preopen at `guest_path`.
    pub fn with_preopen(mut self, guest_path: impl Into<String>, root: impl Into<PathBuf>) -> Self {
        self.readonly_preopens
            .push((guest_path.into(), root.into()));
        self
    }

    /// The tenant's egress policy, derived from its limits.
    pub fn egress_policy(&self) -> EgressPolicy {
        self.limits.egress_policy()
    }

    /// Charge `bytes` of extra memory against the tenant and enforce the
    /// ceiling. Used by the engine hook when an instance grows past budget.
    pub fn charge_memory(&self, bytes: usize) -> Result<()> {
        if bytes > self.limits.max_memory_bytes {
            return Err(Error::LimitExceeded(format!(
                "instance memory {bytes} exceeds tenant ceiling {}",
                self.limits.max_memory_bytes
            )));
        }
        Ok(())
    }

    /// Bytes of host memory currently attributable to this tenant's scratch.
    pub fn scratch_bytes(&self) -> u64 {
        self.scratch.used_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant(id: &str) -> TenantId {
        TenantId::new(id).unwrap()
    }

    #[test]
    fn limits_clamp_to_engine_budget() {
        let mut limits = SandboxLimits {
            max_memory_bytes: 512 * 1024 * 1024,
            max_instances: 64,
            ..SandboxLimits::default()
        };
        limits.clamp_to(256 * 1024 * 1024, 16);
        assert_eq!(limits.max_memory_bytes, 256 * 1024 * 1024);
        assert_eq!(limits.max_instances, 16);
    }

    #[test]
    fn zero_limits_rejected() {
        let mut l = SandboxLimits::default();
        l.max_memory_bytes = 0;
        assert!(l.validate().is_err());
        l = SandboxLimits::default();
        l.max_wall_duration = Duration::ZERO;
        assert!(l.validate().is_err());
    }

    #[test]
    fn memfs_accounts_writes_against_quota() {
        let fs = MemFs::new(1024);
        fs.write(Path::new("a.txt"), vec![b'a'; 512]).unwrap();
        assert_eq!(fs.used_bytes(), 512);
        // Overwrite with a larger file pushes 512 + 513 past the 1024 quota.
        assert!(fs.write(Path::new("a.txt"), vec![b'a'; 1025]).is_err());
        // Overwrite with a smaller file stays legal and fixes accounting.
        fs.write(Path::new("a.txt"), vec![b'a'; 128]).unwrap();
        assert_eq!(fs.used_bytes(), 128);
    }

    #[test]
    fn memfs_rejects_absolute_paths() {
        let fs = MemFs::new(1024);
        assert!(fs.write(Path::new("/etc/passwd"), vec![1, 2, 3]).is_err());
    }

    #[test]
    fn memfs_remove_returns_and_frees() {
        let fs = MemFs::new(1024);
        fs.write(Path::new("k"), vec![7; 64]).unwrap();
        assert_eq!(fs.remove(Path::new("k")), Some(vec![7; 64]));
        assert_eq!(fs.used_bytes(), 0);
        assert!(fs.remove(Path::new("k")).is_none());
    }
}
