# Iliada IPC protocol v1

Types live in `crates/iliada-ipc/src/proto.rs`; this document is normative
where it states MUST.

## Transport

* **Tier 2 (micro-VM):** the guest opens AF_VSOCK to CID 2 (`HOST_CID`), port
  7000 (`BROKER_VSOCK_PORT`). cloud-hypervisor's hybrid vsock forwards it to
  the host Unix socket `<socket_dir>/<app-id>.vsock_7000`; the VM's vsock device
  socket is `<socket_dir>/<app-id>.vsock`.
* **Tier 1 / development:** the same broker socket, bind-mounted at
  `/run/iliada/broker.sock`.
* The broker binds one socket per app. The host MUST derive the app identity
  from the socket, and MUST NOT accept identity claims from the guest.
* App ids are at most 64 bytes, so these paths fit in `sun_path`.

## Framing

`u32` big-endian length, then that many bytes of postcard-encoded message.
Frames over 1 MiB (`MAX_FRAME_LEN`) MUST be rejected and the connection
closed. A clean EOF between frames ends the session.

## Session

```
guest: Hello { version: 1, client }
host:  Welcome { version: 1, app_id, session }   | Goodbye { reason }
guest: Request { id, body } ...
host:  Reply { id, body } ...                    (in request order)
host:  Event(...)                                 (any time after Welcome)
```

Requests are answered strictly in order. A prompt blocks only its own session.

## Requests

| Request | Reply |
|---|---|
| `Capability { capability, reason? }` | `Granted(Grant)` / `Denied { reason }` / `Error` |
| `Revoke { capability }` | `Ok`, then `Event::Revoked` |
| `ListGrants` | `Grants([...])` |
| `Read { handle, offset, len }` | `Data { bytes, eof }` (≤ 256 KiB) / `Error` |
| `Close { handle }` | `Ok` |

`Grant { handle, capability, scope: Once|WhileRunning|Always, resource }` where
`resource` is `None` (pure permission), `File { name, size }` (read through
`Read`), or `Media { kind, endpoint }` (`endpoint` is `None` in Phase 1).

`DenyReason`: `NotDeclared` (no prompt shown), `UserDenied`, `PolicyDenied`,
`Unavailable`.

## Capabilities

`camera`, `microphone`, `location(coarse|precise)`, `clipboard(read|write)`,
`files(user_picked | known_dir(documents|downloads|music|pictures|videos))`,
`notifications`, `screen_capture`. A precise-location declaration covers
coarse requests.

## Versioning

A `Hello` with an unknown version gets `Goodbye`. New request or reply
variants require a version bump, because postcard enums are positional:
append variants, never reorder them.
