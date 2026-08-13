// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Demonstrate digest-driven hot reload against an OCI registry.
//!
//! Pulls a component reference, then polls the registry, reporting each
//! digest transition the way the runtime's reload loop would decide one.
//!
//! ```sh
//! cargo run --example pull_and_reload -- ghcr.io/opanai404/ember/echo:latest
//! ```

use std::time::Duration;

use ember::registry::{HotReloadPolicy, OciClientConfig, Registry};

#[tokio::main]
async fn main() -> ember::Result<()> {
    let reference = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ghcr.io/opanai404/ember/echo:latest".to_string());

    let registry = Registry::new(OciClientConfig::default(), HotReloadPolicy::default())?;

    let mut active: Option<String> = None;
    for round in 0..3 {
        match registry.pull(&reference, None).await {
            Ok(loaded) => {
                println!(
                    "round {round}: resolved `{reference}` → {} ({} bytes)",
                    loaded.digest, loaded.size
                );
                if registry
                    .policy
                    .should_reload(active.as_deref(), &loaded.digest)
                {
                    println!("  → swapping active digest to {}", loaded.digest);
                    active = Some(loaded.digest);
                } else {
                    println!("  → no change; keeping active digest");
                }
            }
            Err(e) => {
                // Mirror the runtime: count failures, park after the budget.
                eprintln!("round {round}: pull failed: {e}");
                if registry.policy.exhausted(round) {
                    eprintln!("  → parked: keeping last known digest");
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    Ok(())
}
