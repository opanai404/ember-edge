<!--
  EMBER · WebAssembly edge runtime for the component model
  SPDX-License-Identifier: MIT
-->

# Design: hot reload from an OCI registry

Status: implemented (v0.1.0). Related code: `src/runtime.rs`
(`spawn_reload_loop`, `poll_once`, `swap`), `src/registry/oci.rs`,
`src/registry/store.rs`.

## Problem

Edge components must update without a restart and without dropping
in-flight traffic. The update channel is an OCI registry — the same one used
to ship the runtime itself — and the update unit is a content-addressed
component digest.

The hard constraints:

1. **Atomicity.** A reload must never leave a tenant running a half-loaded
   component or a mix of two generations.
2. **Reproducibility.** A deployment pinned to a digest must serve exactly
   that digest until an operator moves the pin.
3. **Safety under failure.** A registry outage must not take a healthy tenant
   down with it.

## Design

### The active component is an `ArcSwap`

Each tenant holds `active: ArcSwap<ActiveComponent>`, where `ActiveComponent`
carries the validated artifact, its compiled form, and the active digest.
All readers (the invoke path) load the current generation with one atomic
read and finish on it. A swap is one `store` on the `ArcSwap`; there is no
synchronization point in the request path.

Instance state is **not** swapped eagerly. Each tenant keeps an `InstancePool`
keyed by digest. A request that finds a matching prepared instance reuses it;
a request for the new digest lazily builds a fresh instance and the old
generation drains naturally. This keeps swap latency independent of pool size
and lets us drop the old generation without ever tearing down a running
instance underneath it.

### The digest is the unit of truth

- Pulls go through `OciClient`, which verifies every layer against the
  manifest digest before caching in the `ContentStore` (atomic temp-file +
  rename, so a crash never exposes a partial blob).
- A tenant configured with a `pin` refuses any artifact whose digest does not
  match (`Registry::pull` fails with `DigestMismatch`). Tags are only an
  alias for "whatever digest the registry points at right now".

### The reload loop is conservative

```text
every poll_interval:
  for each tenant with an OCI source:
    if now - last_swap < debounce:            skip (rate limit)
    pull(reference, pin)
      on digest change: validate contract → compile → swap → reset failures
      on failure:       failures += 1
                        if failures >= max_failed_polls: park (log + metric)
```

Three independent guards prevent reload churn:

- **Digest equality** (`HotReloadPolicy::should_reload`) — republishing the
  same bytes is a no-op.
- **Debounce** — at most one swap per tenant per `debounce` window, so a
  noisy registry cannot cause a swap storm.
- **Failure budget** (`max_failed_polls`) — after three consecutive failures
  the tenant is parked on its last known good digest. It stops consuming
  retry budget and the condition is visible in `/metrics`
  (`ember.reload.parked`) and the logs.

## Failure modes

| Failure | Behavior | Why it is safe |
|---------|----------|----------------|
| Registry unreachable | Pull fails; failure counter increments; tenant keeps serving the active generation. | Reads never depend on the registry once an artifact is pinned/cached. |
| Tag moved to a broken artifact | Contract check fails during `poll_once`; swap is refused; previous generation stays active. | `ensure_contract` runs before the `ArcSwap` store. |
| Pin removed upstream (blob GC'd) | Pull of the pinned digest fails like any other pull. | Operators pin digests they also GC-protect; the content store is a second copy. |
| Crash mid-swap | The store is a single atomic pointer; there is no partial state to observe. | `ArcSwap` + atomic temp-file writes. |
| Request in flight during swap | Finishes on the old generation; next request gets the new one. | Generation snapshots via `ArcSwap::load_full`. |

## Alternatives considered

- **`Notify`-based file watching** for local deployments — kept out of the
  v0.1 loop in favor of a single OCI-polling path; a `file` component source
  exists for offline dev but is loaded once at activation.
- **Eager instance rebuild on swap** — rejected: it moves pool work onto the
  critical path of a deploy and can stall in-flight requests that share the
  same store pool.
- **Per-request pull** — rejected: it would make the registry a request-path
  dependency. All requests hit only the `ArcSwap` snapshot.
