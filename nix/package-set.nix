{
  pkgs,
  src,
}:
let
  assertSupportedSystem = import ./assert-supported-system.nix;
  system = pkgs.stdenv.hostPlatform.system;
  releasePackage = pkgs.callPackage ./release-package.nix { };
  sourcePackage = pkgs.callPackage ./package.nix { inherit src; };
in
builtins.seq (assertSupportedSystem system) {
  typol-cli = releasePackage;
  typol-cli-source = sourcePackage;
  default = releasePackage;
}
