{
  sources ? import ./npins,
  system ? null,
  pkgs ? null,
}:
let
  assertSupportedSystem = import ./nix/assert-supported-system.nix;
  requestedSystem =
    if system != null then
      system
    else if pkgs != null then
      pkgs.stdenv.hostPlatform.system
    else
      builtins.currentSystem;
  checkedSystem = assertSupportedSystem requestedSystem;
  selectedPkgs = if pkgs != null then pkgs else import sources.nixpkgs { system = requestedSystem; };
  checkedPackageSystem = assertSupportedSystem selectedPkgs.stdenv.hostPlatform.system;
in
builtins.seq checkedSystem (
  builtins.seq checkedPackageSystem (import ./nix/dev-shell.nix { pkgs = selectedPkgs; })
)
