# The Iliada account on each computer (docs/account.md): which account server
# people sign in to (Settings → Account, `iliada account`), and an hourly
# backup of each signed-in person's settings, encrypted on the computer
# before it is sent. People who haven't signed in are left alone.
{ config, lib, ... }:
let
  cfg = config.iliada.account;
  pkg = config.iliada.package;
in
{
  options.iliada.account.server = lib.mkOption {
    type = lib.types.nullOr lib.types.str;
    # Iliada's own server (online since 2026-11-19).
    default = "https://account.iliada-os.com";
    example = "https://account.example.org";
    description = ''
      The Iliada account server (github.com/iliada-os/cloud): Iliada's own
      by default, or one you run yourself. null leaves the account out of
      Settings; `iliada account sign-in --server …` still works.
    '';
  };

  config = lib.mkIf (cfg.server != null) {
    environment.sessionVariables.ILIADA_ACCOUNT_SERVER = cfg.server;
    systemd.user.services.iliada-account-backup = {
      description = "Back up your settings to your Iliada account";
      environment.ILIADA_ACCOUNT_SERVER = cfg.server;
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${pkg}/bin/iliada account backup --if-signed-in";
        Nice = 10;
      };
    };
    systemd.user.timers.iliada-account-backup = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnStartupSec = "10min";
        OnUnitActiveSec = "1h";
        RandomizedDelaySec = "5min";
      };
    };
  };
}
