{ pkgs }:
let
  supportedSystems = import ../systems.nix;
  workspace = builtins.fromTOML (builtins.readFile ../../Cargo.toml);
  packageSet = import ../package-set.nix {
    inherit pkgs;
    src = ../../.;
  };
  legacy = import ../../default.nix { inherit pkgs; };
  shared = import ../entrypoint.nix { src = ../../.; };
  package = packageSet.typol-cli;
  legacyPackage = legacy.typol-cli;
  exportArguments = {
    inherit pkgs;
    typolCli = package;
    sourceDir = ../../examples/nix/fixtures;
    schemaPath = "root.tschem";
    configPath = "public.tconf";
  };
  sharedExport = shared.lib.mkConfigIr exportArguments;
  legacyExport = legacy.lib.mkConfigIr exportArguments;
  normalizedPlatforms = platforms: builtins.sort builtins.lessThan platforms;
  overlayPackages = legacy.overlay pkgs pkgs;
  unsupportedSystem = "x86_64-darwin";
  unsupportedPkgs = pkgs // {
    stdenv = pkgs.stdenv // {
      hostPlatform = pkgs.stdenv.hostPlatform // {
        system = unsupportedSystem;
      };
    };
  };
  rejects = value: !(builtins.tryEval (builtins.deepSeq value true)).success;
  rejectsLegacySystem = rejects (
    import ../../default.nix {
      inherit pkgs;
      system = unsupportedSystem;
    }
  );
  rejectsLegacySystemBeforeImport = rejects (
    import ../../default.nix {
      system = unsupportedSystem;
    }
  );
  rejectsLegacyCustomPkgs = rejects (import ../../default.nix { pkgs = unsupportedPkgs; });
  rejectsShellSystem = rejects (
    import ../../shell.nix {
      inherit pkgs;
      system = unsupportedSystem;
    }
  );
  rejectsShellSystemBeforeImport = rejects (
    import ../../shell.nix {
      system = unsupportedSystem;
    }
  );
  rejectsShellCustomPkgs = rejects (import ../../shell.nix { pkgs = unsupportedPkgs; });
  rejectsPackageSystem = rejects (
    pkgs.callPackage ../package.nix {
      src = ../../.;
      stdenv = unsupportedPkgs.stdenv;
    }
  );
in
assert
  builtins.attrNames packageSet == [
    "default"
    "typol-cli"
  ];
assert packageSet.default.drvPath == packageSet.typol-cli.drvPath;
assert package.pname == "typol-cli";
assert package.version == workspace.workspace.package.version;
assert package.meta.mainProgram == "typol";
assert legacy.default.drvPath == legacy.typol-cli.drvPath;
assert
  builtins.attrNames legacy.lib == [
    "mkConfigIr"
    "typolConfigIr"
  ];
assert builtins.attrNames shared.lib == builtins.attrNames legacy.lib;
assert sharedExport.drvPath == legacyExport.drvPath;
assert package.version == legacyPackage.version;
assert package.pname == legacyPackage.pname;
assert package.meta.mainProgram == legacyPackage.meta.mainProgram;
assert normalizedPlatforms package.meta.platforms == normalizedPlatforms supportedSystems;
assert normalizedPlatforms legacyPackage.meta.platforms == normalizedPlatforms supportedSystems;
assert
  normalizedPlatforms package.meta.platforms == normalizedPlatforms legacyPackage.meta.platforms;
assert !(builtins.elem unsupportedSystem supportedSystems);
assert builtins.attrNames overlayPackages == [ "typol-cli" ];
assert builtins.attrNames (legacy.overlays.default pkgs pkgs) == [ "typol-cli" ];
assert rejectsLegacySystem;
assert rejectsLegacySystemBeforeImport;
assert rejectsLegacyCustomPkgs;
assert rejectsShellSystem;
assert rejectsShellSystemBeforeImport;
assert rejectsShellCustomPkgs;
assert rejectsPackageSystem;
pkgs.runCommand "typol-nix-entrypoints-check" { } "touch $out"
