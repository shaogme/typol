{
  pkgs,
  flakePackageSet,
  flakeApps,
}:
let
  supportedSystems = import ../systems.nix;
  workspace = builtins.fromTOML (builtins.readFile ../../Cargo.toml);
  packageSet = import ../package-set.nix {
    inherit pkgs;
    src = ../../.;
  };
  legacy = import ../../default.nix { inherit pkgs; };
  shared = import ../entrypoint.nix { src = ../../.; };
  sharedPackageSet = shared.packageSet pkgs;
  package = packageSet.typol-cli;
  sourcePackage = packageSet.typol-cli-source;
  legacyPackage = legacy.typol-cli;
  legacySourcePackage = legacy.typol-cli-source;
  exportArguments = {
    inherit pkgs;
    typolCli = sourcePackage;
    sourceDir = ../../examples/nix/fixtures;
    schemaPath = "root.tschem";
    configPath = "public.tconf";
  };
  sharedExport = shared.lib.mkConfigIr exportArguments;
  legacyExport = legacy.lib.mkConfigIr exportArguments;
  normalizedPlatforms = platforms: builtins.sort builtins.lessThan platforms;
  overlayPackages = legacy.overlay pkgs pkgs;
  defaultOverlayPackages = legacy.overlays.default pkgs pkgs;
  unsupportedSystem = "x86_64-darwin";
  unsupportedPkgs = pkgs // {
    stdenv = pkgs.stdenv // {
      hostPlatform = pkgs.stdenv.hostPlatform // {
        system = unsupportedSystem;
      };
    };
  };
  unsupportedStdenvNoCC = pkgs.stdenvNoCC // {
    hostPlatform = pkgs.stdenvNoCC.hostPlatform // {
      system = unsupportedSystem;
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
  rejectsReleasePackageSystem = rejects (
    pkgs.callPackage ../release-package.nix {
      stdenvNoCC = unsupportedStdenvNoCC;
    }
  );
in
assert
  builtins.attrNames packageSet == [
    "default"
    "typol-cli"
    "typol-cli-source"
  ];
assert packageSet.default.drvPath == packageSet.typol-cli.drvPath;
assert sharedPackageSet.default.drvPath == packageSet.default.drvPath;
assert sharedPackageSet.typol-cli-source.drvPath == packageSet.typol-cli-source.drvPath;
assert package.pname == "typol-cli";
assert package.version == workspace.workspace.package.version;
assert package.meta.mainProgram == "typol";
assert sourcePackage.pname == "typol-cli-source";
assert sourcePackage.version == workspace.workspace.package.version;
assert sourcePackage.drvPath == legacySourcePackage.drvPath;
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
assert sourcePackage.meta.mainProgram == "typol";
assert normalizedPlatforms package.meta.platforms == normalizedPlatforms supportedSystems;
assert normalizedPlatforms sourcePackage.meta.platforms == normalizedPlatforms supportedSystems;
assert normalizedPlatforms legacyPackage.meta.platforms == normalizedPlatforms supportedSystems;
assert
  normalizedPlatforms package.meta.platforms == normalizedPlatforms legacyPackage.meta.platforms;
assert !(builtins.elem unsupportedSystem supportedSystems);
assert
  builtins.attrNames overlayPackages == [
    "typol-cli"
    "typol-cli-source"
  ];
assert
  builtins.attrNames defaultOverlayPackages == [
    "typol-cli"
    "typol-cli-source"
  ];
assert overlayPackages.typol-cli.drvPath == package.drvPath;
assert overlayPackages.typol-cli-source.drvPath == sourcePackage.drvPath;
assert
  builtins.attrNames flakeApps == [
    "default"
    "typol-cli"
    "typol-cli-source"
  ];
assert flakeApps.default.program == "${flakePackageSet.typol-cli}/bin/typol";
assert flakeApps.typol-cli.program == flakeApps.default.program;
assert flakeApps.typol-cli-source.program == "${flakePackageSet.typol-cli-source}/bin/typol";
assert rejectsLegacySystem;
assert rejectsLegacySystemBeforeImport;
assert rejectsLegacyCustomPkgs;
assert rejectsShellSystem;
assert rejectsShellSystemBeforeImport;
assert rejectsShellCustomPkgs;
assert rejectsPackageSystem;
assert rejectsReleasePackageSystem;
pkgs.runCommand "typol-nix-entrypoints-check" { } "touch $out"
