---
title: Quick start
description: Learn Typol's Schema types, Rust mappings, core methods, and configuration read flow.
sidebar:
  order: 1
---

This guide builds a small Rust program that reads a Schema and a configuration file. It introduces the three pieces involved:

1. A `.tschem` Schema declares data types and validation rules.
2. A `.tconf` file provides values for the Schema's entry type.
3. Rust structs and enums represent the validated values in the application.

The usual call order is `SchemaLoader → LoadedSchema → BoundSchema<T> → T`. Each step has a distinct job: compile, bind, then read.

## Set up the project

Typol requires Rust 1.85 or newer. File loading and the process-environment adapter need the `std` feature:

```toml
[dependencies]
typol = { version = "0.1", features = ["std"] }
```

Create a project with `cargo new service-app`, then add these files:

```text
service-app/
├── Cargo.toml
├── service.tschem
├── service.tconf
└── src/
    └── main.rs
```

For an application that does not use the standard library, follow the [no_std guide](../no_std/).

## 1. Declare Schema types

Save this in `service.tschem`:

```tschem
struct Endpoint {
    host: string @host,
    port: u16 @range(1, 65535) = 8080,
}

entry struct ServiceConfig {
    service_name: string @length(1, 64),
    endpoint: Endpoint,
    tags: [string] @size(min = 1, max = 8) = ["api"],
    enabled: bool = true,
    note: option<string>,
}
```

A Schema `struct` groups named fields. Each field has a type and may also have annotations or a default value. `entry` marks the one struct that is the root of a configuration.

Here, `ServiceConfig` is the entry. The `endpoint` field is another declared type. `service_name` is required; `endpoint.port`, `tags`, and `enabled` have defaults; `note` is optional. The Schema validates defaults as well as values supplied in a configuration.

Common Schema types are:

| Schema type | Meaning |
| --- | --- |
| `bool`, `string`, `i8`…`i128`, `u8`…`u128`, `f32`, `f64` | Boolean, string, integer, and finite floating-point scalar |
| `option<T>` | An optional `T`; its empty value is `none` |
| `[T]` | A variable-length array |
| `[T; N]` | An array with exactly `N` elements |
| `map<string, T>` | A map with string keys |
| `struct`, `enum` | User-declared record and choice types |

Annotations such as `@range`, `@length`, and `@size` add validation rules. The [Schema DSL reference](../../specification/schema-dsl/) lists supported types and annotations.

## 2. Provide a configuration value

Save this in `service.tconf`:

```text
{
    service_name: "edge",
    endpoint: {
        host: "edge.internal",
        port: 8443,
    },
}
```

Configuration keys and values follow the Schema. Missing fields use their defaults; an omitted optional field becomes `none`. A supplied value must still match its declared type and constraints. For example, `port: 0` fails the `@range(1, 65535)` rule.

## 3. Map Schema types to Rust

Declare Rust types with the same names and matching fields:

```rust
use typol::Typol;

#[derive(Debug, Typol)]
struct Endpoint {
    host: String,
    port: u16,
}

#[derive(Debug, Typol)]
struct ServiceConfig {
    service_name: String,
    endpoint: Endpoint,
    tags: Vec<String>,
    enabled: bool,
    note: Option<String>,
}

impl ServiceConfig {
    fn service_name(&self) -> &str {
        &self.service_name
    }

    fn port(&self) -> u16 {
        self.endpoint.port
    }
}
```

The derive macro generates the conversion between each Rust type and its Schema type. The Rust type name, field names and types, enum variant names, and payload shapes must correspond to the Schema declarations; binding checks that mapping. Derive does not generate accessors: methods in an `impl` block are ordinary Rust methods chosen by the application. For example, `port(&self)` reads a private field while keeping that field hidden from other modules.

Common mappings are:

| Schema type | Rust type |
| --- | --- |
| `bool`, `string`, integer/float scalar | `bool`, `String`, same-width integer/float |
| `option<T>` | `Option<T>` |
| `[T]` | `Vec<T>` |
| `[T; N]` | `[T; N]` |
| `map<string, T>` | `HashMap<String, T>` with `std`; `OrderedMap<T>` without it |
| `struct Name` / `enum Name` | Rust `struct Name` / `enum Name` deriving `Typol` |

A derived Rust struct must include every Schema field, even when the configuration can omit a field due to a default or `option`. In Rust, an optional Schema field is still represented by an `Option<T>` field on the struct. Schema enum variants can be unit, single-payload, or named-field variants; see the [advanced tutorial](../advanced/) for examples.

## 4. Compile, bind, and read

Put this in `src/main.rs`. The example compiles the Schema file, binds it to `ServiceConfig`, then reads and validates the configuration file:

```rust
use std::{path::Path, process::ExitCode};

use typol::{
    DefaultConfigOptions, EmptyEnvironment, SchemaLoader, Typol, TypolError, TypolOptions,
    opeo::ErrSlot,
};

#[derive(Debug, Typol)]
struct Endpoint {
    host: String,
    port: u16,
}

#[derive(Debug, Typol)]
struct ServiceConfig {
    service_name: String,
    endpoint: Endpoint,
    tags: Vec<String>,
    enabled: bool,
    note: Option<String>,
}

impl ServiceConfig {
    fn service_name(&self) -> &str {
        &self.service_name
    }

    fn port(&self) -> u16 {
        self.endpoint.port
    }
}

fn main() -> ExitCode {
    let mut errors = ErrSlot::<TypolError>::new();
    match errors.try_call(|mut out| {
        let loader = SchemaLoader::new(TypolOptions::default());
        let loaded = loader.compile_file(Path::new("service.tschem"), out.reborrow())?;
        let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
        let mut environment = EmptyEnvironment;
        let config = binding.read_file(
            Path::new("service.tconf"),
            &mut environment,
            out,
        )?;

        println!("{} listens on port {}", config.service_name(), config.port());
        Ok(())
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(caught) => {
            eprintln!("{}", caught.get());
            ExitCode::FAILURE
        }
    }
}
```

Run it from the project directory:

```sh
cargo run
```

The methods in this path are:

| Call | What it does |
| --- | --- |
| `SchemaLoader::new(options)` | Creates a loader with the selected limits and policies. |
| `loader.compile_file(path, out)` | Loads and compiles a Schema file. This file API requires `std`. For an in-memory Schema, use `compile_text(name, text, out)`. |
| `loaded.bind::<ServiceConfig>(out)` | Checks the Rust type against the Schema entry and prepares a reusable binding. |
| `binding.read_file(path, environment, out)` | Reads, applies defaults, validates, and converts one file to `ServiceConfig`. |

`SchemaLoader::new` is an associated function: it is called on the type. `compile_file`, `bind`, and `read_file` are methods called on values. Fallible methods take an `Out` error context and return an OPEO result; `ErrSlot::try_call` provides that context and captures the resulting `TypolError`.

If you need a starter `.tconf` without creating a Rust struct, generate it directly from the loaded Schema:

```rust
let defaults = loaded.default_config_to_string(
    DefaultConfigOptions::default().with_pretty(true),
    out.reborrow(),
)?;
```

The entry's required fields need explicit Schema defaults. An omitted `option<T>` becomes `none`; nested structs, arrays, maps, and enum variants are generated only when their field has an explicit default. Generation does not read `@env` values. The default policy includes actual `@secret` values in the text, so protect the result as sensitive data. See the [advanced tutorial](../advanced/) for output limits, secret rejection, and sink behavior.

A `LoadedSchema` and its binding can be kept and reused to read many configurations. The binding checks the Rust type once; each read still parses and validates its input. If you need Typol's validated value tree instead of a Rust type, use `LoadedSchema::read_value`.

The file methods in this example require `std`. For in-memory input, `BoundSchema::read` accepts a `ReadRequest` wrapping a `Read` source, such as `SliceSource`; `ReadRequest::from_text` is a shortcut for text. The [no_std guide](../no_std/) shows that form in a complete program.

## Read environment variables only when allowed

Environment access is explicit. To permit `host` to use `SERVICE_HOST`, add the same name to the Schema field's `@env` annotation:

```tschem
host: string @host @env("SERVICE_HOST") = "127.0.0.1",
```

Then the configuration can contain:

```text
{
    service_name: "edge",
    endpoint: {
        host: env("SERVICE_HOST", "127.0.0.1"),
        port: 8443,
    },
}
```

Replace `EmptyEnvironment` with `ProcessEnvironment` and `ProcessEnvironmentLimits` in the `typol` import, then create the provider:

```rust
let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
```

The fallback is used when the variable is unset. A set value is parsed as the field's declared type and still has to pass its constraints. Only a field declaring the matching `@env` name can use that environment expression. The [advanced tutorial](../advanced/) explains custom providers and failure behavior.

## Next steps

- Read the [Schema DSL reference](../../specification/schema-dsl/) for type and validation rules.
- Read the [Config DSL reference](../../specification/config-dsl/) for values, enums, maps, and environment expressions.
- Use the [CLI guide](../cli/) to check files without writing a Rust application.
- Follow the [advanced tutorial](../advanced/) for imported Schemas, richer validation, Serde, and Config IR.
- See the [no_std guide](../no_std/) for in-memory inputs without the standard library.
