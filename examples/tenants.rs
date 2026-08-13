// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Multi-tenant sandbox isolation walkthrough.
//!
//! Shows the host-side enforcement primitives: per-tenant scratch quotas,
//! memory ceilings, and egress allowlists — all of which are applied to a
//! tenant's component at activation time.
//!
//! ```sh
//! cargo run --example tenants
//! ```

use std::collections::HashMap;
use std::path::Path;

use ember::runtime::TenantId;
use ember::wasi::sandbox::{Sandbox, SandboxLimits};

#[tokio::main]
async fn main() -> ember::Result<()> {
    let limits_a = SandboxLimits {
        max_memory_bytes: 64 * 1024 * 1024,
        memfs_quota_bytes: 1024,
        egress_allow: vec!["*.example.com".to_string()],
        ..SandboxLimits::default()
    };
    let limits_b = SandboxLimits {
        max_memory_bytes: 32 * 1024 * 1024,
        ..SandboxLimits::default()
    };

    let alpha = Sandbox::new(TenantId::new("alpha")?, limits_a, HashMap::new())?;
    let beta = Sandbox::new(TenantId::new("beta")?, limits_b, HashMap::new())?;

    // Scratch storage is per-sandbox and byte-accounted.
    alpha
        .scratch
        .write(Path::new("secret"), b"alpha-only".to_vec())?;
    println!(
        "alpha scratch uses {} of {} bytes",
        alpha.scratch_bytes(),
        alpha.limits.memfs_quota_bytes
    );
    println!(
        "beta cannot read alpha's scratch: {}",
        !beta.scratch.exists(Path::new("secret"))
    );

    // A write that exceeds alpha's scratch quota is refused by the host.
    let oversized = vec![0u8; 2048];
    match alpha.scratch.write(Path::new("secret"), oversized) {
        Ok(()) => println!("unexpected: quota allowed the write"),
        Err(e) => println!("quota enforced: {e}"),
    }

    // Egress: alpha may reach example.com, beta (deny-by-default) may not.
    println!(
        "alpha egress api.example.com → {}",
        alpha.egress_policy().allows("api.example.com")
    );
    println!(
        "beta egress api.example.com → {}",
        beta.egress_policy().allows("api.example.com")
    );

    // Memory ceiling is enforced before an instance is created.
    println!(
        "alpha instance budget: {} bytes",
        alpha.limits.max_memory_bytes
    );

    Ok(())
}
