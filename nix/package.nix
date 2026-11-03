{
  rustPlatform,
  stdenv,
  src,
}:
let
  workspace = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  version = workspace.workspace.package.version;
  system = stdenv.hostPlatform.system;
  supportedSystems = import ./systems.nix;
  supportedSystem = import ./assert-supported-system.nix system;
in
builtins.seq supportedSystem (
  rustPlatform.buildRustPackage {
    pname = "typol-cli";
    inherit version src;

    cargoLock.lockFile = ../Cargo.lock;
    cargoBuildFlags = [
      "-p"
      "typol-cli"
      "--bin"
      "typol"
    ];
    cargoTestFlags = [
      "-p"
      "typol-cli"
    ];

    doCheck = true;

    meta = {
      description = "Typol configuration validation and typed JSON IR export tool";
      mainProgram = "typol";
      platforms = supportedSystems;
    };
  }
)
