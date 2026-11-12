# Your own Iliada server (proposal)

Status: **proposal, nothing built.** Today a computer backs up its
configuration (`/etc/iliada`: apps, software, services, settings) to a
private GitHub repository. This page describes a second place to keep it:
a server you run yourself, which can also keep more than the system
configuration.

## What it would keep

Two owners, kept apart:

| | What | Who can restore it |
|---|---|---|
| **The computer** | `/etc/iliada`: apps, software, services, configuration, its history | an administrator |
| **Each person** | their desktop settings (COSMIC: appearance, dock, shortcuts, wallpaper), Privacy answers, assistant conversations and settings, each app's own data (every `.app` keeps its data in one folder, so it can be saved app by app), and, if they choose, folders such as Documents | only that person |

API keys and email passwords could be included too, but only because of the
encryption below. Without it they would stay on the computer, as they do
today.

## How it would work

* **Encrypted before it leaves the computer.** The server stores data it
  can't read. Each person (and the computer) has a key; a recovery phrase,
  shown once at setup, brings it back on a new computer. Losing both the
  computer and the phrase means losing the backup; that is the price of a
  server that can't read your data, and the Setup Assistant must say so plainly.
* **Snapshots, not a single copy.** Every backup is a snapshot; unchanged
  data isn't stored twice, so hourly snapshots stay cheap, and any older one
  can be restored ("my settings from last Tuesday").
* **Signing in on a new computer.** In the Setup Assistant, choose "Restore
  from my Iliada server", type its address, approve the new computer from
  one you already use (or enter the recovery phrase), pick what to bring
  back. It is the GitHub flow with a different place.
* **Backup first, sync later.** First version: back up and restore, the same
  as with GitHub. Keeping two computers in step (change a setting on one, it
  changes on the other) comes after, because two computers changing the same
  thing at the same time needs rules for who wins.

## Building it (my recommendation)

1. **Don't write a storage format: reuse restic's.** The Rust implementation
   (`rustic_core`) gives encrypted, deduplicated snapshots and talks to many
   kinds of storage: restic's small `rest-server`, S3-compatible storage
   (MinIO, Garage), SFTP. Self-hosting is then "run `rest-server` on your
   box", a single program, or point it at storage you already have.
2. **A small Iliada server only for what storage can't do:** accounts,
   approving a new computer with a code (as GitHub sign-in works now),
   listing a person's computers, quotas. Written in Rust like the rest, with
   a NixOS module (`services.iliada-server.enable = true`) and a container
   image for people who don't run NixOS.
3. **On the computer, one backup setting with several places:** GitHub (the
   configuration as a git repository, as now), your Iliada server, or later a
   hosted one. The system service backs up the computer; a per-person service
   backs up each person's data while they are logged in.
4. **Keep git for the configuration.** `/etc/iliada` stays a git repository,
   so its history (`iliada history`) works the same wherever it is backed up;
   the snapshot is what is encrypted and sent.

## Questions to decide

* One server per family/company (accounts on it), or one per person?
* Should the server also offer the Iliada app catalog and updates (a mirror,
  for places with slow or no internet)?
* Should a hosted version exist for people who won't run a server, and who
  runs it?
