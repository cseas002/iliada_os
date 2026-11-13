# settings.toml: plain-TOML system settings, edited by the `iliada` command
# (and later the Settings app) instead of Nix code.
#
#   [tools]                     command-line tools (nixpkgs attribute names)
#   packages = ["vim", "git"]
#   [services]
#   ssh = true                  SSH server: installed and started at boot
#                               ("manual": installed, started only by hand; false: not installed)
#   printing = false            CUPS
#   nginx, postgresql, mariadb, redis, docker   (the Store's Services)
{ config, lib, pkgs, ... }:
let
  cfg = config.iliada;
  s = if cfg.settingsFile == null then { } else builtins.fromTOML (builtins.readFile cfg.settingsFile);
  services = s.services or { };
  # Three levels, as on Debian or Fedora: on (true or "on"), "manual", off.
  state = name: let v = services.${name} or false; in
    if v == true || v == "on" then "on" else if v == "manual" then "manual" else "off";
  installed = name: state name != "off";
  manual = name: state name == "manual";
  # The units (and sockets) that would start a service at boot.
  bootUnits = {
    ssh = { services = [ "sshd" ]; sockets = [ ]; };
    printing = { services = [ "cups" ]; sockets = [ "cups" ]; };
    nginx = { services = [ "nginx" ]; sockets = [ ]; };
    postgresql = { services = [ "postgresql" ]; sockets = [ ]; };
    mariadb = { services = [ "mysql" ]; sockets = [ ]; };
    redis = { services = [ "redis" ]; sockets = [ ]; };
    # Its socket stays: docker starts the moment it is used.
    docker = { services = [ "docker" ]; sockets = [ ]; };
  };
  manualServices = lib.filter manual (lib.attrNames bootUnits);
  noBoot = kind: lib.listToAttrs (lib.concatMap (n: map (u: lib.nameValuePair u { wantedBy = lib.mkForce [ ]; }) bootUnits.${n}.${kind}) manualServices);
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
    services.openssh.enable = lib.mkIf (installed "ssh") true;
    services.printing.enable = lib.mkIf (installed "printing") true;
    # Background services from the Store. They listen on this computer; the
    # firewall stays closed to them unless configured otherwise.
    services.nginx = lib.mkIf (installed "nginx") {
      enable = true;
      virtualHosts.localhost = {
        default = true;
        root = "/srv/www";
      };
    };
    systemd.tmpfiles.rules = lib.mkIf (installed "nginx") [ "d /srv/www 0755 root wheel -" ];
    services.postgresql.enable = lib.mkIf (installed "postgresql") true;
    services.mysql = lib.mkIf (installed "mariadb") {
      enable = true;
      package = pkgs.mariadb;
    };
    services.redis.servers."".enable = lib.mkIf (installed "redis") true;
    virtualisation.docker.enable = lib.mkIf (installed "docker") true;
    # "manual": installed, but nothing starts it at boot (systemctl start does).
    systemd.services = noBoot "services";
    systemd.sockets = noBoot "sockets";
  };
}
