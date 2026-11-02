{
  sources ? import ./npins,
  system ? builtins.currentSystem,
  pkgs ? import sources.nixpkgs { inherit system; },
  buildFromSource ? false,
}:
let
  typol-cli = pkgs.callPackage ./nix/package.nix {
    src = ./.;
    inherit buildFromSource;
  };
  typol-cli-from-source = pkgs.callPackage ./nix/package.nix {
    src = ./.;
    buildFromSource = true;
  };
in
{
  inherit typol-cli typol-cli-from-source;
  default = typol-cli;
  lib.typolConfigIr = import ./nix/lib/typol-config-ir.nix;
  overlay = final: _prev: {
    typol-cli = final.callPackage ./nix/package.nix { src = ./.; };
    typol-cli-from-source = final.callPackage ./nix/package.nix {
      src = ./.;
      buildFromSource = true;
    };
  };
}
