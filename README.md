# Typol

[简体中文](README_CN.md) · [Documentation site](https://shaogme.github.io/typol/)

Typol is a schema-driven configuration library for Rust. A `.tschem` file declares the configuration types and rules; a `.tconf` file supplies values. Typol compiles the Schema, parses and validates the configuration, then can convert it to matching Rust structs and enums with `#[derive(Typol)]`.

The workspace requires Rust 1.85 or newer. The examples below use the `typol` facade crate.

## Start here

Add Typol with the `std` feature when your application reads files or uses the process environment:

```toml
[dependencies]
typol = { version = "0.1", features = ["std"] }
```

The repository includes a complete file-based example:

```sh
cargo run -p typol-showcase
```

For a guided application walkthrough, start with the [Quick start](docs/src/content/docs/guides/quickstart.md). It explains the Schema, configuration, Rust type mapping, and the methods that connect them.

## The data model

A Schema describes types and validation rules. Configuration text provides an instance of the entry type.

```tschem
struct Endpoint {
    host: string,
    port: u16 = 8080,
}

entry struct ServiceConfig {
    name: string,
    endpoint: Endpoint,
    tags: [string],
    note: option<string>,
}
```

`Endpoint` and `ServiceConfig` are Schema structs. `entry` selects the root configuration type. The application may define Rust structs with the same names and matching fields; `#[derive(Typol)]` generates their conversion mapping. Rust methods and field visibility remain under the application's control.

At a glance, common Schema types map to Rust as follows:

| Schema type | Rust type |
| --- | --- |
| `bool`, `string`, integer and float scalars | `bool`, `String`, same-width integer or float |
| `option<T>` | `Option<T>` |
| `[T]` | `Vec<T>` |
| `[T; N]` | `[T; N]` |
| `map<string, T>` | `HashMap<String, T>` with `std`, or `OrderedMap<T>` without it |
| Schema `struct` / `enum` | Rust `struct` / `enum` with `#[derive(Typol)]` |

Field names and types must agree between the Schema and Rust declaration. See the [Quick start](docs/src/content/docs/guides/quickstart.md) for examples, including private fields and accessor methods.

## How the Rust API fits together

The main path has three stages:

| Stage | Type or method | Purpose |
| --- | --- | --- |
| Compile | `SchemaLoader::compile_text` or `compile_file` | Check a Schema and produce a `LoadedSchema`. File loading requires `std`. |
| Bind | `LoadedSchema::bind::<T>()` | Check Rust type `T` against the Schema entry and produce a reusable `BoundSchema<T>`. |
| Read | `BoundSchema::read` or `read_file` | Parse, apply defaults, validate, and return `T`. Every read receives an explicit `Environment` provider. |

`SchemaLoader::new(options)` is an associated function that creates the loader. The other entries above are methods called on a loader, loaded Schema, or binding. A `LoadedSchema` and its binding can be reused to read multiple configuration values.

Operations that can fail take an OPEO `Out` error context and return an OPEO result. The Quick start shows how `opeo::ErrSlot::try_call` captures and reports a `TypolError`.

Configuration inputs are explicit too: `BoundSchema::read` receives a `ReadRequest` containing a `Read` source such as `SliceSource`. With `std`, `read_file` and `read_io` adapt host files and readers. Each read also receives an `Environment` provider; Typol never consults process variables implicitly.

## Workspace structure

The source is split by responsibility. The normal application API is in `crates/typol`; most users do not need to depend on the lower-level crates directly.

| Crate | Responsibility |
| --- | --- |
| `typol` | Public facade, loading, Rust bindings, codecs, and optional Config IR |
| `typol-macros` | Implements `#[derive(Typol)]` |
| `typol-syntax` | Lexer, Schema parser, and syntax tree |
| `typol-model` | Shared Schema operator and built-in type definitions |
| `typol-schema` | Imports, name and type resolution, static checks, and Schema compilation |
| `typol-semantic` | Compiled Schema model, constraints, expression programs, and evaluation |
| `typol-config` | Schema-guided configuration parsing, validation, formatting, and source-preserving documents |
| `typol-regex` | Bounded regular-expression compiler and matcher |
| `typol-core` | Shared values, sources, diagnostics, and execution budgets |
| `typol-policy` | Shared resource-limit policies |
| `typol-report` | Localized diagnostic report formatting |
| `typol-std` | Optional file, process-environment, and diagnostic host adapters |
| `typol-i18n` | English and Simplified Chinese message catalogs |
| `typol-cli` | Schema and configuration checks, `.tschem` and `.tconf` formatting, and Config IR export |

## Choose a guide

| If you want to… | Read |
| --- | --- |
| Build a typed configuration loader and understand the core types and methods | [Quick start](docs/src/content/docs/guides/quickstart.md) |
| Use imported Schemas, cross-field checks, Serde, or Config IR | [Advanced tutorial](docs/src/content/docs/guides/advanced.md) |
| Use Typol without the standard library | [no_std guide](docs/src/content/docs/guides/no_std.md) |
| Check or format files, or export Config IR from the command line | [CLI guide](docs/src/content/docs/guides/cli.md) |
| Look up Schema or configuration syntax | [Schema DSL](docs/src/content/docs/specification/schema-dsl.md) · [Config DSL](docs/src/content/docs/specification/config-dsl.md) |
| Understand implementation details | [Schema implementation](docs/src/content/docs/specification/schema-implementation.md) · [Config implementation](docs/src/content/docs/specification/config-implementation.md) |

The documentation site has English and Simplified Chinese editions. Its DSL and format specifications define language behavior; implementation pages describe how the current crates realize those rules.

## Features and build

`typol` has no default features. Add only the capabilities the application needs:

| Feature | Adds |
| --- | --- |
| `std` | File and reader inputs, file-based Schema loading, process environment access, and host diagnostic rendering |
| `serde` | Schema-aware Serde reads and writes |
| `hashbrown` | `hashbrown::HashMap<String, T>` bindings |
| `btree-map` | `BTreeMap<String, T>` bindings |
| `config-ir` | Config IR v1 JSON serialization; also enables `serde` |

Without `std`, provide Schema and configuration sources in memory and pass an `Environment` implementation to each read. An allocator is required for owned values.

From the workspace root:

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Build the documentation site from `docs` with `npm run build`. For Nix packaging, see the [Nix guide](docs/src/content/docs/guides/nix.md). The project is dual-licensed under Apache-2.0 or MIT; see [LICENSE-APACHE](LICENSE-APACHE) and [LICENSE-MIT](LICENSE-MIT).
