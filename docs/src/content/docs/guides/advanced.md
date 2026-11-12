---
title: Advanced tutorial
description: Reuse bindings, compose Schemas, validate related fields, and choose Rust and JSON integration paths.
sidebar:
  order: 3
---

This tutorial extends the [Quick start](../quickstart/). Start with the core object flow:

`SchemaLoader` compiles a Schema into `LoadedSchema`; `LoadedSchema::bind::<T>()` checks a Rust type and returns a reusable `BoundSchema<T>`; that binding reads configuration into `T`. Keep the loaded Schema and binding when the application reads multiple files.

File APIs and process-environment access require the `std` feature. In-memory sources and custom providers also work without `std`; see the [no_std guide](../no_std/). Rust snippets below are method bodies that reuse `loaded`, `binding`, `out`, and imports from the Quick start unless the snippet shows its own setup.

## Reuse compiled Schemas and bindings

Compile and bind once, then reuse the binding for each input:

```rust
let loaded = SchemaLoader::new(TypolOptions::default())
    .compile_file(Path::new("service.tschem"), out.reborrow())?;
let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;

let first = binding.read_file(
    Path::new("service-a.tconf"),
    &mut environment,
    out.reborrow(),
)?;
let second = binding.read_file(
    Path::new("service-b.tconf"),
    &mut environment,
    out,
)?;
```

`compile_text(name, text, out)` is for Schemas without imports. Use `compile_file` with `std` for file imports, or `compile_with_resolver` when the application supplies its own source resolver. Compilation creates a fresh loading session; file contents are not cached across separate calls.

The read methods depend on which result the application needs:

| Call | Result |
| --- | --- |
| `BoundSchema::read` / `read_file` | A Rust value of type `T` |
| `BoundSchema::read_with_value` | `BoundRead<T>` containing both `T` and the validated Typol value tree |
| `LoadedSchema::read_value` | A validated Typol value tree without a Rust binding |
| `LoadedSchema::read_file` | A `ConfigDocument` retaining source text and value locations for editing |
| `LoadedSchema::read_file_value` | A validated Typol value tree from a file |

In particular, `BoundSchema::read_file` returns the typed Rust value. `LoadedSchema::read_file` has a different purpose: it retains the source representation so edits can preserve formatting.

## Generate `.tconf` from Schema defaults

`LoadedSchema` can build a complete configuration from the compiled Schema without a Rust binding or an `Environment` provider:

```rust
let text = loaded.default_config_to_string(
    DefaultConfigOptions::default().with_layout(TextLayout::Pretty),
    out.reborrow(),
)?;
```

The entry struct is treated as an empty root. For each field, generation uses its Schema default, then `none` for an `option<T>` without a default; any other missing field is an error with a field path and Schema location. A nested struct is not constructed just because all its children have defaults: the field itself needs a default such as `{}`. Arrays, maps, and enums likewise need an explicit field default to select a value. Once a value is selected, its missing child fields are completed normally, then constraints and full-tree assertions run.

Generation never evaluates `env(...)` or reads the process environment. `@env` only authorizes expressions in an input `.tconf`. `DefaultConfigOptions` controls compact or pretty output, the UTF-8 output byte limit, and whether to reject values marked `@secret`. The default permits secret values and writes their actual text; treat generated output as sensitive. When rejection is enabled, the entire value tree is checked before the sink receives its first byte.

The string method returns no partial string on failure. `write_default_config` can leave a prefix in a caller sink if encoding, a sink, or an output limit fails. To create a new file atomically, first generate the complete string and then commit its bytes with `FileSourceWriter` and `SourceExpectation::MustBeAbsent`. This succeeds only when the target does not already exist:

```rust
use typol::host::FileSourceWriter;
use typol::{SourceExpectation, SourceWriter};

let text = loaded.default_config_to_string(DefaultConfigOptions::default(), out.reborrow())?;
let mut writer = FileSourceWriter::new(4096);
let commit = writer.replace_atomically(
    Path::new("service.defaults.tconf"),
    SourceExpectation::MustBeAbsent,
    text.as_bytes(),
    1_048_576,
);
```

Handle `commit` according to the application's error policy. `LoadedSchema::write_file` edits a `ConfigDocument` with retained source; it is not the generated-text file API.

## Split a Schema across files

Use `extern mod` to import reusable declarations from another `.tschem` file:

```text
schemas/
├── application.tschem
└── network.tschem
```

In `application.tschem`, reference imported types through their module path:

```tschem
extern mod network;

struct WorkerPool {
    min_threads: u16 @range(1, 256) = 2,
    max_threads: u16 @range(1, 256) = 8,
} @assert(self.min_threads <= self.max_threads, "minimum threads exceed maximum")

entry struct ServiceConfig {
    endpoint: network::Endpoint,
    peers: [network::Endpoint] @size(min = 0, max = 8) @unique(by = "host"),
    worker_pool: WorkerPool = {},
}
@assert(
    !self.peers.any(|peer| peer.host == self.endpoint.host),
    "a peer cannot use the local host"
)
```

In `network.tschem`, define the imported type:

```tschem
mod address {
    type Host = string @host;
    type Port = u16 @range(1, 65535);
}

struct Endpoint {
    host: address::Host = "127.0.0.1",
    port: address::Port = 8080,
}
```

With `std`, `SchemaLoader::compile_file` uses the file resolver. It looks for `network.tschem` beside the root file and follows imports from imported files as well:

```rust
let loaded = SchemaLoader::new(TypolOptions::default())
    .compile_file(Path::new("schemas/application.tschem"), out.reborrow())?;
```

For memory or packaged sources, implement `SchemaResolver` and call `compile_with_resolver`. The resolver receives the requesting source identity and one module name at a time. The [Schema implementation](../../specification/schema-implementation/) describes the lower-level loading and compilation pipeline.

## Choose a validation rule

A field annotation checks one field or collection. A struct or enum `@assert` checks a completed value and can compare fields:

```tschem
struct RetryPolicy {
    delays_ms: [u32 @min(1)] @size(min = 1, max = 5)
        @all(item > 0, "delays must be positive")
        @sum(max = 60000),
    targets: [network::Endpoint] @unique(by = "host"),
}
```

The `@min(1)` annotation is attached to each `u32` item, while `@size` and `@all` apply to the array itself. An annotation applies only to the type expression it immediately follows, so nested arrays and map values can each have their own rules. `@all` checks every array item or map value. `@unique(by = "...")` checks a field path on an array of structs, and `@sum` applies to numeric arrays. Expressions are type-checked when the Schema is compiled. Struct and active enum-variant assertions run after the full configuration value is constructed; a parse or local validation failure prevents those assertions from running.

Enum restrictions compose through transparent aliases and apply to exact type nodes, including array elements:

```tschem
enum Transport { Tcp, Quic, Legacy }
type ModernTransport = Transport @variants(allow = ["Tcp", "Quic"]);
type TcpOnly = ModernTransport @variants(deny = ["Quic"]);
entry struct Proxy {
    transport: TcpOnly,
    history: [Transport @variants(deny = ["Legacy"])],
}
```

The alias allow list and use-site deny list both apply. A present `option<Enum>` value must pass its restriction; `none` passes. Schema compilation rejects unknown or repeated names, unsupported targets, and combinations that leave no enum variant available, including optional enums.

Regular expressions use search semantics, so any matching substring is sufficient. Add `^` and `$` when the whole string must match, for example `@regex("^[a-z][a-z0-9-]*$")`. Typol implements the documented regex subset, not every feature of a general-purpose regex engine. See the [Schema DSL reference](../../specification/schema-dsl/).

## Keep environment access explicit

A configuration may use `env("NAME")` only on a field that declares the same name with `@env`. The Schema is the allowlist:

```tschem
entry struct ServiceConfig {
    host: string @host @env("SERVICE_HOST") = "127.0.0.1",
}
```

```text
{
    host: env("SERVICE_HOST", "127.0.0.1"),
}
```

The fallback is parsed and validated as the field type. An unset variable uses the expression fallback when present. A set but invalid value is an error; Typol does not replace it with the fallback. Without an expression fallback, an unset variable follows the ordinary Schema default, optional-field, or required-field rule.

Every read receives an `Environment` provider. Use `EmptyEnvironment` when reads must not consult the host. With `std`, use the process adapter when process variables are intended:

```rust
let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
let config = binding.read_file(
    Path::new("service.tconf"),
    &mut environment,
    out,
)?;
```

Applications can implement `Environment` to read from a secret manager or another controlled source. The process adapter limits accepted variable name and value sizes; configuration limits separately bound the number of distinct names queried and the bytes cached for one read. See the [Config implementation](../../specification/config-implementation/) for lookup and caching behavior.

Keep a parsed reference with `Sourced<T>` when the application may write the value back. Its scalar shape still matches the Schema, and `Option<Sourced<T>>` retains the reference when the optional value is present:

```tschem
entry struct ServiceConfig {
    token: string @env("SERVICE_TOKEN"),
    optional_port: option<u16> @env("OPTIONAL_PORT"),
}
```

```text
{
    token: env("SERVICE_TOKEN", "local-token"),
    optional_port: env("OPTIONAL_PORT", 8080),
}
```

```rust
#[derive(Typol)]
struct ServiceConfig {
    token: Sourced<String>,
    optional_port: Option<Sourced<u16>>,
}

let binding = loaded.bind::<ServiceConfig>(out)?;
let config = binding.read(request, &mut environment, out)?;
assert_eq!(config.token.environment_name(), Some("SERVICE_TOKEN"));
let source = binding.write_to_string(&config, WriteOptions::default(), out)?;
```

The written source keeps `env("SERVICE_TOKEN", "local-token")`; it does not contain the provider's current token. Reading that source with another provider resolves the reference again, so the actual value may change while the name and typed fallback stay the same. `Sourced::from_environment` and `Sourced::from_environment_with_fallback` create explicit references without reading a provider; the writer checks their names and Schema authorization. Calling `into_value()` deliberately discards the reference, after which the ordinary scalar value is written. When a provider is unset and the expression fallback is selected, the resulting value has `ExplicitFallback` origin and is written as a scalar literal.

## Map Rust structs, enums, and recursive types

A Rust record mirrors a Schema struct. Field names and types must match; fields may remain private, and application methods can expose the intended API. Enum type and variant names and payload shapes must also match. Derive supports unit, single-payload, and named-field variants. For example, this Schema:

```tschem
enum Storage {
    Memory,
    Local(string),
    ObjectStore {
        bucket: string,
        region: string,
    },
}
```

can map to:

```rust
#[derive(Typol)]
enum Storage {
    Memory,
    Local(String),
    ObjectStore {
        bucket: String,
        region: String,
    },
}
```

Arrays map to `Vec<T>` or fixed-size `[T; N]` values. `option<T>` maps to `Option<T>`. A `map<string, T>` can use `HashMap<String, T>` with `std`, `OrderedMap<T>` without `std`, or the optional map integrations listed in the [Schema implementation](../../specification/schema-implementation/).

Recursive Schema types are valid when they have a finite value. For example, an empty child list ends a finite tree:

```tschem
struct Node {
    value: string,
    children: [Node],
}

entry struct TreeConfig {
    root: Node,
}
```

```rust
#[derive(Typol)]
struct Node {
    value: String,
    children: Vec<Node>,
}
```

A required cycle with no finite branch is rejected at Schema compilation. Recursive values remain subject to depth, node, execution, binding, and conversion limits.

For ordinary records and enums, use `#[derive(Typol)]` and the normal `bind` method. A handwritten `Typol` implementation is application code and is not covered by all library-managed limits. If a derived type contains a handwritten implementation, mark it `#[typol(trusted)]` and bind it explicitly:

```rust
#[derive(Typol)]
#[typol(trusted)]
struct ServiceConfig {
    account_id: LegacyAccountId,
    port: u16,
}

let binding = loaded.bind_trusted::<ServiceConfig>(out.reborrow())?;
```

`LegacyAccountId` must provide its own `Typol` implementation. Typol validates the input before conversion, but does not validate the Rust value returned by handwritten code. The callback can allocate, block, or do arbitrary work outside the library's budgets. Use this path only for reviewed mappings; it is not a sandbox.

## Edit configuration while preserving its source

Use `LoadedSchema::read_file` when an editor or application needs to keep comments, whitespace, and source locations:

```rust
let mut document = loaded.read_file(
    Path::new("service.tconf"),
    &mut environment,
    out,
)?;
```

Apply a batch of typed replacements, insertions, or removals with `ConfigDocument::apply_edits` and `ConfigPath`. Array indexes in one batch refer to the original document, and overlapping edits are rejected; a batch can remove the last item and append a new one at the same time. Patches are applied to a candidate and the result is parsed and validated before commit, so failures leave the document unchanged. Use `ConfigEditOptions::default()` to keep the compact encoding, or select `TextLayout::Pretty` to lay out newly encoded values. Pretty affects only the inserted or replaced value fragment. It leaves surrounding source and untouched entries byte-for-byte intact; insertion boundaries still follow the container's local spacing and indentation. Pretty uses the document-wide newline rule (CRLF if the source contains CRLF, otherwise LF), four additional spaces per encoded container, trailing commas on nonempty containers, and multiline parentheses for single-value enum payloads. Empty containers, unit variants, and `env(...)` remain inline. `remove` keeps its existing signature because it does not encode a value.

```rust
use typol::{ConfigEditOptions, ConfigFieldInsertion, ConfigReplacement, TextLayout};

let compact = ConfigEditOptions::default();
document.set_value(
    ConfigReplacement::new(&path, &compact_value),
    compact,
    &mut environment,
    out,
)?;

let pretty = ConfigEditOptions::new().with_layout(TextLayout::Pretty);
document.apply_edits(&[edit], pretty, &mut environment, out)?;
document.insert_field(
    ConfigFieldInsertion::new(&field_path, &field_value),
    pretty,
    &mut environment,
    out,
)?;

// Removal does not need a layout option.
document.remove(&path, &mut environment, out)?;
```

`replace_values`, `apply_edits`, `set_value`, and `insert_field` take the options before the environment argument. `set_value` accepts a `ConfigReplacement`, while `insert_field` accepts a `ConfigFieldInsertion`, so each convenience method groups its path and value in one named argument. A batch has one layout for all values in that batch. If the application only needs a validated value, use `read_file_value` or a bound schema's `read_file`.

Writing a document back is a separate source operation. The standard filesystem adapter cannot atomically compare the retained source snapshot and replace a path, so `write_file` reports `AtomicReplaceUnsupported` for that expectation. Use `write_file_with_expectation` with `MustBeAbsent` for a new path or `ReplaceAny` when overwriting is intentional. A custom target can implement `SourceWriter` with its own atomic replacement behavior.

## Write a typed Rust value

A bound schema validates the complete value against the Schema before serializing it. Import `WriteOptions` and `TextLayout` from `typol`. The method call order is `write(&value, sink, options, out)`; `WriteOptions` comes after the output sink:

```rust
let mut text = String::new();
binding.write(
    &config,
    &mut text,
    WriteOptions::default().with_layout(TextLayout::Pretty),
    out,
)?;
```

Pretty output uses four-space indentation, groups single-value enum payloads across lines, and ends with LF; default output is compact. The layout matches `config fmt`, so pretty writer output can pass `config fmt --check` directly. Formatting whitespace counts toward the output limit. Validation and conversion finish before the first byte is written. A sink or output-limit error can leave an already-written prefix. Values marked `@secret` are still included in the output.

`TextLayout` replaces the former pretty boolean option. Migrate `WriteOptions::with_pretty(true)` and `DefaultConfigOptions::with_pretty(true)` to `.with_layout(TextLayout::Pretty)`; use `.layout()` to inspect the selected mode. This API change also changes pretty indentation from two to four spaces, adds a final LF, and expands single-value enum payload parentheses.

Use `write_to_string` when a buffered `String` is useful. The `write_serde` methods use the same options when the `serde` feature is enabled.

## Use the Schema-aware Serde bridge

Enable Typol's `serde` feature and Serde's derive feature:

```toml
[dependencies]
typol = { version = "0.1", features = ["std", "serde"] }
serde = { version = "1", features = ["derive"] }
```

`read_serde` and `write_serde` use the successfully bound Schema to validate Serde data-model events. Field names, types, and enum payloads must match the Schema. Ordinary calls to Serde's `Serialize` or `Deserialize` traits do not carry a Typol Schema context and do not promise Schema validation. Custom Serde implementations remain caller code and have their own trust boundary. The [Schema implementation](../../specification/schema-implementation/) lists supported Serde data-model operations.

## Export typed JSON with Config IR

Enable `config-ir` to serialize a validated value as Config IR v1. It also enables `serde` and can be used with `no_std + alloc`:

```toml
[dependencies]
typol = { version = "0.1", features = ["config-ir"] }
```

`ConfigIr::serialize` fills Schema defaults and fully validates a raw `ConfigValue` before writing. If a parse/load call already returned `ValidatedConfigValue`, pass it by reference to `ConfigIr::serialize_validated` to avoid a second validation walk; the credential is accepted only by the exact `CompiledSchema` that produced it.

Map entries and struct fields have deterministic output order. Integers are JSON strings so the full `i128` and `u128` ranges are preserved. A node marked `"secret": true` still contains its value; the marker is metadata, not encryption or redaction. Treat that node and its subtree as sensitive. See the [Config IR v1 reference](../../specification/config-ir/).

## Set limits and handle sensitive values

`TypolOptions` configures Schema, configuration, binding, file, conversion, and execution policies. Import `ConfigLimits` and `TypolOptions` with `use typol::{config::ConfigLimits, TypolOptions};`. Tune the relevant limits for expected inputs, for example:

```rust
let options = TypolOptions::default().with_config_limits(
    ConfigLimits::default()
        .with_max_source_bytes(256 * 1024)
        .with_max_nesting_depth(64)
        .with_max_diagnostics(32),
);
```

These limits cover library-managed inputs and workspaces. They are not operating-system CPU, total process-memory, or real-time limits. Handwritten `Typol` and custom Serde callbacks remain user-code boundaries.

`@secret` protects diagnostic and safe-display output, including paths below a secret value. A successful Rust value and Config IR still contain actual secret contents. Applications must protect their own logs, serialized output, and storage.

## Next steps

- See `examples/showcase` in the repository for a complete multi-file application.
- Use the [CLI guide](../cli/) to validate or format files, or export a Config IR document.
- Read the [Schema DSL](../../specification/schema-dsl/) and [Config DSL](../../specification/config-dsl/) for normative language behavior.
- Consult the [Schema implementation](../../specification/schema-implementation/), [Config implementation](../../specification/config-implementation/), and [Config IR v1 reference](../../specification/config-ir/) for detailed contracts.
