# Iliada's desktop apps: the Store, Settings, the Setup Assistant and the
# installer. One
# cargo workspace (apps/), built separately from the core tools because it
# depends on libcosmic (git dependencies, GPU rendering stack).
{ lib, rustPlatform, libcosmicAppHook }:

rustPlatform.buildRustPackage {
  pname = "iliada-desktop-apps";
  version = lib.trim (builtins.readFile ../VERSION);

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../apps
    ];
  };
  cargoRoot = "apps";
  buildAndTestSubdir = "apps";
  cargoLock = {
    lockFile = ../apps/Cargo.lock;
    allowBuiltinFetchGit = true;
  };

  # libcosmic: a text field's right-click menu (Cut, Copy, Paste) opened off
  # by the page's scroll offset, often at the bottom of the screen. The patch
  # anchors it where the pointer is in the window.
  postPatch = ''
    lc=$(dirname "$(dirname "$(dirname "$(echo "$cargoDepsCopy"/*/src/widget/text_context_menu.rs)")")")
    patch -d "$lc" -p1 < ${./patches/libcosmic-context-menu-position.patch}
  '';

  nativeBuildInputs = [ libcosmicAppHook ];
  doCheck = false;

  postInstall = ''
    # Alt+Space: the assistant's quick panel.
    cat > $out/bin/iliada-ask <<EOS
    #!/bin/sh
    exec $out/bin/iliada-assistant-app --panel "\$@"
    EOS
    chmod +x $out/bin/iliada-ask
    install -Dm644 -t $out/share/applications \
      apps/iliada-store-gui/res/org.iliada.Store.desktop \
      apps/iliada-settings/res/org.iliada.Settings.desktop \
      apps/iliada-settings/res/org.iliada.Setup.desktop \
      apps/iliada-settings/res/org.iliada.Assistant.desktop
    # The installer's entries are only linked in on the live system.
    install -Dm644 -t $out/share/iliada/installer apps/iliada-installer/res/*.desktop
  '';

  meta = {
    description = "Iliada's desktop apps: Store, Settings, Setup Assistant";
    license = with lib.licenses; [ mit asl20 ];
    platforms = lib.platforms.linux;
    mainProgram = "iliada-store-gui";
  };
}
