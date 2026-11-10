# Developer access to a test VM: SSH with the keys in nix/hosts/dev-keys/
# (root and every user), and the name `iliada.local` on the local network.
# Used by the utm-dev image; scripts/dev-push.sh updates such a VM in place.
{ lib, ... }:
let
  keys = map (f: builtins.readFile (../hosts/dev-keys + "/${f}"))
    (lib.attrNames (lib.filterAttrs (n: t: t == "regular" && lib.hasSuffix ".pub" n) (builtins.readDir ../hosts/dev-keys)));
in
{
  services.openssh = {
    enable = true;
    settings.PasswordAuthentication = false;
    settings.PermitRootLogin = "prohibit-password";
  };
  users.users.root.openssh.authorizedKeys.keys = keys;
  services.avahi = {
    enable = true;
    nssmdns4 = true;
    publish = {
      enable = true;
      addresses = true;
    };
  };
}
