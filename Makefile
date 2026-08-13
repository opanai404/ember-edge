# ─────────────────────────────────────────────────────────────
# EMBER · WebAssembly edge runtime for the component model
# SPDX-License-Identifier: MIT
# ─────────────────────────────────────────────────────────────

RUST ?= cargo
DOCKER ?= docker
WASM_TOOLS ?= wasm-tools

.PHONY: build test lint lint-fix fmt fmt-check clean doc wit-check wit-fmt example docker docker-run help

help:
	@echo "ember-edge targets:"
	@echo "  build       cargo build (debug)"
	@echo "  release     cargo build --release"
	@echo "  test        cargo test --all-features"
	@echo "  lint        cargo clippy --all-targets -- -D warnings"
	@echo "  lint-fix    cargo clippy --fix --allow-dirty"
	@echo "  fmt         cargo fmt"
	@echo "  fmt-check   cargo fmt -- --check"
	@echo "  wit-check   validate wit/ with wasm-tools component wit"
	@echo "  doc         cargo doc --no-deps"
	@echo "  clean       cargo clean"
	@echo "  docker      build the ember-edge image"
	@echo "  docker-run  run the built image on :8080"

build:
	$(RUST) build

release:
	$(RUST) build --release

test:
	$(RUST) test --all-features

lint:
	$(RUST) clippy --all-targets -- -D warnings

lint-fix:
	$(RUST) clippy --fix --allow-dirty

fmt:
	$(RUST) fmt

fmt-check:
	$(RUST) fmt -- --check

wit-check:
	$(WASM_TOOLS) component wit wit/ >/dev/null

doc:
	$(RUST) doc --no-deps

clean:
	$(RUST) clean
	rm -rf $(CURDIR)/data $(CURDIR)/registry-cache

example:
	$(RUST) run --example minimal -- config/ember.example.toml

docker:
	$(DOCKER) build -t ember-edge:0.1.0 .

docker-run:
	$(DOCKER) run --rm -p 8080:8080 \
		-v $(CURDIR)/config:/etc/ember \
		ember-edge:0.1.0 --config /etc/ember/ember.example.toml
