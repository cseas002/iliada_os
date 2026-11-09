# ADR 0001: NixOS as build engine, image-based delivery

**Status:** accepted (Phase 1)

## Context
The draft combined "NixOS immutable store", "A/B partitions" and "Nix
rollbacks". Options: (a) plain NixOS with a custom shell; (b) a from-scratch
distribution; (c) NixOS modules compiled into a verified image.

## Decision
(c). Nix composes the system; the output is an erofs store with dm-verity, a
signed UKI carrying the root hash, a tmpfs root, and A/B store slots updated
by systemd-sysupdate with boot counting.

## Consequences
* Users never see Nix and cannot mutate the system; rollback means booting the
  previous UKI.
* Apps cannot be part of the system closure; hence `/Applications` as a
  separate volume, and hence no admin prompt for installs.
* We inherit NixOS hardware support and reproducibility without inheriting
  its mutable administration model.
* Option (a) was rejected because nothing verifies the running store. Option
  (b) would cost years on drivers and packaging before reaching Iliada's
  actual differentiators.
