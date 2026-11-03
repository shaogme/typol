{ pkgs }:
let
  sourceDir = ../../examples/nix/fixtures;
  entrypoint = import ../entrypoint.nix { src = ../../.; };
  typolCli = (entrypoint.packageSet pkgs).typol-cli-source;
  publicIr = entrypoint.lib.mkConfigIr {
    inherit pkgs typolCli sourceDir;
    schemaPath = "root.tschem";
    configPath = "public.tconf";
  };
in
pkgs.runCommand "typol-nix-config-export-check"
  {
    nativeBuildInputs = [ typolCli ];
    src = sourceDir;
  }
  ''
    set -eu
    cli=${typolCli}/bin/typol

    test -s ${publicIr}/config-ir.json
    cd "$src"
    TYPOL_NIX_REGION=build-machine-value "$cli" config export-ir \
      --schema root.tschem \
      --environment empty \
      --reject-secrets \
      public.tconf > "$TMPDIR/direct.json"
    cmp ${publicIr}/config-ir.json "$TMPDIR/direct.json"
    grep -Fq 'config-fallback' ${publicIr}/config-ir.json
    ! grep -Fq 'build-machine-value' ${publicIr}/config-ir.json

    if "$cli" config export-ir \
      --schema root.tschem \
      --environment empty \
      --reject-secrets \
      invalid.tconf > "$TMPDIR/invalid.json" 2> "$TMPDIR/invalid.stderr"; then
      exit 1
    fi
    test ! -s "$TMPDIR/invalid.json"

    if "$cli" config export-ir \
      --schema secret.tschem \
      --environment empty \
      --reject-secrets \
      secret.tconf > "$TMPDIR/secret.json" 2> "$TMPDIR/secret.stderr"; then
      exit 1
    fi
    test ! -s "$TMPDIR/secret.json"
    ! grep -Fq 'fixture-secret-value' "$TMPDIR/secret.stderr"
    touch "$out"
  ''
