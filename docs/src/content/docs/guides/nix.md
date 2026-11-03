---
title: Nix integration
description: Validate public configuration in a Nix derivation and consume Config IR with pure Nix helpers.
---

Use the Nix integration when a build needs to validate a Typol configuration or read a public subset of Config IR. The Flake and `default.nix` entry points expose the same `typol-cli`, `lib.typolConfigIr`, `lib.mkConfigIr`, and overlay.

## Build a public Config IR file

`mkConfigIr` receives the Nix package set, CLI package, source directory, and paths relative to that directory:

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

The source directory is kept intact in the derivation so `extern mod` imports resolve beside the root Schema. The CLI uses `--environment empty` and `--reject-secrets`. Invalid configurations fail the build. A configuration containing any `secret: true` value fails before the IR reaches the derivation output.

The source directory and its configuration files become Nix store inputs. Config IR retains the actual value even when marked secret, so do not put credentials in those files. The Nix constructor only creates store outputs for configurations without secret markers. It does not pass arbitrary environment variables into the derivation.

The IR is an ordinary derivation output. Pass its file path to an application derivation, or copy it into a trusted context and read it with the pure helper. Nix evaluation does not automatically read the output.

## Read and select values

`fromText` and `fromFile` decode JSON and validate the complete v1 envelope. `validate` checks an already-decoded Nix value. Defaults are 16 MiB of input, 128 levels of IR value or type nesting, and 100,000 processed JSON/IR nodes. Limits may be lowered; hard caps cannot be raised. The JSON scanner rejects duplicate object keys before `builtins.fromJSON` can fold them. A file passed to `fromFile` is materialized by Nix before its byte limit can be checked.

```nix
let
  ir = typol.lib.typolConfigIr;
  envelope = ir.fromFile { } ./public-config-ir.json;
  decoded = ir.decode { } envelope;
  selected = ir.select {
    paths = [
      [ "endpoint" "host" ]
      [ "endpoint" "port" ]
    ];
  } decoded.value;
  port = ir.toInteger { min = 0; max = 65535; }
    (ir.at { } decoded.value [ "endpoint" "port" ]);
in {
  inherit selected port;
}
```

`at` accepts string selectors for struct fields and map keys, and non-negative integer selectors for array indexes. Its errors identify the IR path, expected type, and actual type without printing node values. Map-key path segments are redacted in diagnostics.

`select` accepts string paths and returns nested attribute sets containing only those paths. It checks each selected subtree for secrets; nearby fields that were not selected do not affect the result. Selecting an entire parent includes its whole subtree and fails if that subtree contains a secret. `includeSecrets = true` is an explicit opt-in for `select`, `at`, typed readers, and `project`.

## Projected value shapes

`project` and `select` return ordinary Nix values with these shapes:

| IR kind | Nix value |
| --- | --- |
| string, bool | Nix string or bool |
| integer, float | Original decimal string, preserving large integers and `-0` |
| option | `null` or the projected inner value |
| array | List, preserving element order |
| map, struct | Attribute set keyed by map key or field name |
| enum | `{ variant = "..."; payload = ...; }`; payload is `{ kind = "unit"; }`, `{ kind = "single"; value = ...; }`, or `{ kind = "struct"; fields = { ... }; }` |

Use `readString`, `readBool`, `readInteger`, and `readFloat` to preserve the declared scalar type when reading a node. `toInteger` requires explicit Nix integer `min` and `max` bounds and rejects values outside them; large values should normally remain strings.
