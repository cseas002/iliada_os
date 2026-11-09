# ADR 0002: Two isolation tiers; cloud-hypervisor for micro-VMs

**Status:** accepted (Phase 1)

## Context
The draft proposed KVM/Firecracker micro-VMs for untrusted apps.

## Decision
* Tier 2 uses **cloud-hypervisor**: Rust, KVM, virtio-fs, vsock and
  vhost-user, with a path to virtio-gpu cross-domain display. Firecracker
  lacks virtio-fs, GPU and display devices by design (it targets serverless).
  crosvm is the strongest display option but harder to embed; we keep it as
  the fallback if cloud-hypervisor's display path stalls.
* Tier 1 uses bubblewrap namespaces for first-party / reviewed apps, where a
  full VM per app would be too costly.
* Both tiers expose exactly one channel to the host: the broker.

## Consequences
* RAM cost per Tier-2 app (~100–200 MiB baseline) is accepted for untrusted code.
* GPU acceleration in Tier 2 depends on virtio-gpu work in Phase 2.
