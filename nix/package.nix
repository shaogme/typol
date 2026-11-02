{
  lib,
  autoPatchelfHook,
  fetchzip,
  rustPlatform,
  stdenv,
  src,
  buildFromSource ? false,
  releaseHashes ? import ./release-hashes.nix,
}:
let
  workspace = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  version = workspace.workspace.package.version;
  releaseTargets = {
    "x86_64-linux" = "x86_64-unknown-linux-gnu";
    "aarch64-linux" = "aarch64-unknown-linux-gnu";
    "aarch64-darwin" = "aarch64-apple-darwin";
  };
  system = stdenv.hostPlatform.system;
  target = releaseTargets.${system} or null;
  releaseHash = releaseHashes.${system} or null;

  sourcePackage = rustPlatform.buildRustPackage {
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
      description = "Typol 配置校验与类型化 JSON IR 导出工具";
      mainProgram = "typol";
      platforms = builtins.attrNames releaseTargets;
    };
  };

  releasePackage =
    let
      archive = "typol-cli-${target}-v${version}.tar.gz";
    in
    stdenv.mkDerivation {
      pname = "typol-cli";
      inherit version;

      src = fetchzip {
        url = "https://github.com/shaogme/typol/releases/download/typol-cli-v${version}/${archive}";
        sha256 = releaseHash;
      };

      nativeBuildInputs = lib.optionals stdenv.hostPlatform.isLinux [ autoPatchelfHook ];
      buildInputs = lib.optionals stdenv.hostPlatform.isLinux [ stdenv.cc.cc.lib ];

      installPhase = ''
        runHook preInstall
        install -Dm755 typol "$out/bin/typol"
        runHook postInstall
      '';

      meta = {
        description = "Typol 配置校验与类型化 JSON IR 命令行工具";
        mainProgram = "typol";
        platforms = builtins.attrNames releaseTargets;
      };
    };
in
if buildFromSource || target == null || releaseHash == null then sourcePackage else releasePackage
