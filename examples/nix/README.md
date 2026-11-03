# Nix Integration Example

This example reads a pre-generated Config IR file and selects public values for an application. The default input is `../cli/cli_demo-ir-v1.json`; use the `irFile` parameter to specify another exported file.

From the repository root, run the following commands to export the Config IR and inspect the example output:

```sh
nix run .#typol-cli -- config export-ir \
  --schema examples/cli/cli_demo.tschem \
  --environment empty \
  examples/cli/cli_demo.tconf > /tmp/typol-config-ir.json

nix eval --impure --json --expr \
  'import ./examples/nix/default.nix { irFile = /tmp/typol-config-ir.json; }'
```

The example uses `fromFile`, `decode`, `select`, `at`, and `toInteger` from `nix/lib/typol-config-ir.nix`. The selected output contains only the endpoint, labels, attempts, and optional comment; it does not include the secret API key. Integer conversion declares an explicit Nix integer range, while the IR itself keeps the original decimal strings.

`select` checks secrets only in each selected subtree. A selected subtree containing `secret: true` fails by default. Set `includeSecrets = true` explicitly to read one; this does not redact or encrypt its contents. Selecting public paths does not include nearby unselected secret fields.

The high-level constructor creates a derivation that validates the Schema and configuration in the build sandbox and writes a public IR file:

```nix
let
  typol = import ../.. { inherit pkgs; };
in
typol.lib.mkConfigIr {
  inherit pkgs;
  typolCli = typol.typol-cli;
  sourceDir = ./fixtures;
  schemaPath = "root.tschem";
  configPath = "public.tconf";
}
```

It always uses an empty environment and enables `--reject-secrets`. The complete source directory is a derivation input so relative Schema imports work. Source configuration files enter the Nix store, so never put credentials in them. The IR format still contains actual values when marked secret; the constructor refuses to create a store output for such a configuration. For applications that already have a separately generated public IR file, use the pure helper directly and avoid evaluation-time derivation reads.

Integers remain decimal strings by default and floating-point values remain strings, including `-0`. The repository's `nix flake check` covers IR validation, path selection, numeric bounds, JSON duplicate-key rejection, input budgets, secret handling, imported Schemas, and rejected builds for invalid or secret configurations.
