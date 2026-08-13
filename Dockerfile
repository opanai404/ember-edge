# ─────────────────────────────────────────────────────────────
# EMBER · WebAssembly edge runtime for the component model
# SPDX-License-Identifier: MIT
# ─────────────────────────────────────────────────────────────

# ── Build stage ──────────────────────────────────────────────
FROM rust:1.90-bookworm AS build

WORKDIR /build

# Cache dependencies first: copy sources and compile before adding new layers,
# so incremental image builds reuse the crate cache.
COPY Cargo.toml rust-toolchain.toml ./
COPY src ./src
COPY wit ./wit
COPY tests ./tests
COPY examples ./examples
COPY benches ./benches

# Cargo.lock is intentionally untracked (see .gitignore), so the build must
# resolve rather than fail on a missing lockfile.
RUN cargo build --release

# ── Runtime stage ────────────────────────────────────────────
FROM gcr.io/distroless/cc-debian12:nonroot

WORKDIR /app

COPY --from=build /build/target/release/ember /usr/local/bin/ember
COPY config /etc/ember

ENV EMBER_BIND=0.0.0.0:8080
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/ember"]
CMD ["--config", "/etc/ember/ember.example.toml"]
