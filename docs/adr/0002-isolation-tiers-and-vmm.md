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
* **Tier 1 is the default for every app**: bubblewrap namespaces, Landlock and
  seccomp, the equivalent of macOS's App Sandbox. Native speed, no extra memory.
* **Tier 2 is opt-in**, per app ("Extra isolation" in Settings) or for every
  app ("High security mode", `iliada.isolation.highSecurity`). It costs
  ~0.3–1 s at start-up, ~50–150 MB per running app, and graphics speed
  (virtual GPU). The store may recommend it for apps from unverified sources.
* Both tiers expose exactly one channel to the host: the broker.

## Consequences
* Users choose stronger isolation where they want it; everyday apps stay fast.
* GPU acceleration in Tier 2 depends on virtio-gpu work in Phase 2.
