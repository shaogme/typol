---
title: no_std
description: Use Typol in no_std Rust applications with alloc and explicit host providers.
sidebar:
  order: 2
---

Typol's facade is `no_std` compatible by default. It uses `alloc` for owned values, so this is a fit for applications that avoid Rust's standard library but still provide an allocator. For a file-based application using the standard library, start with the [quick start](../quickstart/).

## Choose features

The `typol` crate has no default features. Make that choice explicit in a `no_std` project's `Cargo.toml`:

```toml
[dependencies]
typol = { version = "0.1", default-features = false }
```

| Feature | What it adds | Available without `std`? |
| --- | --- | --- |
| *(none)* | In-memory Schema loading, text or byte configuration input, typed bindings, and explicit `Environment` providers | Yes |
| `serde` | Schema-aware Serde bridge; Typol enables Serde's `alloc` support | Yes |
| `std` | File and reader inputs, file Schema loading, process environment access, and host diagnostic rendering | No |
| `config-ir` | Config IR v1 JSON serialization using Serde and `alloc` | Yes |

The facade remains `no_std` internally when the `std` feature is enabled; `std` adds host adapters to the API. A `no_std` executable must also provide its platform runtime and global allocator. Typol does not provide either.

With `config-ir`, callers can serialize an in-memory `ConfigValue` without `std`. The result is an owned `alloc::vec::Vec<u8>`:

```rust
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use typol::{
    ConfigIr, ConfigIrError, ConfigIrOptions,
    core::ConfigValue, semantic::CompiledSchema,
};

fn export_ir(
    schema: &CompiledSchema,
    value: ConfigValue,
) -> Result<Vec<u8>, ConfigIrError> {
    ConfigIr::serialize(schema, value, ConfigIrOptions::default())
}
```

The Schema and value can come from the in-memory loading APIs described below. The target still needs an allocator for owned values and the serialized output.

## Load text in memory

This library-style example uses only APIs available without `std`. It compiles a Schema without imports, binds it to an owned Rust type, and parses configuration text:

```rust
#![no_std]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use typol::{
    ConfigInput, EmptyEnvironment, SchemaLoader, Typol, TypolError, TypolOptions,
    opeo::{ErrSlot, OResult, Out},
};

const SCHEMA_TEXT: &str = r#"
entry struct ServiceConfig {
    name: string,
    ports: [u16],
}
"#;

const CONFIG_TEXT: &str = r#"{ name: "edge", ports: [8080, 8081] }"#;

#[derive(Typol)]
struct ServiceConfig {
    name: String,
    ports: Vec<u16>,
}

fn read_config<'slot>(out: Out<'slot, '_, TypolError>) -> OResult<'slot, ServiceConfig> {
    let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
        b"embedded-schema",
        SCHEMA_TEXT,
        out.reborrow(),
    )?;
    let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
    let mut environment = EmptyEnvironment;
    binding.read(ConfigInput::text(CONFIG_TEXT), &mut environment, out)
}

fn load_service() -> Result<ServiceConfig, TypolError> {
    let mut errors = ErrSlot::<TypolError>::new();
    match errors.try_call(|out| read_config(out)) {
        Ok(config) => Ok(config),
        Err(caught) => Err(caught.take()),
    }
}
```

The caller supplies the OPEO error context (`Out`). `load_service` uses `opeo::ErrSlot` to convert the operation's `OResult` into core `Result` while keeping the `TypolError`. In a no_std build, `TypolError` exposes its stage and structured schema, configuration, budget, or codec errors. Its `Display` text is a short stage summary; rich host diagnostic rendering is part of the `std` adapter.

## Provide sources and environment access

The in-memory APIs keep host access in the application:

- `SchemaLoader::compile_text` compiles a Schema that has no `extern mod` imports. To resolve imports from memory or packaged resources, implement `SchemaResolver` and pass it to `SchemaLoader::compile_with_resolver`. The resolver decides how those sources are obtained; Typol does not open files in this path.
- `ConfigInput::text` reads a string, and `ConfigInput::bytes` reads through a caller-provided `ByteSource`. Both `LoadedSchema::read_value` and `BoundSchema::read` receive an explicit `Environment` provider.
- `EmptyEnvironment` is suitable when the configuration does not need environment expressions. For `@env` expressions, implement `Environment` to read from the application's chosen source. The no_std build does not read process variables automatically.

With `std`, the same loader and read methods can also use `compile_file`, `ConfigInput::file`, `ConfigInput::reader`, and `ProcessEnvironment`. The [Config implementation](../../specification/config-implementation/) documents the shared and host-specific inputs.

## Account for collection bindings

Owned strings and vectors use `alloc`. The built-in `HashMap<String, T>` binding is available only with `std`; Typol does not provide a built-in `BTreeMap` binding because its node insertion cannot report allocation failure. Schema-level maps can still be parsed into Typol's configuration value model, but binding them to those Rust collections is not supported by the built-in mappings. See the [Schema implementation](../../specification/schema-implementation/) when choosing application types.

The library's resource limits account for operations and storage that Typol manages. They do not replace the target's allocator policy or bound all memory used by the application and its providers.

## Next steps

- Follow the [quick start](../quickstart/) for the file-based `std` workflow.
- Read the [advanced tutorial](../advanced/) for imports, custom validation, Serde, and Config IR.
- See the [Schema implementation](../../specification/schema-implementation/) and [Config implementation](../../specification/config-implementation/) for the lower-level APIs.
