# ADR 0005: No terminal for users; configuration that follows you

**Status:** accepted (design), implementation phased (see Roadmap)

## Requirement
A user never needs a terminal: installing and removing apps, changing
settings, backing up and moving to a new computer all happen in the GUI,
like macOS. The configuration is backed up to the user's own storage (a
private GitHub repository now, an Iliada account later) and restoring it on
a new machine brings back apps, preferences and settings.

## Decision

### 1. Two layers, and only one of them ever rebuilds
| Layer | What | Where | How it changes |
|---|---|---|---|
| **System** | users, host name, time zone, features, hardware | `/etc/iliada` (`settings.toml`, optional `configuration.nix`) | GUI Settings writes `settings.toml`; `iliada-configd` applies it with an atomic rebuild in the background |
| **User** | apps, desktop preferences, (opt-in) app data | `/Applications`, `/etc/iliada/apps.toml`, `~/.config` | immediately, no rebuild, ever |

Apps never cause a rebuild. They live outside the system image by design
(ADR 0001), so installing one only means adding a bundle.

### 2. App lifecycle without a terminal
* **The only app format users see is `.app`.** No `.deb`, Flatpak or
  AppImage ever reaches the user. The Package Translator turns those into
  `.app` bundles behind the scenes (on a build service, or locally for app-list
  entries).
* **Install:** drag a `.app` into Applications. Web downloads arrive as a
  single compressed, signed `.app` (like macOS's zipped apps) that opens into a
  `.app`. Later, a "Get Apps" catalog.
* **Uninstall:** drag the app to the Trash.
* **Record:** `iliada-appd` keeps `/etc/iliada/apps.toml` equal to what is in
  `/Applications`, including where each app came from, so another machine can
  reinstall the same apps. Nobody edits this file by hand (but they may).

### 3. Settings without a terminal
`iliada-configd` is a small privileged daemon. It applies `/etc/iliada` with
`nixos-rebuild switch` in the background, shows progress in the shell, and
keeps the previous generation for rollback. It accepts requests only from
the active local session, checked by polkit. The Settings app writes typed
values (`settings.toml`) rather than Nix code.

### 4. Backup and sync (`iliada-syncd`)
* `/etc/iliada` is a git repository. With backup on, it is pushed to a
  **private** repository `iliada_<name>`, where `<name>` is chosen by the user
  ("personal-laptop").
* Repository layout:
  ```
  flake.nix, flake.lock      which Iliada release to build from
  settings.toml              system settings shared by every device on this profile
  apps.toml                  installed apps (maintained by iliada-appd)
  preferences/               desktop preferences (COSMIC config, wallpaper…)
  devices/<host>/            per-device: hardware-configuration.nix, overrides
  configuration.nix          optional power-user Nix
  ```
* **Never synced:** passwords, tokens, keys, Wi-Fi secrets, browser profiles.
  App data only if the user opts in.
* **Changes are signed by devices.** Every device has its own signing key,
  enrolled when it joins the profile. A device applies only commits signed by
  an enrolled device of that profile. Someone with the GitHub token but no
  enrolled device cannot change your machines. Applying remote system changes
  is root on every device, so this is mandatory, not optional.
* Offline first: changes are committed locally and pushed when online.
  Conflicts resolve per file; device-specific files never conflict.

### 5. First run
* **Installer** (graphical): pick a disk and a language. Done.
* **Setup Assistant** (first boot): keyboard, Wi-Fi, your account, then
  **"Back up and sync this computer?"** →
  *Connect GitHub* (OAuth device flow: a code shown on screen, approved in a
  browser) → **Restore** an existing `iliada_*` profile, or **Create new**
  (name it). Restoring enrols the device, pulls the profile and applies it in
  the background; apps reinstall themselves from `apps.toml`.
* Later: an Iliada account replaces GitHub as the backend. The format stays a
  git repository, so moving between backends is a push.

## Consequences
* `apps.toml` moves out of the Nix store: the Package Translator watches the
  mutable `/etc/iliada/apps.toml` and re-reads it continuously.
* GitHub authentication needs an OAuth App (or GitHub App) client id
  registered by the Iliada project; repository creation needs the `repo`
  scope, which is broad, so the token is kept in a root-only store and used
  only by `iliada-syncd`.
* `configuration.nix` remains for power users; GUI changes go to
  `settings.toml`, so the two never fight over the same file.
