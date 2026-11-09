# ADR 0004: Ingestion bundles libraries instead of static relinking

**Status:** accepted (Phase 1)

## Context
The draft asked the AI daemon to "statically link missing libraries". Static
linking happens at link time; a distributed, dynamically linked ELF cannot be
converted afterwards.

## Decision
* Bundle the missing shared objects under `Contents/Libraries` and make the
  loader find them with RUNPATH (`patchelf`) or a launcher that sets
  `LD_LIBRARY_PATH`.
* Define a small **runtime base** (`iliada-base-1`) of sonames the platform
  guarantees, and bundle everything else.
* AI is an **advisor**, never an authority: its output is verified against
  the package index hash and the library's DT_SONAME before use, and recorded
  in `Provenance.toml`.
* Never execute maintainer scripts; extract with pure-Rust parsers under
  Landlock.

## Consequences
* Bundles are larger than distro packages (gnome-calculator pulls ~95
  libraries while GTK is outside the runtime base). Adding GTK/Qt runtimes to
  the base is the planned mitigation.
* Packages that rely on maintainer scripts (users, services, alternatives) are
  out of scope for ingestion. They need recipes or first-party bundles.
