{ pkgs }:
let
  assertSupportedSystem = import ./assert-supported-system.nix;
in
builtins.seq (assertSupportedSystem pkgs.stdenv.hostPlatform.system) (
  pkgs.mkShell {
    packages = with pkgs; [
      cargo
      clippy
      nixfmt
      npins
      rustc
      rustfmt
    ];
  }
)
