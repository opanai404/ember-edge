// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! Wasmtime engine construction.
//!
//! Ember uses a single engine shared by every tenant with the **pooling
//! allocator** (fixed memory/instance limits, no per-instance host malloc),
//! **epoch interruption** (cooperative time budget via a background ticker),
//! and **async support** for the component-model / `wasi:http` stack.

use std::time::Duration;

use tokio::time::MissedTickBehavior;
use wasmtime::{
    Config as WasmConfig, Engine, InstanceAllocationStrategy, OptLevel, WasmBacktraceDetails,
    pooling,
};

use crate::error::{Error, Result};
use crate::wasi::sandbox::SandboxLimits;

/// Global engine capacity. Tenant limits are clamped against these values.
#[derive(Debug, Clone, Copy)]
pub struct EngineCap {
    /// Total guest linear memory across all tenants.
    pub max_memory_bytes: usize,
    /// Maximum concurrently live instances.
    pub max_instances: usize,
    /// Compilation worker threads.
    pub threads: usize,
}

/// Builder for the shared engine.
#[derive(Debug, Clone)]
pub struct EngineBuilder {
    cap: EngineCap,
    /// Epoch tick interval; 0 disables the background ticker.
    epoch_interval: Duration,
}

impl EngineBuilder {
    /// Create a builder with the given global capacity.
    pub fn new(cap: EngineCap) -> Self {
        Self {
            cap,
            epoch_interval: Duration::from_millis(100),
        }
    }

    /// Derive a builder from an engine config (used by the runtime).
    pub fn from_config(config: &crate::config::EngineConfig) -> Self {
        Self::new(EngineCap {
            max_memory_bytes: config.max_memory_bytes,
            max_instances: config.max_instances,
            threads: config.threads,
        })
        .with_epoch_interval(Duration::from_millis(config.epoch_interval_ms))
    }

    /// Set the epoch tick interval.
    pub fn with_epoch_interval(mut self, interval: Duration) -> Self {
        self.epoch_interval = interval;
        self
    }

    /// Compile the engine.
    pub fn build(&self) -> Result<Engine> {
        let mut cfg = WasmConfig::new();

        cfg.cranelift_opt_level(OptLevel::SpeedAndSize);
        cfg.parallel_compilation(true);
        cfg.wasm_component_model(true);
        cfg.wasm_function_references(true);
        cfg.wasm_backtrace_details(WasmBacktraceDetails::Disable);

        // Async is required for the component model + wasi:http stack.
        cfg.async_support(true);

        // Cooperative time budgets: a background task bumps the epoch and
        // instances check it on each loop iteration / call return.
        cfg.epoch_interruption(true);

        // Pooling allocator: fixed-size instance pool, no lazy host alloc.
        let pages_per_instance = (self.cap.max_memory_bytes / (64 * 1024)).max(1);
        cfg.allocation_strategy(InstanceAllocationStrategy::Pooling(
            pooling::Config::new()
                .total_memory_pages(pages_per_instance)
                .instance_memory_limit(self.cap.max_memory_bytes)
                .total_instances(self.cap.max_instances)
                .max_tables_per_instance(32),
        ));

        // Keep the best single instance of each component compiled.
        cfg.cache_config_load_default();

        Ok(Engine::new(&cfg)?)
    }

    /// Spawn the epoch ticker for this engine. The returned task should live
    /// for the lifetime of the runtime.
    pub fn spawn_epoch_ticker(self, engine: &Engine) -> tokio::task::JoinHandle<()> {
        let engine = engine.clone();
        let interval = self.epoch_interval;
        if interval.is_zero() {
            return tokio::spawn(std::future::pending());
        }
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                engine.increment_epoch();
            }
        })
    }
}

/// Per-tenant engine-level budgets derived from sandbox limits.
pub fn engine_limits(limits: &SandboxLimits) -> (usize, usize) {
    (limits.max_memory_bytes, limits.max_instances)
}

/// Convenience engine for tooling and test harnesses that need a small,
/// default-configured instance.
pub fn build_default_engine() -> Result<Engine> {
    EngineBuilder::new(EngineCap {
        max_memory_bytes: 256 * 1024 * 1024,
        max_instances: 8,
        threads: 2,
    })
    .build()
    .map_err(|e| Error::Wasm(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_per_instance_at_least_one() {
        let cap = EngineCap {
            max_memory_bytes: 0,
            max_instances: 1,
            threads: 1,
        };
        let pages = (cap.max_memory_bytes / (64 * 1024)).max(1);
        assert_eq!(pages, 1);
    }

    #[test]
    fn engine_limits_forward_sandbox_budget() {
        let l = SandboxLimits {
            max_memory_bytes: 64 * 1024 * 1024,
            max_instances: 4,
            ..SandboxLimits::default()
        };
        assert_eq!(engine_limits(&l), (64 * 1024 * 1024, 4));
    }
}
