# The Iliada account (design)

Status: **being built.** The server (sign-in, passkeys, the website, encrypted
storage) and the computer side (Settings → Account, the Setup Assistant,
`iliada account`, hourly encrypted backups) work in the dev VM against a
server on the build machine; Iliada's own server has no home yet. It grows the self-hosted server proposal
([self-hosted-sync.md](self-hosted-sync.md)) into an account people sign up
for, the way a Mac has an Apple Account. Iliada runs the default server;
anyone can run their own, and a computer can point at either.

## What a person gets

* **Sign up and sign in** with a passkey (the phone or computer unlocks it
  with a fingerprint, face or PIN), with Google or Apple, or with email and
  a code. No password to remember.
* **Their settings follow them**: desktop (appearance, dock, wallpaper,
  shortcuts), Privacy answers, the assistant's conversations and settings,
  their app list, and each app's data if they choose. A new computer signs
  in and is theirs again.
* **Passwords**: the account comes with a Bitwarden-compatible password
  server (Vaultwarden), so the Bitwarden already in Firefox and Chromium can
  sync passwords and passkeys without a Bitwarden account.
* **Later**: keeping two computers in step; Documents and other folders
  ("cloud"); family sharing.

## How a computer signs in

Passkeys, Google and Apple all happen in a web browser: Linux has no
system-wide passkey support for ordinary apps yet. So the account server
has a small website (sign up, sign in, your computers), and a computer signs
in the way GitHub backups already do: Settings shows a short code and opens
the account page; the person signs in there (passkey, Google, Apple or an
email code) and approves the code. The computer then gets its own token,
which the person can remove from the website at any time (OAuth 2.0 device
authorization, RFC 8628). Phones and other computers use the same page.

## The one rule: the server can't read it

Everything a person keeps is encrypted on their computer before it is sent.
The server holds data it cannot read, so a break-in or a mistake on the
server leaks nothing, and running it carries much less legal weight (GDPR:
the server stores the account's email and encrypted blobs, nothing else
about the person).

* Each account has a random **account key**, made on the first computer.
  The server never sees it.
* It reaches a new computer in one of two ways: **approve it from a computer
  already signed in** (a code shown on both, as with GitHub backups now), or
  the **recovery key** (shown once at sign-up, to write down or print).
* Signing in (passkey, Google, Apple, email) proves who you are; it does not
  unlock your data. That is why Google or Apple can never read it either.
* Losing every computer *and* the recovery key loses the data. The Setup
  Assistant says so plainly and asks the person to keep the key.
* Later: passkeys that can also unlock (the WebAuthn PRF extension), so on
  most devices the passkey alone is enough.

## Pieces

| Piece | What | Built from |
|---|---|---|
| **Account server** (`iliada-account`) | sign-up/sign-in (passkeys, Google, Apple, email codes), sessions, list and remove your computers, approve a new computer, quotas, delete the account | Rust: `axum`, `webauthn-rs` (passkeys), `openidconnect` (Google, Apple), SQLite first (one file; PostgreSQL when it needs to grow) |
| **Encrypted storage** | each person's snapshots | restic's format through `rustic_core`, stored on the server's disk first, S3-compatible storage (Garage) later |
| **Password server** | Bitwarden sync | Vaultwarden (Rust), run next to it |
| **On the computer** | Settings → Account, the Setup Assistant's "Sign in", `iliada account`; a per-person service that backs up while they are logged in | the existing system service and Settings |
| **Deployment** | one NixOS module (`services.iliada-account`) with TLS, backups of the server itself | Nix, like the rest |

## Building it

1. **Server, first version** (done) (on this build server, reachable from the dev
   VM): sign-up and sign-in with a passkey and with an email code, on its
   website; a computer is approved with a code and gets a token; encrypted
   blobs stored per account with a size limit. Tests for each step (passkeys
   with a software authenticator), and an end-to-end test from the dev VM.
2. **On the computer** (done in the dev VM, 2026-11-16): Settings → Account and the Setup Assistant step;
   backing up desktop settings, Privacy answers and the assistant's
   conversations; restore on a fresh install (verified in the installed VM). Built as the
   `iliada-account` crate: a code approved on the website gives the computer
   its own token; the account key is made on the first computer and shown
   once as a 32-character recovery key; `~/.config/cosmic`, `permissions.toml`
   and `conversations/` are packed (tar + zstd), sealed with
   XChaCha20-Poly1305 (the name of what is kept is authenticated too) and
   uploaded as `settings`; a user timer backs up hourly when something
   changed. A new computer gets the key from one already signed in
   (2026-11-20): it leaves a one-time X25519 public key with the server
   (`/v1/keys`), the other computer seals the account key to it (X25519 +
   XChaCha20-Poly1305) after the person sees the same 8-character code on
   both screens (a hash of that public key), and the new computer checks
   the key against `key-check`. Left: the installed VM, app data.
3. **Vaultwarden** next to it, the Bitwarden extension pointed at it.
4. **Google and Apple sign-in.** Needs a Google Cloud project (free) and an
   Apple Developer account ($99 a year) in Iliada's name.
5. **Hosting**: a domain, a server, an email sender for sign-in codes, a
   privacy policy, account deletion and export.

## What the owner decides

* The domain and where the default server runs (this machine, or a rented
  one).
* Whether a hosted account is free, and its storage limit (settings are
  tiny; files are where cost grows).
* The server is public: [iliada-os/cloud](https://github.com/iliada-os/cloud)
  (Rust: axum, webauthn-rs, SQLite; the website is plain HTML with one small
  script for passkeys).
