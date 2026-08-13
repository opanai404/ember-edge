// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Minimal Ember deployment: serve a configured tenant set locally.
//!
//! ```sh
//! cargo run --example minimal -- config/ember.example.toml
//! curl -s localhost:8080/healthz
//! ```

use std::path::PathBuf;

use ember::config::Config;
use ember::runtime::Runtime;

#[tokio::main]
async fn main() -> ember::Result<()> {
    let path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/ember.example.toml"));

    let config = Config::load(Some(&path))?;
    let runtime = Runtime::build(config).await?;

    println!(
        "ember v{} serving {} tenant(s) on {}",
        ember::VERSION,
        runtime.tenant_count(),
        runtime.bind
    );

    runtime.serve(runtime.bind).await
}
