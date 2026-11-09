{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "iliada-tools";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../.cargo
      ../crates
      ../xtask
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  # Only the default members (crates/*) are built; xtask is dev tooling.
  meta = {
    description = "Iliada system daemons: permission broker and ingestion pipeline";
    homepage = "https://github.com/cseas002/iliada_os";
    license = with lib.licenses; [ mit asl20 ];
    platforms = lib.platforms.unix;
    mainProgram = "iliada-broker";
  };
}
