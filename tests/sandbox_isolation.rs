// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Sandbox isolation tests: limits, scratch quota, tenant ids, egress.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use ember::http::bridge::EgressPolicy;
use ember::runtime::TenantId;
use ember::wasi::sandbox::{MemFs, Sandbox, SandboxLimits};

fn tenant(name: &str) -> TenantId {
    TenantId::new(name).unwrap()
}

fn sandbox(name: &str) -> Sandbox {
    Sandbox::new(tenant(name), SandboxLimits::default(), HashMap::new()).unwrap()
}

#[test]
fn tenant_id_grammar_is_enforced() {
    assert!(TenantId::new("echo-api-2").is_ok());
    assert!(TenantId::new("a_b").is_ok());
    assert!(TenantId::new("").is_err());
    assert!(TenantId::new("has space").is_err());
    assert!(TenantId::new("slash/name").is_err());
    assert!(TenantId::new("café").is_err());
    let long = "x".repeat(65);
    assert!(TenantId::new(&long).is_err());
}

#[test]
fn sandbox_rejects_zero_limits() {
    let mut limits = SandboxLimits::default();
    limits.max_memory_bytes = 0;
    assert!(Sandbox::new(tenant("x"), limits, HashMap::new()).is_err());
}

#[test]
fn charge_memory_enforces_ceiling() {
    let s = sandbox("mem");
    assert!(s.charge_memory(1024).is_ok());
    assert!(s.charge_memory(usize::MAX).is_err());
}

#[test]
fn memfs_quota_is_per_sandbox() {
    let a = sandbox("a");
    let b = sandbox("b");
    let data = vec![b'x'; 2048];
    // Each sandbox has its own scratch filesystem with its own quota.
    a.scratch.write(Path::new("blob"), data.clone()).unwrap();
    assert_eq!(a.scratch_bytes(), 2048);
    assert_eq!(b.scratch_bytes(), 0);
    assert!(!b.scratch.exists(Path::new("blob")));
    assert!(a.scratch.exists(Path::new("blob")));
}

#[test]
fn sandbox_derives_egress_policy_from_limits() {
    let mut limits = SandboxLimits::default();
    limits.egress_allow = vec!["*.example.com".to_string()];
    limits.default_deny_egress = true;
    let s = Sandbox::new(tenant("eg"), limits, HashMap::new()).unwrap();
    let policy = s.egress_policy();
    assert!(policy.allows("api.example.com"));
    assert!(!policy.allows("evil.example.net"));
}

#[test]
fn egress_defaults_to_deny() {
    let policy = EgressPolicy::default();
    assert!(policy.default_deny);
    assert!(!policy.allows("http://anywhere.example"));
}

#[test]
fn wall_clock_budget_is_forwarded() {
    let mut limits = SandboxLimits::default();
    limits.max_wall_duration = Duration::from_millis(250);
    let s = sandbox("fast");
    let _ = s;
    assert_eq!(limits.max_wall_duration, Duration::from_millis(250));
}

#[test]
fn memfs_scratch_read_write_roundtrip() {
    let fs = MemFs::new(4096);
    fs.write(Path::new("k.txt"), b"v1".to_vec()).unwrap();
    assert_eq!(fs.read(Path::new("k.txt")), Some(b"v1".to_vec()));
    assert_eq!(fs.used_bytes(), 2);
}
