# The ready-to-run VM image, for developing Iliada against: the developer's
# Mac reaches it as `iliada.local` and pushes new Iliada sources into it
# (scripts/dev-push.sh), which rebuild in place. The tester's setup, apps
# and settings survive every update: no reinstalling.
#
# Not for distribution: it trusts the keys in dev-keys/.
{
  imports = [ ./utm.nix ../profiles/dev-access.nix ];
  # The configuration written on first boot keeps this access.
  iliada.system.firstBoot.devAccess = true;
}
