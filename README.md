# Typol

[简体中文](README_CN.md) · [Documentation](https://shaogme.github.io/typol/)

Typol is a schema-driven configuration library for Rust. It compiles `.tschem` schemas, parses `.tconf` configuration values against the compiled types, validates constraints and assertions, and can bind the result to Rust types with `#[derive(Typol)]`.

The workspace requires Rust 1.85 or newer. The examples below use the `typol` facade crate; the lower-level crates can also be used independently.

The facade defaults to `no_std` and uses `alloc` for owned values. Its in-memory loader accepts Schema text or a resolver, and configuration reads take text or a caller-provided `ByteSource` plus an explicit `Environment` provider. The `serde` and `config-ir` features remain available without `std`; Config IR serialization returns an `alloc`-backed `Vec<u8>`. Enable `std` for file and reader inputs, process environment access, and host diagnostic rendering.

To read process variables, construct `ProcessEnvironment` with `ProcessEnvironmentLimits` and pass it to `BoundSchema::read` or `LoadedSchema::read_value`. Both types are exported from the facade root. Environment limits belong to the provider, so applications can choose them per read policy.

## Quick start

This example uses the `std` host adapter to compile a schema in memory, bind it to a Rust type, read configuration text, apply schema defaults, and resolve an authorized environment expression through an explicit provider. Add `features = ["std"]` to the `typol` dependency for this API.

```toml
[dependencies]
typol = { version = "0.1", features = ["std"] }
```

```rust
use typol::{
    opeo::ErrSlot, ConfigInput, EmptyEnvironment, SchemaLoader, Typol, TypolError, TypolOptions,
};

#[derive(Debug, PartialEq, Typol)]
struct ServiceConfig {
    name: String,
    port: u16,
    tags: Vec<String>,
    enabled: bool,
    endpoint: String,
}

let mut errors = ErrSlot::<TypolError>::new();
let result = errors.try_call(|mut out| {
    let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
        b"readme",
        r#"
entry struct ServiceConfig {
    name: string @length(1, 64),
    port: u16 = 8080,
    tags: [string] @size(min = 1, max = 8),
    enabled: bool = true,
    endpoint: string @env("APP_ENDPOINT") = "127.0.0.1:8080",
}
"#,
        out.reborrow(),
    )?;
    let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
    let mut environment = EmptyEnvironment;
    let config = binding.read(
        ConfigInput::text(
            r#"{ name: "edge", tags: ["api"], endpoint: env("APP_ENDPOINT") }"#,
        ),
        &mut environment,
        out,
    )?;

    assert_eq!(
        config,
        ServiceConfig {
            name: "edge".into(),
            port: 8080,
            tags: vec!["api".into()],
            enabled: true,
            endpoint: "127.0.0.1:8080".into(),
        }
    );
    Ok(())
});

assert!(result.is_ok());
```

Fallible facade operations receive an `Out` error context and return an OPEO result. `ErrSlot::try_call` is one way to run such an operation and retrieve its error. `compile_text` is for schemas without imports. For `extern mod` schemas, use `SchemaLoader::compile_file` or the lower-level compiler with a `SchemaResolver`.

## Capabilities

- **Schema DSL:** structs, enums with unit/single/record payloads, aliases, nested modules, finite recursive types, defaults, typed assertions, and validation annotations.
- **Config DSL:** typed scalars, options, arrays, fixed-size arrays, string-keyed maps, enum values, comments, and explicit `env("NAME")` expressions.
- **Validation:** scalar and collection constraints, precompiled regular expressions, `@all` predicates, and `@assert` checks over completed values.
- **Rust integration:** `#[derive(Typol)]` describes Rust structs and enums; binding checks their shape against the compiled entry type. Bound schemas can read and write multiple values.
- **Host adapters:** with the `std` feature, `typol-std` supplies file loading, reader inputs, process environment access, and safe diagnostic rendering. The core parser and compiler do not access the filesystem or process environment directly.
- **Typed JSON export:** enable `config-ir` to export Config IR v1. The format preserves schema type information and deterministic ordering. `secret: true` marks sensitive nodes but does not remove or encrypt their values.
- **Resource bounds:** parsing, schema compilation, validation, regular expressions, diagnostics, and IR serialization use configurable limits and report resource failures instead of returning partial results.

Regular expressions use Typol's documented subset and search semantics. Use `^` and `$` when a pattern must match the entire string. `@secret` controls diagnostic and safe-display behavior; application values and serialized output still contain the actual data.

Recursive schemas can describe finite trees and recursive enums. Rust derive supports recursive edges through `Vec<Self>` and the built-in map types; a recursive type still needs a finite branch, and each value remains subject to the configured depth and work limits. See the [recursive Schema](examples/config/recursive.tschem) and [finite Config value](examples/config/recursive.tconf) example:

```rust
#[derive(Typol)]
struct Node {
    value: String,
    children: Vec<Node>,
}
```

## Write bound values

Pass `WriteOptions` directly to the bound writer. Compact output remains the default; use `with_pretty(true)` for two-space indentation:

```rust
use typol::WriteOptions;

let text = binding.write(
    &config,
    WriteOptions::default().with_pretty(true),
    out,
)?;
```

The options argument is now part of `BoundSchema::write` and `BoundSchema::write_serde`; migrate calls from `write(value, out)` to `write(value, options, out)`. The old `write_with_options` and `write_serde_with_options` methods are removed. `TypedSchema::serialize` and `serialize_serde` use `CodecOptions::with_pretty` for the same format choice. Formatting whitespace counts toward the output limit, and no partial text is returned on failure. Serialization retains actual `@secret` values.

## Handwritten `Typol` implementations

`LoadedSchema::bind` and `TypedSchema::bind` are for built-in types and derive-generated types whose nested fields and enum payloads also carry the hidden `BudgetedTypol` marker. Mark a derived type that contains a handwritten implementation with `#[typol(trusted)]`; it remains available through `LoadedSchema::bind_trusted` or `TypedSchema::bind_trusted`, and its returned `TrustedBoundSchema` or `TrustedTypedSchema` keeps that choice visible in the Rust type. Load the schema with `SchemaLoader`, bind the Rust type, and read configuration through the resulting binding; for handwritten types, call `bind_trusted` explicitly.

Library budgets cover library-managed conversion nodes, depth, workspace, output, and context-routed target storage. They are not operating-system CPU, process-memory, or real-time limits. A trusted callback can still allocate, block, or run arbitrary code, and its returned Rust value is not revalidated. Use derive for ordinary record and enum mappings; keep handwritten implementations for special mappings that the application has reviewed and trusts. If code is untrusted, run it behind a separate process or Wasm boundary. Caller-provided Serde implementations remain a separate user-code trust boundary even on a regular `BudgetedTypol` binding.

## Workspace crates

| Crate | Responsibility |
| --- | --- |
| `typol` | Public facade, Rust type binding, loading, codecs, and optional Config IR |
| `typol-syntax` | Shared lexer, tokens, and Schema parser/AST |
| `typol-schema` | Schema imports, name and type resolution, static checks, and compilation |
| `typol-semantic` | Immutable compiled schema model, constraint views, expression programs, and evaluation |
| `typol-config` | Streaming, schema-guided configuration parsing and validation |
| `typol-regex` | Bounded regular-expression compiler and matcher |
| `typol-core` | Shared values, limits, budgets, source spans, and diagnostic types |
| `typol-std` | Filesystem, process environment, and diagnostic host adapters |
| `typol-i18n` | English and Simplified Chinese CLI translations compiled by `u18n` |
| `typol-macros` | `#[derive(Typol)]` implementation |
| `typol-cli` | Schema checking, configuration checking, and Config IR export commands |

## Documentation

The [Typol documentation site](docs/src/content/docs/index.md) is built with Starlight and includes English and Simplified Chinese editions. The DSL and format specifications are normative; implementation documents describe how the current crates realize those contracts.

| Document | Description |
| --- | --- |
| [Quick start](docs/src/content/docs/guides/quickstart.md) | Beginner tutorial for Schema, Config, and Rust type binding |
| [no_std guide](docs/src/content/docs/guides/no_std.md) | In-memory loading, alloc, and explicit providers without `std` |
| [CLI guide](docs/src/content/docs/guides/cli.md) | Build and use the CLI to check files and export Config IR |
| [Advanced tutorial](docs/src/content/docs/guides/advanced.md) | Imports, validation, Rust bindings, Serde, resource limits, and Config IR |
| [Config DSL](docs/src/content/docs/specification/config-dsl.md) | `.tconf` syntax, values, defaults, options, and environment expressions |
| [Schema DSL](docs/src/content/docs/specification/schema-dsl.md) | `.tschem` syntax, types, annotations, assertions, and regex subset |
| [Config implementation](docs/src/content/docs/specification/config-implementation.md) | Streaming parse, validation, environment handling, and diagnostics |
| [Schema implementation](docs/src/content/docs/specification/schema-implementation.md) | Imports, static compilation, compiled records, and runtime use |
| [Config IR v1](docs/src/content/docs/specification/config-ir.md) | Typed JSON export format, ordering, secret markers, and limits |
| [Regex implementation](docs/src/content/docs/specification/regex-implementation.md) | Regex compilation, NFA search, workspace, and resource bounds |
| [Showcase](examples/showcase/README.md) | End-to-end file-based schema and configuration example |
| [CLI examples](examples/cli/CLI.md) | CLI checks and Config IR export examples |

Run the documentation site locally from the repository root:

```sh
cd docs
npm install
npm run dev
```

Use `npm run build` in `docs` to generate the production site.

## Build and test

From the workspace root:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run the showcase or check a schema with the CLI:

```sh
cargo run -p typol-showcase
cargo run -p typol-cli -- schema check examples/cli/cli_demo.tschem
cargo run -p typol-cli -- config check --schema examples/cli/cli_demo.tschem --environment empty examples/cli/cli_demo.tconf
cargo run -p typol-cli -- --language zh-CN --help
```

The CLI selects English or Simplified Chinese with `--language en|zh-CN`. When omitted, it checks
`TYPOL_LANG`, `LC_ALL`, `LC_MESSAGES`, and `LANG` in that order, then defaults to English. CLI help,
usage errors, and operation errors use the generated `u18n` catalogs. Typol's detailed schema and
configuration diagnostics are currently rendered in English by the library. Locale detection
normalizes POSIX names such as `en_US.UTF-8` to BCP 47 tags; regional English tags use the generated
catalog fallback, while any `zh-*` tag selects the single Simplified Chinese catalog. Catalog tag
fallback only chooses an included catalog and does not provide missing message translations.
Translation errors remain visible to the caller and are not replaced with fixed fallback text.

## Nix

Nix supports `x86_64-linux`, `aarch64-linux`, and `aarch64-darwin`. `x86_64-darwin` is rejected by
the Flake, traditional package entry, overlay, and development shell. The default `typol-cli` package
fetches the matching GitHub Release binary. Before a compatible Nix Release is available, select the
source package explicitly:

```sh
nix build .#typol-cli
nix run .#typol-cli -- --help
nix build .#typol-cli-source
nix run .#typol-cli-source -- --help
nix flake check
nix develop
nix fmt

nix-build -A typol-cli
nix-build -A typol-cli-source
nix-shell
```

Both Nix entry points expose the same release package, source package, default app, and overlay
names. The source package always builds from the repository; the default package never switches to
source automatically and explains how to select `typol-cli-source` when its Release is unavailable.

Nix consumers can use `lib.mkConfigIr` to validate a Schema and Config in a derivation, or use
`lib.typolConfigIr` to read a pre-generated public IR file. The derivation uses an empty
environment and rejects secret-marked configurations. Source configuration files become Nix store
inputs, so they must not contain credentials. Config IR still contains actual values when marked
`secret: true`; see the [Nix integration guide](docs/src/content/docs/guides/nix.md) and
[integration example](examples/nix/README.md).

The project is dual-licensed under Apache-2.0 or MIT; see [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT).
