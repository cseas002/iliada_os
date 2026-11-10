# Installing and configuring Iliada

Iliada is a **developer preview**. Try it in a virtual machine (Option 0),
or install it from a USB stick. Either way the whole system is described by
one configuration folder, `/etc/iliada`, and changed from Iliada Settings.

## Option 0: a ready-to-run virtual machine (no installation)

A disk image that boots straight to the desktop: `iliada.qcow2`.

```sh
nix build github:cseas002/iliada_os#packages.aarch64-linux.utm-image   # Apple Silicon (UTM)
nix build github:cseas002/iliada_os#packages.x86_64-linux.utm-image    # x86 (UTM, QEMU, Proxmox)
```

**UTM on a Mac:**
1. **+** → **Virtualize** → **Linux**. Leave the boot ISO empty → Continue.
2. Memory **4096 MB**, CPU cores **2–4** → Continue. Storage: any size (it is
   replaced next) → Continue → Continue.
3. Summary: name it "Iliada", tick **Open VM Settings** → Save.
4. In the settings, **Drives**: delete the drive UTM made, then **New…** →
   **Import…** → choose `iliada.qcow2` (interface VirtIO) → Save.
5. Start it. You are logged in as `iliada` (password `iliada`).

On first boot Iliada writes `/etc/iliada` itself, so Settings can change and
update the VM like an installed system. The disk grows to 40 GB on its own.

### For developers: test changes without reinstalling

Use the `utm-dev-image` instead: the same VM, reachable from the Mac that
built it as `iliada.local` (its key is in `nix/hosts/dev-keys/`). Set it up
once; after that every change is pushed into the running VM, which rebuilds
in place and keeps your apps, settings and files:

```sh
nix build .#packages.aarch64-linux.utm-dev-image      # once
scripts/dev-push.sh                                    # after every change (default host iliada.local)
```

## Option A: fresh install (installer ISO)

Requirements: x86_64 PC or VM, UEFI firmware, 4 GB+ RAM, 40 GB+ disk,
internet during installation.

1. **Get the ISO.** Build it on any Linux machine with Nix:
   ```sh
   nix build github:cseas002/iliada_os#installer-iso     # or `.#installer-iso` in a checkout
   ls result/iso/                                        # iliada-…-x86_64-linux.iso
   ```
2. **Write it to a USB stick** (replace `/dev/sdX`, which is erased):
   ```sh
   sudo dd if=result/iso/*.iso of=/dev/sdX bs=4M status=progress conv=fsync
   ```
   Or use balenaEtcher. For a VM, attach the ISO as a CD.
3. **Boot it** (UEFI). You land in a COSMIC live session, and **Install
   Iliada** opens: choose the disk, enter your name, account name, computer
   name, password and time zone, confirm, and wait. The **whole disk** is
   erased (GPT: 1 GiB EFI partition + ext4 root).
4. Restart and remove the USB stick. On first login the **Setup Assistant**
   offers to restore a backup from GitHub or to start backing this computer up.

The same installation from a terminal (what the installer runs):
```sh
lsblk                                     # find your disk
sudo iliada-install --disk /dev/nvme0n1 --user alex --fullname "Alex" \
                    --hostname alex-laptop --timezone Europe/Athens
```

## Option B: an existing NixOS machine

```sh
sudo mkdir -p /etc/iliada && cd /etc/iliada
sudo nix flake init -t github:cseas002/iliada_os
sudo nixos-generate-config --show-hardware-config | sudo tee hardware-configuration.nix
sudoedit configuration.nix          # set your user, host name, time zone
sudo nixos-rebuild switch --flake /etc/iliada#iliada
```

## Your configuration

```
/etc/iliada/                  a git repository: every change is recorded
  apps.toml                   your apps (.app bundles), maintained automatically
  settings.toml               command-line tools and services (plain TOML)
  configuration.nix           everything else: users, host name, any NixOS option
  flake.nix                   which Iliada version to build from (don't usually edit)
  hardware-configuration.nix  disks and drivers (generated)
  source/                     the Iliada release this machine was installed from
```

### The `iliada` command

| | |
|---|---|
| `iliada list` | apps in the Store |
| `iliada install firefox` · `iliada remove firefox` | install / remove an app (no password, no rebuild) |
| `iliada update` | update apps, and install any on your list that are missing |
| `iliada tool add htop` · `iliada tool remove htop` · `iliada tool list` | command-line tools |
| `iliada service enable ssh` · `disable` · `list` | services (SSH server, printing) |
| `iliada system status` · `iliada system update` | Iliada version and updates |
| `iliada system apply` | apply `/etc/iliada` after editing it by hand |
| `iliada backup sign-in` · `enable <name>` · `now` · `list` · `restore <owner/iliada_name>` | GitHub backup |

Everything here is also in **Iliada Settings** (Updates, Backup,
Command-line tools, Services, Privacy, About). Neither needs `sudo`: the
Iliada system service does the work for administrators (members of
`wheel`). Changes apply the system atomically and are committed to
`/etc/iliada`'s history; if applying fails, the file is put back and the
running system is untouched.

Apps update themselves: `iliada-store-sync` runs two minutes after boot and
every six hours (`iliada.store.updateInterval`).

Edit `configuration.nix` by hand if you like, then apply it with
`iliada system apply`. Every change is atomic. If a change breaks something,
reboot and pick the previous entry in the boot menu.

### Backup and restore

Settings → Backup → **Sign in with GitHub** (you approve a code in the
browser), name the backup, done: `/etc/iliada` (your apps, tools, services
and configuration) is pushed to the private repository `iliada_<name>`
after every change and every hour. On a new computer the Setup Assistant (or
Settings → Backup → Restore) brings back its apps and settings; the new
computer keeps its own hardware configuration. Backups need a GitHub OAuth
app (`iliada.system.githubClientId`).

### Software and services

Apps are `.app`s. Command-line software (Python, Node.js, ffmpeg…) and
background services (nginx, PostgreSQL, MariaDB, Redis, Docker, SSH,
printing) are not: they are part of the system. Find them in the Store
under **Software** and **Services** (labeled as such), or use
`iliada tool add python3` / `iliada service enable nginx`. Any nixpkgs
package works with `iliada tool add`. They are recorded in `settings.toml`
and backed up.

### The assistant

Settings → Assistant: turn it on, choose **Ask me first** or **Act on its
own**, pick the model (Claude Opus 5.5 by default) and add your Anthropic
API key. Then ask in the **Assistant** app, or `iliada ask "…"`.

### Apps (no rebuild, no terminal)

* **Install:** open the **Store** (in the dock) and click **Get**, or drag an
  `.app` into Applications. It appears in the launcher within seconds. Apps
  are only ever `.app` bundles; there are no other package formats.
* **Uninstall:** drag the app to the Trash.
* Both are recorded in `/etc/iliada/apps.toml` automatically. On a machine
  restored from that configuration, the listed apps install themselves.
* You can also add an entry by hand (`debian = "gnome-text-editor"`). It is
  picked up within seconds. Apps never need `nixos-rebuild`.

Every app runs in its own sandbox and sees only its own files. The first
time an app starts it asks for what it declared (a folder such as Pictures,
the camera, the microphone); change the answers in Settings → Privacy. A
file you open with an app (double-click) is given to it, and only that
file. Settings → Privacy → **Extra isolation** runs an app in its own
virtual machine.

### Useful options

| Option | Default | Meaning |
|---|---|---|
| `iliada.desktop.enable` | `true` in the template | COSMIC desktop |
| `iliada.desktop.autoLoginUser` | `null` | log a user in automatically |
| `iliada.workstation.enable` | follows `desktop.enable` | networking, audio, bluetooth, firmware |
| `iliada.applications.list` | `/etc/iliada/apps.toml` | the app list (mutable, maintained automatically) |
| `iliada.settingsFile` | `./settings.toml` | tools and services (see above) |
| `iliada.store.catalog` | Iliada's catalog | where the Store gets apps (signed) |
| `iliada.store.updateInterval` | `"6h"` | how often apps are updated |
| `iliada.isolation.defaultTier` | `"namespace"` | `namespace` (native speed) or `micro-vm` |
| `iliada.isolation.highSecurity` | `false` | every app in its own micro-VM (slower, strongest) |
| `iliada.isolation.defaultNetwork` | `"none"` | network for app sandboxes: `none` / `nat` |
| `iliada.rustUserland.enable` | `true` | uutils, sudo-rs, ntpd-rs, fish |
| `iliada.ux.hideFhs` | `true` | hide Unix folders in the file manager |
| `iliada.ux.noSudo` | `false` | remove sudo entirely |
| `iliada.system.githubClientId` | `null` | GitHub OAuth app for backups (device flow) |
| `iliada.desktop.setupAssistant` | `true` | Setup Assistant on each user's first login |
| `iliada.image.enable` | `false` | build the immutable appliance image instead |

Any NixOS option works too (`services.printing.enable = true;` etc.).

## Updating

Settings → Updates → **Check now** / **Update now** (or
`iliada system update`). Iliada checks daily. An update downloads the
signed release, swaps `/etc/iliada/source`, rebuilds, and is a new boot-menu
entry; if the build fails, nothing changes. Apps update on their own.
