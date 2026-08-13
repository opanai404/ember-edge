<!--
  EMBER · WebAssembly edge runtime for the component model
  SPDX-License-Identifier: MIT
-->

# Security Policy

Ember runs untrusted WebAssembly at the edge. Sandbox escapes, WASI
confinement bugs, and digest/registry attacks are all treated as high
severity.

## Reporting a vulnerability

**Do not open a public GitHub issue.** Report privately so we can fix and
release before the details become public.

- Primary channel: [GitHub security advisory](https://github.com/hrniu/ember-edge/security/advisories/new)
  against this repository.
- Alternate channel: security@hrniu.dev (monitored continuously).

Please include:

- The affected version(s) and platform.
- A minimal reproducer (component bytes or a scenario description).
- Impact assessment, if known (escape vs. denial of service vs. data leak).

You will receive an acknowledgement within 48 hours and a triage decision
within 5 business days.

## Supported versions

| Version | Supported          |
|---------|--------------------|
| 0.1.x   | Yes (current)      |
| < 0.1   | No                 |

Only the most recent minor release receives security backports. If you rely
on a specific digest-pinned component or a pinned registry endpoint, pin the
`ember-edge` release to match your rollouts.

## Security model

- Guest components are **untrusted**. The trust boundary is the component
  ABI: imports must be satisfied by the host or explicitly denied, and
  egress is deny-by-default (see `EgressPolicy`).
- The **pinned digest** of every active component is recorded and exposed on
  `/metrics`; a reload never swaps a component whose digest does not match
  its pin.
- Registry pulls are always over TLS unless `insecure = true` is explicitly
  configured for a private mirror, and every layer is verified against the
  manifest digest.
