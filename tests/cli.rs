// ─────────────────────────────────────────────────────────────
// EMBER · WebAssembly edge runtime for the component model
// SPDX-License-Identifier: MIT
// ─────────────────────────────────────────────────────────────
//! CLI contract tests for the `ember` binary.

use assert_cmd::Command;
use predicates::prelude::*;

fn ember() -> Command {
    Command::cargo_bin("ember").expect("binary must build")
}

#[test]
fn version_flag_prints_version() {
    ember()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn help_flag_prints_usage() {
    ember()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("ember"))
        .stdout(predicate::str::contains("--config"));
}

#[test]
fn check_flag_accepts_valid_config() {
    ember()
        .args(["--check", "--config", "config/ember.example.toml"])
        .assert()
        .success()
        .stdout(predicate::str::contains("config OK"));
}

#[test]
fn check_flag_rejects_missing_config() {
    ember()
        .args(["--check", "--config", "does-not-exist.toml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot read"));
}
