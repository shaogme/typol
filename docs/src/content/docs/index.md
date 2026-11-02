---
title: Typol Documentation
description: Bilingual documentation for Typol, a schema-driven configuration library for Rust.
template: splash
hero:
  tagline: A schema-driven configuration library for Rust.
  actions:
    - text: Quick start
      link: guides/quickstart/
      icon: right-arrow
    - text: CLI guide
      link: guides/cli/
      variant: minimal
    - text: Explore specifications
      link: specification/schema-dsl/
      variant: minimal
---

## Documentation

Typol compiles `.tschem` schemas, parses `.tconf` values against the compiled types, validates constraints and assertions, and can bind the result to Rust types with `#[derive(Typol)]`.

The guides introduce Rust and CLI workflows. The specifications below describe the DSL contracts, their current implementations, the Config IR export format, and the supported regular-expression engine. Choose **简体中文** in the language selector for the Chinese edition.

- [Quick start guide](guides/quickstart/)
- [no_std guide](guides/no_std/)
- [CLI guide](guides/cli/)
- [Advanced tutorial](guides/advanced/)
- [Config DSL](specification/config-dsl/)
- [Schema DSL](specification/schema-dsl/)
- [Config implementation](specification/config-implementation/)
- [Schema implementation](specification/schema-implementation/)
- [Config IR v1](specification/config-ir/)
- [Regex implementation](specification/regex-implementation/)

## Run the documentation site

From the repository root:

```sh
cd docs
npm install
npm run dev
```

The production site is generated with `npm run build`.
