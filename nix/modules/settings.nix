# settings.toml: plain-TOML system settings, edited by the `iliada` command
# (and later the Settings app) instead of Nix code.
#
#   [tools]                     command-line tools (nixpkgs attribute names)
#   packages = ["vim", "git"]
#   [services]
#   ssh = false                 SSH server
#   printing = false            CUPS
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
  };
}
