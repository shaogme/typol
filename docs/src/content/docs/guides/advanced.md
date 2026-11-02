---
title: Advanced tutorial
description: Use imported Schemas, cross-field validation, explicit environment access, trusted Rust mappings, Serde, and Config IR.
sidebar:
  order: 3
---

This tutorial builds on the [quick start](../quickstart/). It covers integration choices for applications with reusable Schema files, richer validation, and Rust or JSON boundaries. Examples that use files or process variables require Typol's `std` feature. For the in-memory `no_std + alloc` path, see the [no_std guide](../no_std/).

## Split a Schema across files

Use `extern mod` to import declarations from another `.tschem` file. For example, keep reusable network types beside the application Schema:

```text
schemas/
├── application.tschem
└── network.tschem
```

In `application.tschem`, declare the imported module and refer to its types by module path:

```text
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

In `network.tschem`, define the imported types:

```text
mod address {
    type Host = string @host;
    type Port = u16 @range(1, 65535);
}

struct Endpoint {
    host: address::Host = "127.0.0.1",
    port: address::Port = 8080,
}
```

`SchemaLoader::compile_text` accepts schemas without imports. With `std`, `compile_file` uses the file resolver, which looks for `network.tschem` beside `application.tschem` (and supports a directory named after the root file as an alternate layout):

```rust
let loaded = SchemaLoader::new(TypolOptions::default())
    .compile_file(Path::new("schemas/application.tschem"), out.reborrow())?;
```

The file resolver follows imports from imported files as well. `LoadedSchema` owns the compiled Schema and can be reused for multiple configuration reads. Each `compile_file` call starts a new loading session; file contents are not cached across calls. For in-memory or packaged sources, implement `SchemaResolver` and call `compile_with_resolver`; the resolver receives the requesting source identity and one module name at a time. See the [Schema implementation](../../specification/schema-implementation/) for the lower-level loading model.

## Validate relationships between values

Field annotations validate one value or collection. `@assert` checks a completed struct or selected enum variant, so it can express relationships between fields. In the example above, the worker pool checks its two bounds together, and the entry assertion checks each peer against the local endpoint.

Other useful collection checks include:

```text
struct RetryPolicy {
    delays_ms: [u32] @size(min = 1, max = 5) @all(item > 0, "delays must be positive") @sum(max = 60000),
    targets: [network::Endpoint] @unique(by = "host"),
}
```

Expressions are type-checked when the Schema is compiled. `@all` checks every array item or map value; `@unique(by = "...")` checks a field path on an array of structs; `@sum` applies to numeric arrays. Assertions run after the whole configuration has been parsed and locally validated. A failed parse or local constraint prevents root assertions from running.

Regular expressions use search semantics: a pattern succeeds when a substring matches. Add `^` and `$` when the entire string must match, for example `@regex("^[a-z][a-z0-9-]*$")`. Typol supports a documented regex subset rather than every feature of a general-purpose regex engine; see the [Schema DSL specification](../../specification/schema-dsl/).

## Keep environment access explicit

An `env("NAME")` expression is allowed only when the target field declares the same name with `@env`. The Schema is the allowlist; configuration text does not get arbitrary access to process variables.

```text
entry struct ServiceConfig {
    host: string @host @env("SERVICE_HOST") = "127.0.0.1",
}
```

```text
{
    host: env("SERVICE_HOST", "127.0.0.1"),
}
```

The fallback is parsed and validated against the field type. An unset variable selects the fallback when one is present; a set but invalid value is an error and does not select the fallback. Without an expression fallback, an unset variable follows the usual Schema default, optional, or required-field rule.

Reads always receive an `Environment` provider. Use `EmptyEnvironment` for deterministic reads that should not consult the host, or use the `std` adapter when process access is intended:

```rust
let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
let config = binding.read(input, &mut environment, out)?;
```

`ProcessEnvironmentLimits` caps environment-name and value bytes accepted by the process adapter. `ConfigLimits` separately caps the number of distinct names read and the total bytes cached during one configuration read. Applications can implement `Environment` to read from a secret manager or another controlled source. The [Config implementation](../../specification/config-implementation/) describes lookup, caching, and fallback behavior.

## Bind custom Rust types and use Serde

For ordinary Rust records and enums, prefer `#[derive(Typol)]`. The normal `bind` path accepts built-in or derive-generated implementations whose nested fields and enum payloads follow the same rule.

When a type contains a handwritten `Typol` implementation, mark the containing derive as trusted and call `bind_trusted`:

```rust
#[derive(Typol)]
#[typol(trusted)]
struct ServiceConfig {
    account_id: LegacyAccountId,
    port: u16,
}

let binding = loaded.bind_trusted::<ServiceConfig>(out.reborrow())?;
```

`LegacyAccountId` must have its own handwritten `Typol` implementation. Such callbacks are application code: Typol budgets cover work routed through `ConversionContext`, but cannot constrain arbitrary computation, blocking, or direct allocations in the callback. Input is validated against the Schema before conversion; the returned Rust value is not validated again. Keep handwritten implementations small and reviewed. The API contract is documented in the [Schema implementation](../../specification/schema-implementation/).

The optional Serde bridge can read or write Serde types with the bound Schema in context. Enable both Typol's `serde` feature and Serde's derive feature:

```toml
[dependencies]
typol = { version = "0.1", features = ["std", "serde"] }
serde = { version = "1", features = ["derive"] }
```

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Typol)]
struct ServiceConfig {
    host: String,
    port: u16,
}

let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
let config = binding.read_serde(input, &mut environment, out)?;
```

The Schema field names and types must match the Serde data model. `read_serde` validates the parsed value before constructing the target. Ordinary calls to `Serialize` or `Deserialize` without a `TypedSchema` bridge have no Typol Schema context or validation guarantee. Custom Serde implementations are also caller code and have their own trust boundary; consult the [Schema implementation](../../specification/schema-implementation/) for the supported Serde data model.

## Export typed JSON with Config IR

Enable `config-ir` to serialize a `ConfigValue` as Config IR v1. It works with `no_std + alloc` and does not enable `std`:

```toml
[dependencies]
typol = { version = "0.1", features = ["config-ir"] }
```

The default mode fills Schema defaults and performs full validation before writing JSON. A small helper for a value already obtained from a validated read is:

```rust
use std::vec::Vec;
use typol::{
    core::ConfigValue, semantic::CompiledSchema,
    ConfigIr, ConfigIrError, ConfigIrOptions,
};

fn export_ir(
    schema: &CompiledSchema,
    value: ConfigValue,
) -> Result<Vec<u8>, ConfigIrError> {
    ConfigIr::serialize(schema, value, ConfigIrOptions::new().with_pretty(true))
}
```

The same API is available in a `no_std` application with an allocator; import the output type from `alloc::vec::Vec`. In-memory Schema loading and value reading work without host file or environment adapters.

Pass `validated.into_config_value()` from `LoadedSchema::read_value` as the helper's value. To deliberately export without default filling or constraint validation, select `ConfigIrValueMode::AsProvided` with `ConfigIrOptions::with_value_mode`. Map entries and struct fields have deterministic output order, and integer values are JSON strings so full `i128` and `u128` ranges are preserved.

An IR node marked `"secret": true` still contains the actual value. Treat the node and its subtree as sensitive; the marker is metadata, not encryption or redaction. See the [Config IR v1 specification](../../specification/config-ir/).

## Set limits and handle diagnostics

Tune configuration limits for the expected input instead of relying on a larger host-wide budget. For example, these settings cap each source, nesting depth, and collected diagnostics:

```rust
use typol::{TypolOptions, config::ConfigLimits};

let options = TypolOptions::default().with_config_limits(
    ConfigLimits::default()
        .with_max_source_bytes(256 * 1024)
        .with_max_nesting_depth(64)
        .with_max_diagnostics(32),
);
```

`TypolOptions` also accepts schema, binding, file, conversion, and execution policies. Keep in mind that these limits cover library-managed input and workspaces; they are not operating-system CPU, total process-memory, or real-time limits. Handwritten `Typol` and Serde callbacks remain user-code boundaries.

Return or display `TypolError` through its diagnostic interface rather than logging raw configuration values. `@secret` marks values for safe diagnostics and display, including paths below the secret value. Successful Rust values and Config IR still carry the actual secret, so application logging and storage need their own controls.

## Next steps

- Follow the `examples/showcase` files in the repository for a complete multi-file application Schema and configuration.
- See the [CLI guide](../cli/) to check files or export validated configuration values from the command line.
- Read the [Schema DSL](../../specification/schema-dsl/) and [Config DSL](../../specification/config-dsl/) for normative language behavior.
- See the [Schema implementation](../../specification/schema-implementation/), [Config implementation](../../specification/config-implementation/), and [Config IR v1 specification](../../specification/config-ir/) for detailed API and format contracts.
