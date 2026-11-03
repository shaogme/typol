{ src }:
let
  lib = {
    typolConfigIr = import ./lib/typol-config-ir.nix;
    mkConfigIr = arguments: import ./lib/mk-config-ir.nix arguments;
  };
  packageSet = pkgs: import ./package-set.nix { inherit pkgs src; };
  overlay = final: _prev: {
    typol-cli = final.callPackage ./release-package.nix { };
    typol-cli-source = final.callPackage ./package.nix { inherit src; };
  };
in
{
  inherit lib overlay packageSet;
}
