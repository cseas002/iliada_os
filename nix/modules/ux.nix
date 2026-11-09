# User-facing system behaviour.
{ config, lib, ... }:
let
  cfg = config.iliada.ux;
  hidden = [ "bin" "boot" "dev" "etc" "lib" "lib64" "mnt" "nix" "opt" "proc" "root" "run" "sbin" "srv" "sys" "tmp" "usr" "var" ];
in
{
  options.iliada.ux = {
    hideFhs = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Hide Unix system directories from file managers (GTK/Qt honour `/.hidden`).
        They remain reachable for developers; they are just not part of the user's world.
      '';
    };
    noSudo = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Remove sudo entirely. Everyday administration (installing apps, updates,
        power) needs no elevation on Iliada: apps live outside the image, updates
        are atomic image swaps, and the polkit rules below allow the remaining
        actions for the active local user. Leave false on development systems.
      '';
    };
  };

  config = lib.mkMerge [
    (lib.mkIf cfg.hideFhs {
      systemd.tmpfiles.rules = [ "f+ /.hidden 0644 root root - ${lib.concatStringsSep "\\n" hidden}" ];
    })
    (lib.mkIf cfg.noSudo { security.sudo.enable = false; })
    {
      security.polkit.enable = true;
      security.polkit.extraConfig = ''
        // Iliada: no password prompts for actions that cannot harm system integrity.
        polkit.addRule(function (action, subject) {
          if (!subject.local || !subject.active) return polkit.Result.NOT_HANDLED;
          if (action.id == "org.freedesktop.login1.reboot" ||
              action.id == "org.freedesktop.login1.power-off" ||
              action.id == "org.freedesktop.login1.suspend" ||
              action.id.indexOf("org.freedesktop.sysupdate1.") == 0) {
            return polkit.Result.YES;
          }
          return polkit.Result.NOT_HANDLED;
        });
      '';
    }
  ];
}
