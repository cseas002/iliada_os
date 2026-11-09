# ADR 0003: Permission broker and IPC design

**Status:** accepted (Phase 1)

## Decision
* One broker socket per app; identity comes from the socket (unforgeable),
  not from guest messages.
* Length-prefixed postcard frames: compact, serde-native, and with no schema
  compiler. Rejected alternatives: D-Bus (large attack surface, no identity
  over vsock), gRPC/protobuf (heavy dependency for a tiny protocol), JSON
  (no binary payloads, ambiguity).
* Declaration (install-time upper bound) plus prompt (runtime consent),
  following Android. Undeclared requests are denied silently, so apps can't
  nag users.
* Resources are mediated handles; the guest never sees host paths or devices.
* Semantics mirror xdg-desktop-portal, so Tier-1 apps using portals can be
  bridged to the same broker later.

## Consequences
The broker is security-critical and must stay small. It is a separate crate
with no network access (`PrivateNetwork=yes`) and an audit log.
