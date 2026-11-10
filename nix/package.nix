{ lib, rustPlatform, pkg-config, zstd, git, bash }:

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
      # iliada-system's tests check the configuration it writes from these.
      ../templates
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  # zstd: used by the Store's tests (archive fixtures) and the catalog builder.
  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ zstd ];
  # iliada-system's tests drive git (the configuration is a repository).
  # bash: the assistant runs commands with it (its tests start it).
  nativeCheckInputs = [ git bash ];

  # Only the default members (crates/*) are built; xtask is dev tooling.
  meta = {
    description = "Iliada system tools: the system service, Store, broker, sandbox and the iliada command";
    homepage = "https://github.com/cseas002/iliada_os";
    license = with lib.licenses; [ mit asl20 ];
    platforms = lib.platforms.unix;
    mainProgram = "iliada-broker";
  };
}
