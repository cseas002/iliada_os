# ┌─────────────────────────────────────────────────────────────────────┐
# │ Iliada system configuration                                         │
# │                                                                     │
# │ Everything about this computer is declared here. Apply changes with │
# │   sudo nixos-rebuild switch --flake /etc/iliada#iliada              │
# │ Every switch is atomic; the boot menu keeps previous versions, so   │
# │ a bad change is undone by rebooting into the one before it.         │
# └─────────────────────────────────────────────────────────────────────┘
{ pkgs, ... }:
{
  # ── This computer ───────────────────────────────────────────────────
  networking.hostName = "iliada";
  time.timeZone = "UTC"; # e.g. "Europe/Athens"
  i18n.defaultLocale = "en_US.UTF-8";
  console.keyMap = "us";

  # ── People ──────────────────────────────────────────────────────────
  users.users.iliada = {
    isNormalUser = true;
    description = "Iliada User";
    # wheel: may administer the system · iliada-apps: may install apps
    extraGroups = [ "wheel" "iliada-apps" "networkmanager" ];
    initialPassword = "iliada"; # change it after first login with `passwd`
  };

  # ── Iliada ──────────────────────────────────────────────────────────
  iliada = {
    # The COSMIC desktop, plus networking, sound, bluetooth and firmware.
    desktop.enable = true;
    # desktop.autoLoginUser = "iliada";

    # Apps: listed in apps.toml, translated into /Applications automatically
    # and kept up to date. Each runs in its own sandbox.
    applications.ingest = {
      enable = true;
      watchlist = ./apps.toml;
    };

    # Untrusted apps get a micro-VM (falls back to a namespace sandbox until
    # micro-VM support lands). Network access for apps is off by default.
    isolation.defaultTier = "micro-vm";
    isolation.defaultNetwork = "none";

    # Rust userland: uutils coreutils, sudo-rs, ntpd-rs, fish.
    rustUserland.enable = true;

    # Hide Unix system folders from the file manager.
    ux.hideFhs = true;
  };

  # Extra command-line tools for this machine (most apps belong in apps.toml).
  environment.systemPackages = with pkgs; [ git ];

  # The NixOS release this machine was first installed with. Don't change it.
  system.stateVersion = "26.11";
}
