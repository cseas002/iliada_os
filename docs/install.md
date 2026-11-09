# Installing and configuring Iliada

Iliada 0.1 is a **developer preview**. It installs like NixOS: you boot an
installer, and the whole system is then described by one configuration
folder, `/etc/iliada`.

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
3. **Boot it** (UEFI). You land in a COSMIC live session.
4. **Install** from a terminal:
   ```sh
   lsblk                                     # find your disk
   sudo iliada-install --disk /dev/nvme0n1 --user alex --fullname "Alex" \
                       --hostname alex-laptop --timezone Europe/Athens
   ```
   It asks you to confirm by typing the disk name, then asks for a password.
   The **whole disk** is erased (GPT: 1 GiB EFI partition + ext4 root).
5. Reboot and remove the USB stick.

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
/etc/iliada/
  flake.nix                   which Iliada version to build from (don't usually edit)
  configuration.nix           this computer: users, host name, desktop, isolation…
  apps.toml                   your installed apps (maintained automatically)
  hardware-configuration.nix  disks and drivers (generated)
  source/                     the Iliada release this machine was installed from
```

Change anything, then apply it:

```sh
sudo nixos-rebuild switch --flake /etc/iliada#iliada
```

Every change is atomic. If a change breaks something, reboot and pick the
previous entry in the boot menu.

`/etc/iliada` is a git repository: every change has history, and it is what
the (upcoming) backup and sync feature pushes to your private GitHub repo.

### Apps (no rebuild, no terminal)

* **Install:** drag an `.app` into Applications. It appears in the app
  launcher within seconds. Apps are only ever `.app` bundles; there are no
  other package formats to deal with.
* **Uninstall:** drag the app to the Trash.
* Both are recorded in `/etc/iliada/apps.toml` automatically. On a machine
  restored from that configuration, the listed apps install themselves.
* You can also add an entry by hand (`debian = "gnome-text-editor"`). It is
  picked up within seconds. Apps never need `nixos-rebuild`.

Every app runs in its own sandbox, with no network and no access to your
files unless you grant it.

### Useful options

| Option | Default | Meaning |
|---|---|---|
| `iliada.desktop.enable` | `true` in the template | COSMIC desktop |
| `iliada.desktop.autoLoginUser` | `null` | log a user in automatically |
| `iliada.workstation.enable` | follows `desktop.enable` | networking, audio, bluetooth, firmware |
| `iliada.applications.list` | `/etc/iliada/apps.toml` | the app list (mutable, maintained automatically) |
| `iliada.isolation.defaultTier` | `"micro-vm"` | `namespace` or `micro-vm` |
| `iliada.isolation.defaultNetwork` | `"none"` | network for app sandboxes: `none` / `nat` |
| `iliada.rustUserland.enable` | `true` | uutils, sudo-rs, ntpd-rs, fish |
| `iliada.ux.hideFhs` | `true` | hide Unix folders in the file manager |
| `iliada.ux.noSudo` | `false` | remove sudo entirely |
| `iliada.image.enable` | `false` | build the immutable appliance image instead |

Any NixOS option works too (`services.printing.enable = true;` etc.).

## Updating

In 0.1, the machine builds from the Iliada copy in `/etc/iliada/source`.
To update, replace that folder with a newer Iliada checkout, then:

```sh
cd /etc/iliada && sudo nix flake update iliada && sudo nixos-rebuild switch --flake .#iliada
```

(Once Iliada publishes releases, `flake.nix` will point at them and
`nix flake update` alone will do.)
