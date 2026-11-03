{
  pkgs,
  src,
}:
let
  assertSupportedSystem = import ./assert-supported-system.nix;
  system = pkgs.stdenv.hostPlatform.system;
  package = pkgs.callPackage ./package.nix { inherit src; };
in
builtins.seq (assertSupportedSystem system) {
  typol-cli = package;
  default = package;
}
