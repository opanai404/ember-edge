// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Micro-benchmarks for the host-side hot paths that do not require a
//! component: contract checking, digest computation, and egress filtering.
//!
//! ```sh
//! cargo bench
//! ```

use std::hint::black_box;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use ember::component::loader::{contract_violations, digest_of};
use ember::http::bridge::EgressPolicy;

/// Benchmark `contract_violations` over a fully-linked export list.
fn bench_contract_check(c: &mut Criterion) {
    let exports: Vec<String> = vec![
        "#ember-guest".into(),
        "wasi:http/incoming-handler".into(),
        "wasi:io/streams".into(),
        "ember:runtime/handler".into(),
    ];
    c.bench_function("contract_check_linked", |b| {
        b.iter(|| black_box(contract_violations(&exports)).is_none())
    });
}

/// Benchmark digesting a 1 MiB component payload.
fn bench_digest_1mib(c: &mut Criterion) {
    let payload = vec![0u8; 1024 * 1024];
    c.bench_function("digest_sha256_1mib", |b| {
        b.iter(|| black_box(digest_of(&payload)))
    });
}

/// Benchmark egress allowlist matching against a ten-rule policy.
fn bench_egress_allowlist(c: &mut Criterion) {
    let policy = EgressPolicy {
        allow: (0..10).map(|i| format!("svc-{i}.example.com")).collect(),
        default_deny: true,
    };
    c.bench_function("egress_allows_miss", |b| {
        b.iter(|| black_box(policy.allows("https://svc-3.example.com:8443")))
    });
    c.bench_function("egress_allows_deny", |b| {
        b.iter(|| black_box(policy.allows("https://unknown.example.org")))
    });
}

/// Benchmark the pure digest + contract path for a single request.
fn bench_request_abi_setup(c: &mut Criterion) {
    let exports = vec!["#ember-guest".into(), "ember:runtime/handler".into()];
    c.bench_function("request_abi_setup", |b| {
        b.iter(|| {
            let ok = contract_violations(&exports).is_none();
            let _ = digest_of(b"payload");
            black_box(ok)
        })
    });
}

criterion_group! {
    name = invoke;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2));
    targets = bench_contract_check, bench_digest_1mib, bench_egress_allowlist, bench_request_abi_setup
}
criterion_main!(invoke);
