# Branding: "Iliada" in /etc/os-release, the boot menu and the greeter, and
# its logo, a Greek helmet (περικεφαλαία) with a horsehair crest: the
# `iliada` icon, and the picture `fastfetch` / `neofetch` show.
# The distribution ID stays "nixos" so NixOS tooling keeps working.
{ pkgs, ... }:
let
  branding = pkgs.runCommand "iliada-branding" { } ''
    install -Dm644 ${../branding/iliada.svg} $out/share/icons/hicolor/scalable/apps/iliada.svg
    install -Dm644 ${../branding/iliada.svg} $out/share/pixmaps/iliada.svg
    install -Dm644 ${../branding/logo.txt} $out/share/iliada/logo.txt
    substitute ${../branding/fastfetch.jsonc} $out/share/iliada/fastfetch.jsonc \
      --replace-fail @logo@ $out/share/iliada/logo.txt
  '';
  # neofetch is no longer maintained; fastfetch is its successor.
  neofetch = pkgs.writeShellScriptBin "neofetch" ''exec ${pkgs.fastfetch}/bin/fastfetch "$@"'';
in
{
  system.nixos.distroName = "Iliada";
  system.nixos.extraOSReleaseArgs.LOGO = "iliada";
  environment.systemPackages = [ branding pkgs.fastfetch neofetch ];
  environment.etc."xdg/fastfetch/config.jsonc".source = "${branding}/share/iliada/fastfetch.jsonc";
}
