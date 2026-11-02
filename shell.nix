{
  sources ? import ./npins,
  system ? builtins.currentSystem,
  pkgs ? import sources.nixpkgs { inherit system; },
}:
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
