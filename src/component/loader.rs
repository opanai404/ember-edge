// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Component artifact loading, validation, and contract checking.
//!
//! A [`Loaded`] artifact is a component binary that has passed structural
//! validation (`wasm-tools`) and has a recorded content digest. Contract
//! checking ([`ensure_contract`]) is a separate, cheap step that verifies the
//! artifact exposes the `ember-guest` handler surface before it is ever
//! scheduled.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use wasm_tools::{Component, Validate, Validator};

use crate::error::{Error, Result};

/// The world id a component must target to be scheduled by Ember.
pub const EMBER_WORLD: &str = "ember-guest";

/// The exported interface a component uses to receive requests.
pub const HANDLER_INTERFACE: &str = "ember-handler";

/// A validated, content-addressed component artifact.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// Human-readable origin (file path or OCI reference).
    pub source: String,
    /// Raw component bytes.
    pub bytes: Arc<[u8]>,
    /// Content digest, `sha256:<hex>`, computed over `bytes`.
    pub digest: String,
    /// Size of the artifact in bytes.
    pub size: u64,
    /// The world id embedded in the component (`#ember-guest` → `ember-guest`).
    pub world_id: Option<String>,
    /// Export names declared by the component.
    pub exports: Vec<String>,
}

impl Loaded {
    /// Validate `bytes` as a component and derive metadata.
    pub fn from_bytes(source: impl Into<String>, bytes: Vec<u8>) -> Result<Self> {
        let source = source.into();

        let mut validator = Validator::new();
        validator
            .validate_all(&bytes)
            .map_err(|e| Error::Validation(e.to_string()))?;

        let component = Component::new(bytes.as_slice()).map_err(|e| Error::InvalidComponent {
            path: source.clone(),
            reason: e.to_string(),
        })?;

        let exports: Vec<String> = component.exports().map(|name| name.to_string()).collect();

        // A fully-linked component carries its world id as an export named
        // `#<world-id>`; `#ember-guest` is what we look for.
        let world_id = exports
            .iter()
            .find_map(|e| e.strip_prefix('#').map(str::to_owned));

        let digest = digest_of(&bytes);
        let size = bytes.len() as u64;

        Ok(Self {
            source,
            bytes: Arc::from(bytes),
            digest,
            size,
            world_id,
            exports,
        })
    }

    /// Load and validate a component artifact from disk.
    pub fn from_path(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| Error::InvalidComponent {
            path: path.display().to_string(),
            reason: e.to_string(),
        })?;
        Self::from_bytes(path.display().to_string(), bytes)
    }

    /// Whether the artifact declares export `name` (exact match).
    pub fn exports_interface(&self, name: &str) -> bool {
        self.exports.iter().any(|e| e == name)
    }

    /// Whether the artifact targets world `world`.
    pub fn has_world(&self, world: &str) -> bool {
        self.world_id.as_deref() == Some(world)
    }
}

/// Compute the content digest for `bytes`.
pub fn digest_of(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Return a human-readable description of what is missing from `exports`,
/// or `None` if the exports satisfy the Ember guest contract.
///
/// A component satisfies the contract if it exports the handler interface
/// (under its fully-qualified name, its bare name, or the `ember-guest`
/// world). This is intentionally tolerant of the two canonical encodings
/// produced by `wasm-tools component` / `wit-bindgen`.
pub fn contract_violations(exports: &[String]) -> Option<String> {
    let has_world = exports
        .iter()
        .any(|e| e == EMBER_WORLD || e == &format!("#{EMBER_WORLD}"));
    let has_interface = exports
        .iter()
        .any(|e| e == HANDLER_INTERFACE || e == "handler" || e.contains(":runtime/handler"));

    if has_world || has_interface {
        None
    } else {
        let seen = if exports.is_empty() {
            "no exports at all".to_string()
        } else {
            format!("only: {}", exports.join(", "))
        };
        Some(format!(
            "artifact exports {seen}; expected interface `{HANDLER_INTERFACE}` or world `{EMBER_WORLD}`"
        ))
    }
}

/// Ensure a loaded artifact satisfies the Ember guest contract.
pub fn ensure_contract(loaded: &Loaded) -> Result<()> {
    match contract_violations(&loaded.exports) {
        None => Ok(()),
        Some(reason) => Err(Error::Contract(reason)),
    }
}
