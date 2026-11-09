# ┌─────────────────────────────────────────────────────────────────────┐
# │ Iliada system configuration                                         │
# │                                                                     │
# │ Everything about this computer is declared here. Apply changes with │
# │   sudo nixos-rebuild switch --flake /etc/iliada#iliada              │
# │ Every switch is atomic; the boot menu keeps previous versions, so   │
# │ a bad change is undone by rebooting into the one before it.         │
# └─────────────────────────────────────────────────────────────────────┘
{ ... }:
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
    # Command-line tools and services (SSH server, printing) live in
    # settings.toml; change them with `iliada tool add vim`,
    # `iliada service enable ssh`, or by editing the file and `iliada apply`.
    settingsFile = ./settings.toml;

    # The COSMIC desktop, plus networking, sound, bluetooth and firmware.
    desktop.enable = true;
    # desktop.autoLoginUser = "iliada";

    # Apps are .app bundles in /Applications, each in its own sandbox.
    # Install one by dragging it into Applications; uninstall by dragging it
    # to the Trash. No rebuild is ever needed for apps. The list
    # of installed apps is kept automatically in /etc/iliada/apps.toml.
    applications.ingest.enable = true;

    # Every app runs in its own sandbox (like macOS's App Sandbox), at native
    # speed. High security mode gives every app its own micro-VM instead:
    # stronger isolation, slower apps. Single apps can get "Extra isolation"
    # in Settings. Network access for apps is off by default.
    isolation.defaultTier = "namespace";
    isolation.highSecurity = false;
    isolation.defaultNetwork = "none";

    # Rust userland: uutils coreutils, sudo-rs, ntpd-rs, fish.
    rustUserland.enable = true;

    # Hide Unix system folders from the file manager.
    ux.hideFhs = true;
  };


  # The NixOS release this machine was first installed with. Don't change it.
  system.stateVersion = "26.11";
}
