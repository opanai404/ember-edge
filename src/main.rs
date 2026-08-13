// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Ember runtime binary.
//!
//! Reads a TOML config (see `config/ember.example.toml`), assembles the
//! runtime, and serves the HTTP ingress until a shutdown signal arrives.

use std::path::PathBuf;

use clap::Parser;

use ember::config::Config;
use ember::error::Result;
use ember::telemetry;

/// WebAssembly edge runtime for the component model.
#[derive(Debug, Parser)]
#[command(name = "ember", version, about)]
struct Cli {
    /// Path to the runtime configuration file.
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Emit JSON logs (container-friendly).
    #[arg(long)]
    json: bool,

    /// Log level filter (`info`, `ember=debug,tower_http=info`, ...).
    #[arg(long, env = "EMBER_LOG")]
    log_level: Option<String>,

    /// Validate the configuration and exit.
    #[arg(long)]
    check: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    telemetry::init(cli.log_level.as_deref(), cli.json)?;

    let config = Config::load(cli.config.as_deref())?;

    if cli.check {
        println!(
            "config OK: {} tenant(s), engine {:.1} MiB / {} instances, ingress {}",
            config.tenants.len(),
            config.engine.max_memory_bytes as f64 / (1024.0 * 1024.0),
            config.engine.max_instances,
            config.bind,
        );
        return Ok(());
    }

    let runtime = ember::Runtime::build(config).await?;
    tracing::info!(
        tenants = runtime.tenant_count(),
        bind = %runtime.bind,
        version = ember::VERSION,
        "ember runtime assembled"
    );

    runtime.serve(runtime.bind).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_config_and_check_flags() {
        let cli = Cli::parse_from(["ember", "--check", "--json"]);
        assert!(cli.check);
        assert!(cli.json);
        assert!(cli.config.is_none());
    }
}
