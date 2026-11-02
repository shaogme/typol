{
  description = "Typol CLI package and pure Nix helpers for Typol config IR";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "aarch64-darwin"
      ];
      forAllSystems =
        function:
        builtins.listToAttrs (
          map (system: {
            name = system;
            value = function system;
          }) systems
        );
      packageFor =
        pkgs:
        {
          buildFromSource ? false,
        }:
        pkgs.callPackage ./nix/package.nix {
          src = self.outPath;
          inherit buildFromSource;
        };
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          typol-cli = packageFor pkgs { };
          typol-cli-from-source = packageFor pkgs { buildFromSource = true; };
        in
        {
          inherit typol-cli typol-cli-from-source;
          default = typol-cli;
        }
      );

      apps = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          package = packageFor pkgs { };
          sourcePackage = packageFor pkgs { buildFromSource = true; };
          app = package: {
            type = "app";
            program = "${package}/bin/typol";
            meta.description = "Typol 配置校验与 JSON IR 命令行工具";
          };
        in
        {
          typol-cli = app package;
          typol-cli-from-source = app sourcePackage;
          default = app package;
        }
      );

      checks = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          typol-cli = packageFor pkgs { };
          typol-cli-from-source = packageFor pkgs { buildFromSource = true; };
          typol-config-ir = import ./nix/tests/typol-config-ir.nix { inherit pkgs; };
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              nixfmt
              npins
              rustc
              rustfmt
            ];
          };
        }
      );

      formatter = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        pkgs.nixfmt
      );

      lib.typolConfigIr = import ./nix/lib/typol-config-ir.nix;

      overlays.default = final: _prev: {
        typol-cli = final.callPackage ./nix/package.nix { src = self.outPath; };
        typol-cli-from-source = final.callPackage ./nix/package.nix {
          src = self.outPath;
          buildFromSource = true;
        };
      };
    };
}
