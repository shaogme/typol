---
title: CLI guide
description: Check Typol schemas and configurations, and export validated values as Config IR.
sidebar:
  order: 2
---

The `typol` command line tool checks `.tschem` schemas, validates `.tconf` files, and exports configuration values as typed JSON. It complements the [Rust quick start](../quickstart/): use the CLI to check files directly, or use the Rust library when your application needs to load values itself.

The CLI supports English and Simplified Chinese. Help, command-line errors, operation messages, and detailed schema and configuration diagnostics use the same selected language. The CLI checks are rendered from the build-time `u18n` catalogs in `typol-i18n`.

## Build and run

From the Typol workspace root, run the CLI through Cargo:

```sh
cargo run -p typol-cli -- --help
```

Choose a language before the command with `--language en` or `--language zh-CN`:

```sh
cargo run -p typol-cli -- --language zh-CN --help
```

When `--language` is omitted, the CLI checks `TYPOL_LANG`, `LC_ALL`, `LC_MESSAGES`, and `LANG` in that order, then defaults to English. Locale names such as `zh_CN.UTF-8` are recognized. The option also accepts the `--language=zh-CN` form.

The command after `--` is passed to Typol. To build a standalone release binary, run `cargo build --release -p typol-cli`; the binary is `target/release/typol` (or `target/release/typol.exe` on Windows). The examples below assume you are in the workspace root. File paths are resolved from the current working directory.

## Commands

### Check a Schema

```sh
cargo run -p typol-cli -- schema check examples/cli/cli_demo.tschem
```

This compiles the Schema and its `extern mod` imports. On success, the command exits with status `0` and writes nothing to standard output.

### Check a configuration

```sh
cargo run -p typol-cli -- config check \
  --schema examples/cli/cli_demo.tschem \
  --environment empty \
  examples/cli/cli_demo.tconf
```

`--schema` is required, followed by one configuration file path. The command compiles the Schema, reads the configuration, applies Schema defaults, and validates the resulting value. On success, it exits with status `0` and writes nothing to standard output.

### Export Config IR

```sh
cargo run -p typol-cli -- config export-ir \
  --schema examples/cli/cli_demo.tschem \
  --environment empty \
  --pretty \
  examples/cli/cli_demo.tconf
```

`config export-ir` performs the same Schema and configuration checks, then writes Config IR v1 JSON to standard output. Add `--pretty` for indented JSON; without it, the output is compact. The output ends with a newline, so it can be redirected or piped to another program:

```sh
cargo run -p typol-cli -- config export-ir \
  --schema examples/cli/cli_demo.tschem \
  --environment empty \
  examples/cli/cli_demo.tconf > config.json
```

See the [Config IR v1 specification](../../specification/config-ir/) for the JSON structure and compatibility rules.

Add `--reject-secrets` when the IR would become a Nix store input. The command then fails before writing output if any node is marked `secret: true`. This option rejects secret-marked configurations; it does not redact their values.

## Environment selection

`--environment` accepts two values:

| Value | Behavior |
| --- | --- |
| `inherit` | Read variables from the process environment. This is the default when the option is omitted. |
| `empty` | Do not read process variables. An `env("NAME", fallback)` expression uses its fallback; without a fallback, a required value cannot be resolved. |

For example, the demo configuration reads `TYPOL_CLI_DEMO_KEY`. With the default `inherit` mode, that variable can provide the value. With `empty`, the expression's fallback is used instead. As described in the [Config DSL](../../specification/config-dsl/), the target Schema field must authorize the variable with a matching `@env("NAME")` annotation.

## Options and help

```text
typol schema check <schema.tschem>
typol config check --schema <schema.tschem> [--environment inherit|empty] <config.tconf>
typol config export-ir --schema <schema.tschem> [--environment inherit|empty] [--pretty] [--reject-secrets] <config.tconf>
```

- `--schema <path>` supplies the Schema file for either `config` command.
- `--environment inherit|empty` selects the environment provider for configuration reads.
- `--pretty` is available only for `config export-ir`.
- `--reject-secrets` is available only for `config export-ir`; it prevents output when any secret marker is present.
- `--language en|zh-CN` selects the language for CLI messages and detailed diagnostics, and must appear before the command. `TYPOL_LANG` can set the same preference.
- `-h` and `--help` print command help. For example, use `typol --help`, `typol schema check --help`, or `typol config export-ir --help`.
- In a `config` command, put `--` before a configuration path that begins with `-`, so it is treated as a path rather than an option.

The CLI accepts file paths; it does not read Schema or configuration text from standard input. Schema imports are resolved relative to the importing Schema file.

## Output and exit status

Successful check commands are quiet. `config export-ir` writes JSON to standard output. Diagnostics and command-line errors go to standard error.

| Status | Meaning |
| ---: | --- |
| `0` | Success, including help output. |
| `1` | Schema or configuration diagnostics, such as invalid syntax or a failed validation. |
| `2` | Invalid command-line usage, such as a missing argument or unknown option. |
| `3` | Host or I/O failure, translation rendering failure, or failure to write or serialize output. |

`@secret` values are hidden in safe diagnostics, but Config IR includes the actual values and adds `"secret": true` markers. Treat exported JSON as sensitive data and handle it accordingly.

For examples covering enums, imported modules, maps, arrays, and numeric edge cases, see `examples/cli/CLI.md` in the repository.
