---
title: "ADR 0003: Model-assisted driver conversion and package registry"
type: adr
status: accepted
created: 2026-11-15
updated: 2026-11-15
tags: [drivers, packages, registry, linux]
---

# ADR 0003: Model-assisted driver conversion and package registry

## Context

A new kernel has no drivers. Linux has drivers for almost all hardware, but
they depend on Linux-internal kernel APIs that are not stable across versions.

## Decision

1. A device first runs on a generic class driver so the system keeps working.
2. The OS identifies the device and looks up a converted driver in the package
   registry.
3. If none exists, the equivalent Linux driver for the same architecture is
   converted to the native driver API by a model. Conversion runs in the
   background, never in the path of a running system.
4. The converted driver is tested, published to the registry and installed.
5. The driver server is restarted on the converted driver; no reboot.

One package manager installs both drivers and programs.

## Open questions

- **Who converts and publishes.** Accepting uploads from end-user machines lets
  a compromised machine publish driver code for everyone. Proposed: the device
  only requests a conversion; a registry-side build service converts, tests and
  signs. Not decided yet.
- **Devices with no generic class.** Most network cards have none, and without
  network the registry is unreachable. Proposed: ship converted drivers for
  common network cards in the install image and support USB tethering.
- **Licence.** Linux drivers are GPL-2.0 and a translation is a derivative
  work, so converted drivers are expected to be GPL-2.0 and their source must
  be published. This needs legal confirmation before the OS licence is chosen.

## Consequences

- A native driver API has to exist before any conversion.
- Verification is the bottleneck: differential testing only covers hardware
  that QEMU emulates.
- Registry packages need provenance (Linux commit, model version, test
  results) and a trust level.
