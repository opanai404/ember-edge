// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Runtime configuration.
//!
//! [`Config`] is deserialized from a TOML file (see
//! `config/ember.example.toml`) and merged with a small set of environment
//! overrides. It is validated eagerly so that a typo'd memory budget fails
//! at startup rather than at the first tenant activation.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::wasi::sandbox::SandboxLimits;

/// Top-level runtime configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Address the HTTP ingress binds to. Overridable via `EMBER_BIND`.
    #[serde(default = "Config::default_bind")]
    pub bind: SocketAddr,

    /// Tenants provisioned at startup.
    #[serde(default)]
    pub tenants: Vec<TenantConfig>,

    /// Global engine configuration.
    #[serde(default)]
    pub engine: EngineConfig,

    /// OCI registry configuration.
    #[serde(default)]
    pub registry: RegistryConfig,

    /// HTTP ingress configuration.
    #[serde(default)]
    pub http: HttpConfig,
}

/// One tenant: a name, the source of its component, and its sandbox limits.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TenantConfig {
    /// Tenant id; must match [`crate::runtime::TenantId`] grammar.
    pub name: String,

    /// Where the tenant's component artifact comes from.
    pub component: ComponentSource,

    /// Per-tenant resource ceilings.
    #[serde(default)]
    pub limits: SandboxLimits,

    /// Per-tenant configuration exposed to the guest via `ember:runtime/tenant`.
    #[serde(default)]
    pub config: std::collections::HashMap<String, String>,
}

/// Where a tenant's component artifact is resolved from.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ComponentSource {
    /// Pull from an OCI registry; optionally pinned to a content digest.
    #[serde(rename = "oci")]
    Oci {
        /// `registry/repo:tag` or `registry/repo@sha256:...`.
        reference: String,
        /// Optional `sha256:<hex>` pin; the pin wins over `reference`'s tag.
        #[serde(default)]
        pin: Option<String>,
    },
    /// Load from a local file (development / offline deployments).
    #[serde(rename = "file")]
    File {
        /// Path to a `.wasm` component artifact.
        path: PathBuf,
    },
}

/// Global engine budget shared by the pooled allocator.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EngineConfig {
    /// Total guest memory across all tenants.
    #[serde(deserialize_with = "deserialize_memory")]
    pub max_memory_bytes: usize,
    /// Maximum number of concurrently live instances.
    pub max_instances: usize,
    /// Compilation threads.
    pub threads: usize,
    /// Interval at which the background task bumps the epoch counter.
    pub epoch_interval_ms: u64,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 1024 * 1024 * 1024,
            max_instances: 64,
            threads: 4,
            epoch_interval_ms: 100,
        }
    }
}

impl EngineConfig {
    /// Clamp every tenant's limits into the engine budget.
    pub fn clamp_tenants(&self, tenants: &mut [TenantConfig]) {
        for t in tenants {
            t.limits.clamp_to(self.max_memory_bytes, self.max_instances);
        }
    }
}

/// OCI registry client configuration.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RegistryConfig {
    /// Default registry endpoint, e.g. `ghcr.io` or `registry.example.com`.
    pub endpoint: String,
    /// Allow plain-HTTP pulls (private mirrors only).
    pub insecure: bool,
    /// How often the hot-reload loop re-resolves tags.
    pub pull_interval_secs: u64,
    /// Local content-addressable cache directory.
    pub cache_dir: PathBuf,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            endpoint: "ghcr.io".to_string(),
            insecure: false,
            pull_interval_secs: 30,
            cache_dir: PathBuf::from("registry-cache"),
        }
    }
}

impl RegistryConfig {
    /// Poll interval for the hot-reload loop.
    pub fn pull_interval(&self) -> Duration {
        Duration::from_secs(self.pull_interval_secs.max(1))
    }
}

/// HTTP ingress tuning.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HttpConfig {
    /// Per-request wall-clock budget.
    pub timeout_secs: u64,
    /// In-flight request ceiling.
    pub concurrency: usize,
    /// Maximum request body size, enforced before dispatch.
    #[serde(deserialize_with = "deserialize_memory")]
    pub max_body_bytes: usize,
    /// Header name used to surface the generated request id.
    pub request_id_header: String,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            timeout_secs: 30,
            concurrency: 512,
            max_body_bytes: 8 * 1024 * 1024,
            request_id_header: "x-request-id".to_string(),
        }
    }
}

impl HttpConfig {
    /// Per-request wall-clock budget as a `Duration`.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs.max(1))
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: Self::default_bind(),
            tenants: Vec::new(),
            engine: EngineConfig::default(),
            registry: RegistryConfig::default(),
            http: HttpConfig::default(),
        }
    }
}

impl Config {
    fn default_bind() -> SocketAddr {
        "127.0.0.1:8080".parse().expect("static address")
    }

    /// Parse `size` as an integer byte count or a human size suffix:
    /// `k`/`m`/`g` (powers of 1024), e.g. `512`, `64k`, `128m`, `1g`.
    ///
    /// ```
    /// use ember::config::parse_memory;
    /// assert_eq!(parse_memory("512").unwrap(), 512);
    /// assert_eq!(parse_memory("64k").unwrap(), 64 * 1024);
    /// assert_eq!(parse_memory("128m").unwrap(), 128 * 1024 * 1024);
    /// ```
    pub fn parse_memory(size: &str) -> Result<usize> {
        parse_memory(size)
    }

    /// Load configuration from a TOML file.
    pub fn load(path: Option<&Path>) -> Result<Config> {
        let mut config = match path {
            Some(p) => {
                let raw = std::fs::read_to_string(p)
                    .map_err(|e| Error::Config(format!("cannot read {}: {e}", p.display())))?;
                toml::from_str(&raw)
                    .map_err(|e| Error::Config(format!("invalid TOML in {}: {e}", p.display())))?
            }
            None => Config::default(),
        };
        config.apply_env();
        config.validate()?;
        Ok(config)
    }

    /// Layer environment overrides on top of file configuration.
    pub fn apply_env(&mut self) {
        if let Ok(bind) = std::env::var("EMBER_BIND") {
            match bind.parse::<SocketAddr>() {
                Ok(addr) => self.bind = addr,
                Err(e) => tracing::warn!(%bind, error = %e, "ignoring invalid EMBER_BIND"),
            }
        }
        if let Ok(endpoint) = std::env::var("EMBER_REGISTRY_ENDPOINT") {
            self.registry.endpoint = endpoint;
        }
        if let Ok(interval) = std::env::var("EMBER_PULL_INTERVAL_SECS") {
            if let Ok(v) = interval.parse() {
                self.registry.pull_interval_secs = v;
            }
        }
    }

    /// Validate the config and clamp tenant limits into the engine budget.
    pub fn validate(&mut self) -> Result<()> {
        if self.engine.max_memory_bytes == 0 {
            return Err(Error::Config("engine.max_memory_bytes must be > 0".into()));
        }
        if self.engine.max_instances == 0 {
            return Err(Error::Config("engine.max_instances must be > 0".into()));
        }
        for t in &self.tenants {
            t.limits.validate()?;
            crate::runtime::TenantId::new(&t.name)
                .map_err(|e| Error::Config(format!("tenant `{}`: {e}", t.name)))?;
            match &t.component {
                ComponentSource::Oci { reference, pin } => {
                    if reference.is_empty() {
                        return Err(Error::Config(format!(
                            "tenant `{}`: empty oci reference",
                            t.name
                        )));
                    }
                    if let Some(pin) = pin {
                        let _ = crate::registry::oci::parse_digest(pin).map_err(|e| {
                            Error::Config(format!("tenant `{}`: bad pin: {e}", t.name))
                        })?;
                    }
                }
                ComponentSource::File { path } => {
                    if path.as_os_str().is_empty() {
                        return Err(Error::Config(format!(
                            "tenant `{}`: empty file path",
                            t.name
                        )));
                    }
                }
            }
        }
        self.engine.clamp_tenants(&mut self.tenants);
        Ok(())
    }
}

/// Parse a human-readable byte count (see [`Config::parse_memory`]).
pub fn parse_memory(size: &str) -> Result<usize> {
    let s = size.trim();
    if s.is_empty() {
        return Err(Error::Config("empty memory size".into()));
    }
    let (digits, unit) = match s.find(|c: char| !c.is_ascii_digit()) {
        Some(idx) => s.split_at(idx),
        None => (s, ""),
    };
    if digits.is_empty() {
        return Err(Error::Config(format!("invalid memory size `{size}`")));
    }
    let value: usize = digits
        .parse()
        .map_err(|_| Error::Config(format!("invalid memory size `{size}`")))?;
    let multiplier = match unit.to_ascii_lowercase().as_str() {
        "" => 1,
        "k" | "kb" | "kib" => 1024,
        "m" | "mb" | "mib" => 1024 * 1024,
        "g" | "gb" | "gib" => 1024 * 1024 * 1024,
        other => return Err(Error::Config(format!("unknown memory unit `{other}`"))),
    };
    value
        .checked_mul(multiplier)
        .ok_or_else(|| Error::Config(format!("memory size `{size}` overflows")))
}

/// Deserialize a memory budget as either a raw integer or a human-size
/// string (`"256m"`, `"64k"`). Applies to TOML/YAML/JSON inputs alike and to
/// any `usize` field that carries a byte count (`engine`, `http`, `limits`).
pub(crate) fn deserialize_memory<'de, D>(deserializer: D) -> std::result::Result<usize, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum MemSize {
        Bytes(u64),
        Text(String),
    }
    match MemSize::deserialize(deserializer)? {
        MemSize::Bytes(b) => usize::try_from(b).map_err(serde::de::Error::custom),
        MemSize::Text(t) => parse_memory(&t).map_err(serde::de::Error::custom),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_and_suffixed_sizes() {
        assert_eq!(parse_memory("0").unwrap(), 0);
        assert_eq!(parse_memory("512").unwrap(), 512);
        assert_eq!(parse_memory("64k").unwrap(), 64 * 1024);
        assert_eq!(parse_memory("64K").unwrap(), 64 * 1024);
        assert_eq!(parse_memory("128m").unwrap(), 128 * 1024 * 1024);
        assert_eq!(parse_memory("2g").unwrap(), 2 * 1024 * 1024 * 1024);
    }

    #[test]
    fn rejects_garbage_sizes() {
        assert!(parse_memory("").is_err());
        assert!(parse_memory("abc").is_err());
        assert!(parse_memory("12z").is_err());
        assert!(parse_memory("-5").is_err());
    }

    #[test]
    fn detects_overflow() {
        assert!(parse_memory("999999999999999999999999g").is_err());
    }

    #[test]
    fn rejects_unknown_tenant_config_keys() {
        let raw = r#"
            bind = "127.0.0.1:8080"
            [[tenants]]
            name = "ok"
            unexpected = true
            [tenants.component]
            kind = "oci"
            reference = "ghcr.io/acme/echo:latest"
        "#;
        assert!(toml::from_str::<Config>(raw).is_err());
    }
}
