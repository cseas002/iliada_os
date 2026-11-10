# settings.toml: plain-TOML system settings, edited by the `iliada` command
# (and later the Settings app) instead of Nix code.
#
#   [tools]                     command-line tools (nixpkgs attribute names)
#   packages = ["vim", "git"]
#   [services]
#   ssh = false                 SSH server
#   printing = false            CUPS
#   nginx, postgresql, mariadb, redis, docker   (the Store's Services)
{ config, lib, pkgs, ... }:
let
  cfg = config.iliada;
  s = if cfg.settingsFile == null then { } else builtins.fromTOML (builtins.readFile cfg.settingsFile);
  services = s.services or { };
  toPkg = name:
    lib.attrByPath (lib.splitString "." name) (throw "iliada settings: there is no package called ${name}") pkgs;
in
{
  options.iliada = {
    settingsFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      example = lib.literalExpression "./settings.toml";
      description = "Plain-TOML system settings (tools, services).";
    };
    tools = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "vim" "git" ];
      description = "Command-line tools installed system-wide (nixpkgs attribute names), in addition to settings.toml.";
    };
  };

  config = {
    environment.systemPackages = map toPkg (cfg.tools ++ (s.tools.packages or [ ]));
    services.openssh.enable = lib.mkIf (services.ssh or false) true;
    services.printing.enable = lib.mkIf (services.printing or false) true;
    # Background services from the Store. They listen on this computer; the
    # firewall stays closed to them unless configured otherwise.
    services.nginx = lib.mkIf (services.nginx or false) {
      enable = true;
      virtualHosts.localhost = {
        default = true;
        root = "/srv/www";
      };
    };
    systemd.tmpfiles.rules = lib.mkIf (services.nginx or false) [ "d /srv/www 0755 root wheel -" ];
    services.postgresql.enable = lib.mkIf (services.postgresql or false) true;
    services.mysql = lib.mkIf (services.mariadb or false) {
      enable = true;
      package = pkgs.mariadb;
    };
    services.redis.servers."".enable = lib.mkIf (services.redis or false) true;
    virtualisation.docker.enable = lib.mkIf (services.docker or false) true;
  };
}
