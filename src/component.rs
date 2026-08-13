// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Component model machinery.
//!
//! [`loader`] turns untrusted bytes into a validated, content-addressed
//! [`loader::Loaded`] artifact and enforces the `ember-guest` contract.
//! [`linker`] assembles the host linker (WASI preview 2 + `wasi:http` +
//! `ember:runtime` imports) and generates the ABI types from `wit/ember.wit`.

pub mod linker;
pub mod loader;

pub use loader::{Loaded, contract_violations, digest_of, ensure_contract};
