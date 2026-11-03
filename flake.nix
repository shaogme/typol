{
  description = "Typol CLI package and pure Nix helpers for Typol config IR";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = import ./nix/systems.nix;
      shared = import ./nix/entrypoint.nix { src = self.outPath; };
      forAllSystems =
        function:
        builtins.listToAttrs (
          map (system: {
            name = system;
            value = function system;
          }) systems
        );
      packageSetFor = pkgs: shared.packageSet pkgs;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        packageSetFor pkgs
      );

      apps = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          packageSet = packageSetFor pkgs;
          releaseApp = {
            type = "app";
            program = "${packageSet.typol-cli}/bin/typol";
            meta.description = "Typol 配置校验与 JSON IR 命令行工具";
          };
          sourceApp = {
            type = "app";
            program = "${packageSet.typol-cli-source}/bin/typol";
            meta.description = "Typol CLI built from repository source";
          };
        in
        {
          typol-cli = releaseApp;
          typol-cli-source = sourceApp;
          default = releaseApp;
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          packageSet = packageSetFor pkgs;
        in
        {
          typol-cli-source = packageSet.typol-cli-source;
          typol-config-ir = import ./nix/tests/typol-config-ir.nix { inherit pkgs; };
          nix-config-export = import ./nix/tests/config-export.nix { inherit pkgs; };
          nix-entrypoints = import ./nix/tests/entrypoints.nix {
            inherit pkgs;
            flakePackageSet = packageSet;
            flakeApps = self.apps.${system};
          };
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = import ./nix/dev-shell.nix { inherit pkgs; };
        }
      );

      formatter = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        pkgs.nixfmt
      );

      inherit (shared) lib;
      overlays.default = shared.overlay;
    };
}
