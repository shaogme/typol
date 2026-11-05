---
title: Quick start
description: Build a small typed configuration loader with Typol.
sidebar:
  order: 1
---

This guide creates a Rust program that reads a `.tschem` schema and a `.tconf` file, validates the configuration, and returns a Rust struct.

## Add Typol to a Rust project

Typol requires Rust 1.85 or newer. In your application's `Cargo.toml`, add the facade crate with its `std` feature. This feature enables file inputs and host adapters. For applications that do not use the standard library, see the [no_std guide](../no_std/).

```toml
[dependencies]
typol = { version = "0.1", features = ["std"] }
```

Create these files in the project root:

```text
service-app/
├── Cargo.toml
├── service.tschem
├── service.tconf
└── src/
    └── main.rs
```

For a new project, run `cargo new service-app` and then `cd service-app` before adding the Schema and configuration files.

## Describe the configuration in a Schema

Put this in `service.tschem`:

```text
entry struct ServiceConfig {
    service_name: string @length(1, 64),
    host: string @host = "127.0.0.1",
    port: u16 @range(1, 65535) = 8080,
    tags: [string] @size(min = 1, max = 8) = ["api"],
    enabled: bool = true,
    note: option<string>,
}
```

`entry` marks the root configuration type. `service_name` is required and must contain 1–64 characters. The other fields show scalar types, a list, constraints, and defaults. Because `note` is optional and has no default, omitting it produces `none`.

## Write a configuration file

Put this in `service.tconf`:

```text
{
    service_name: "edge",
    host: "edge.internal",
}
```

The configuration uses the Schema's field names and types. Omitted fields receive their Schema defaults; an omitted optional field becomes `none`. A supplied value still has to satisfy its type and constraints. For example, setting `port` to `0` fails its `@range` check.

## Bind the Schema to Rust and read the file

Put this in `src/main.rs`:

```rust
use std::{path::Path, process::ExitCode};

use typol::{
    EmptyEnvironment, SchemaLoader, Typol, TypolError, TypolOptions,
    host::{DiagnosticRenderError, Language, RenderOptions, Translator},
    opeo::ErrSlot,
};

#[derive(Debug, Typol)]
struct ServiceConfig {
    service_name: String,
    host: String,
    port: u16,
    tags: Vec<String>,
    enabled: bool,
    note: Option<String>,
}

fn main() -> ExitCode {
    let mut errors = ErrSlot::<TypolError>::new();
    match errors.try_call(|mut out| {
        let loaded = SchemaLoader::new(TypolOptions::default())
            .compile_file(Path::new("service.tschem"), out.reborrow())?;
        let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
        let mut environment = EmptyEnvironment;
        let config = binding.read_file(Path::new("service.tconf"), &mut environment, out)?;

        println!("{config:#?}");
        Ok(())
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(caught) => {
            let mut render_errors = ErrSlot::<DiagnosticRenderError>::new();
            match render_errors.call(|out| {
                caught.get().render(
                    Translator::new(Language::English),
                    RenderOptions::default(),
                    out,
                )
            }) {
                Ok(diagnostics) => {
                    eprintln!("{diagnostics}");
                    ExitCode::FAILURE
                }
                Err(render_error) => {
                    eprintln!("Diagnostic rendering failed: {render_error}");
                    ExitCode::from(3)
                }
            }
        }
    }
}
```

Run the program from the project root:

```sh
cargo run
```

`#[derive(Typol)]` generates the mapping between the Schema's `ServiceConfig` and the Rust struct. `bind` checks that the declared fields and types match. `read` loads and validates the configuration before returning the typed value. The example uses `EmptyEnvironment` so its result does not depend on the machine's environment.

With `std` enabled, `TypolError::render` renders library diagnostics using the `Translator` you pass. Its result distinguishes a Typol operation error from a diagnostic rendering or translation error, so an application can handle rendering failure separately.

For this input, the returned struct has `port: 8080`, `tags: ["api"]`, `enabled: true`, and `note: None`; those values came from defaults or the optional-field rule.

## Read environment variables when needed

Environment access is explicit. To allow the `host` field to read `SERVICE_HOST`, update its Schema declaration to include `@env("SERVICE_HOST")`:

```text
host: string @host @env("SERVICE_HOST") = "127.0.0.1",
```

Then write the configuration file as:

```text
{
    service_name: "edge",
    host: env("SERVICE_HOST", "127.0.0.1"),
}
```

Then replace `EmptyEnvironment` in the top-level `typol` import with `ProcessEnvironment` and `ProcessEnvironmentLimits`, and replace the provider initialization:

```rust
let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
```

The expression's fallback is used when the variable is unset. A set value is parsed as the field's declared type and still has to pass its constraints. Only fields authorized with the matching `@env` annotation can use that expression.

## Next steps

- Read the [Schema DSL specification](../../specification/schema-dsl/) for types, defaults, constraints, and assertions.
- Read the [Config DSL specification](../../specification/config-dsl/) for configuration values, enums, maps, and environment expressions.
- Use the [CLI guide](../cli/) to check Schema and configuration files or export Config IR without writing a Rust application.
- See the [no_std guide](../no_std/) for in-memory loading and explicit providers without the standard library.
- Try the [advanced tutorial](../advanced/) for imports, trusted handwritten mappings, resource limits, and Config IR.
