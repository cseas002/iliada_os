# The Iliada Store (COSMIC GUI). Built separately from the core tools because
# it depends on libcosmic (git dependencies, GPU rendering stack).
{ lib, rustPlatform, libcosmicAppHook }:

rustPlatform.buildRustPackage {
  pname = "iliada-store-gui";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
      ../apps/iliada-store-gui
    ];
  };
  cargoRoot = "apps/iliada-store-gui";
  buildAndTestSubdir = "apps/iliada-store-gui";
  cargoLock = {
    lockFile = ../apps/iliada-store-gui/Cargo.lock;
    allowBuiltinFetchGit = true;
  };

  nativeBuildInputs = [ libcosmicAppHook ];
  doCheck = false;

  postInstall = ''
    install -Dm644 apps/iliada-store-gui/res/org.iliada.Store.desktop \
      $out/share/applications/org.iliada.Store.desktop
  '';

  meta = {
    description = "The Iliada Store";
    license = with lib.licenses; [ mit asl20 ];
    platforms = lib.platforms.linux;
    mainProgram = "iliada-store-gui";
  };
}
