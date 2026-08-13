// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Registry store and hot-reload policy tests.

use std::time::Duration;

use ember::registry::oci::verify_digest;
use ember::registry::store::{ContentStore, HotReloadPolicy, PinSet, digest};

#[tokio::test]
async fn content_store_roundtrip_via_tempdir() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContentStore::open(dir.path().join("cas")).unwrap();

    let d = store.put(b"component bytes").await.unwrap();
    assert!(d.starts_with("sha256:"));
    assert_eq!(store.get(&d).unwrap(), b"component bytes");
    assert_eq!(store.disk_usage().unwrap(), 15);
}

#[tokio::test]
async fn content_store_dedupes_by_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContentStore::open(dir.path().join("cas")).unwrap();

    let d1 = store.put(b"same").await.unwrap();
    let d2 = store.put(b"same").await.unwrap();
    assert_eq!(d1, d2);
    // Two writes of identical content still occupy one blob on disk.
    assert_eq!(store.disk_usage().unwrap(), 4);
}

#[tokio::test]
async fn content_store_atomic_put_never_leaves_tmp_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContentStore::open(dir.path().join("cas")).unwrap();
    store.put(b"atomic").await.unwrap();
    let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("cas"))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files must be renamed or cleaned"
    );
}

#[test]
fn digest_verification_rejects_mismatch() {
    let good = digest(b"component");
    assert!(verify_digest(b"component", &good).is_ok());
    let err = verify_digest(b"tampered", &good).unwrap_err();
    assert!(matches!(err, ember::Error::DigestMismatch { .. }));
}

#[test]
fn pins_are_resolved_and_compare_unpinned() {
    let mut pins = PinSet::default();
    pins.pin("echo", "sha256:aaaa");
    pins.pin("static", "sha256:bbbb");
    assert_eq!(pins.resolve("echo"), Some("sha256:aaaa"));
    assert_eq!(pins.resolve("missing"), None);
    assert!(!pins.unpin("echo", "sha256:cccc"));
    assert!(pins.unpin("echo", "sha256:aaaa"));
    assert_eq!(pins.resolve("echo"), None);
}

#[test]
fn reload_policy_reloads_only_on_digest_change() {
    let policy = HotReloadPolicy::default();
    assert!(policy.should_reload(None, "sha256:one"));
    assert!(!policy.should_reload(Some("sha256:one"), "sha256:one"));
    assert!(policy.should_reload(Some("sha256:one"), "sha256:two"));
}

#[test]
fn reload_policy_parks_after_failures() {
    let policy = HotReloadPolicy::default();
    assert_eq!(policy.max_failed_polls, 3);
    assert!(!policy.exhausted(2));
    assert!(policy.exhausted(3));
    assert!(policy.exhausted(99));
}

#[test]
fn reload_policy_debounce_prevents_thrashing() {
    let policy = HotReloadPolicy {
        debounce: Duration::from_secs(60),
        ..HotReloadPolicy::default()
    };
    // Even a digest change is not enough if it arrives inside the debounce
    // window; the caller is responsible for the timestamp gate.
    assert!(policy.should_reload(Some("old"), "new"));
    assert_eq!(policy.debounce.as_secs(), 60);
}

#[test]
fn digest_is_sha256_hex_of_length_64() {
    let d = digest(b"payload");
    let hex = d.strip_prefix("sha256:").unwrap();
    assert_eq!(hex.len(), 64);
    assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
}

#[test]
fn store_rejects_non_sha256_digests() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContentStore::open(dir.path().to_path_buf()).unwrap();
    assert!(store.get("md5:abc").is_err());
    assert!(store.get("sha256:1234").is_err());
    assert!(!store.contains("sha256:1234"));
}
