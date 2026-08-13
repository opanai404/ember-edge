// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Loader and contract-checking tests.

use ember::component::loader::{
    HANDLER_INTERFACE, Loaded, contract_violations, digest_of, ensure_contract,
};
use ember::registry::store::digest;

/// Minimal valid component binary: the component magic + version bytes and
/// no sections. `wasm-tools` accepts this as a well-formed (empty) component.
const EMPTY_COMPONENT: &[u8] = &[0x00, 0x61, 0x73, 0x6d, 0x0d, 0x00, 0x01, 0x00];

#[test]
fn rejects_truncated_bytes() {
    let err = Loaded::from_bytes("truncated", vec![0x00, 0x61, 0x73]).unwrap_err();
    assert!(matches!(
        err,
        ember::Error::Validation(_) | ember::Error::InvalidComponent { .. }
    ));
}

#[test]
fn rejects_non_component_magic() {
    let err = Loaded::from_bytes(
        "core-module",
        vec![0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00],
    )
    .unwrap_err();
    assert!(err.to_string().contains("component") || err.to_string().contains("validation"));
}

#[test]
fn accepts_valid_empty_component() {
    let loaded = Loaded::from_bytes("empty", EMPTY_COMPONENT.to_vec()).unwrap();
    assert_eq!(loaded.digest, digest(EMPTY_COMPONENT));
    assert_eq!(loaded.size, EMPTY_COMPONENT.len() as u64);
    assert!(loaded.exports.is_empty());
    assert!(loaded.world_id.is_none());
}

#[test]
fn digest_of_matches_store_digest() {
    assert_eq!(digest_of(EMPTY_COMPONENT), digest(EMPTY_COMPONENT));
}

#[test]
fn contract_rejects_missing_handler() {
    let loaded = Loaded::from_bytes("empty", EMPTY_COMPONENT.to_vec()).unwrap();
    let violation = contract_violations(&loaded.exports).expect("must report a violation");
    assert!(violation.contains("handler"), "got: {violation}");
    assert!(matches!(
        ensure_contract(&loaded),
        Err(ember::Error::Contract(_))
    ));
}

#[test]
fn contract_accepts_bare_handler_interface() {
    let exports = vec!["handler".to_string()];
    assert!(contract_violations(&exports).is_none());
}

#[test]
fn contract_accepts_fully_qualified_handler() {
    let exports = vec!["ember:runtime/handler@0.1.0".to_string()];
    assert!(contract_violations(&exports).is_none());
}

#[test]
fn contract_accepts_world_id_export() {
    let exports = vec!["#ember-guest".to_string()];
    assert!(contract_violations(&exports).is_none());
}

#[test]
fn contract_rejects_unrelated_exports() {
    let exports = vec![
        "wasi:http/incoming-handler".to_string(),
        "some-module".to_string(),
    ];
    let violation = contract_violations(&exports).unwrap();
    assert!(violation.contains(&format!("only: {}", exports.join(", "))));
}

#[test]
fn world_id_is_stripped_from_export_names() {
    let exports = vec!["#ember-guest".to_string()];
    let loaded = Loaded {
        source: "synthetic".into(),
        bytes: EMPTY_COMPONENT.into(),
        digest: digest(EMPTY_COMPONENT),
        size: EMPTY_COMPONENT.len() as u64,
        world_id: exports[0].strip_prefix('#').map(str::to_owned),
        exports,
    };
    assert!(loaded.has_world("ember-guest"));
    assert!(loaded.exports_interface("ember-handler") == false);
    assert_eq!(loaded.world_id.as_deref(), Some("ember-guest"));
}

#[test]
fn handler_interface_constant_is_sane() {
    assert_eq!(HANDLER_INTERFACE, "ember-handler");
}
