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

<div class="typol-flow" role="list" aria-label="Typol configuration workflow">
  <div class="typol-flow-step" role="listitem">
    <span class="typol-flow-index">01</span>
    <strong>Define a schema</strong>
    <code>.tschem</code>
  </div>
  <div class="typol-flow-step" role="listitem">
    <span class="typol-flow-index">02</span>
    <strong>Parse configuration</strong>
    <code>.tconf</code>
  </div>
  <div class="typol-flow-step" role="listitem">
    <span class="typol-flow-index">03</span>
    <strong>Bind Rust types</strong>
    <code>#[derive(Typol)]</code>
  </div>
</div>

Typol compiles `.tschem` schemas, parses `.tconf` values against the compiled types, validates constraints and assertions, and binds the result to Rust types. Choose **简体中文** in the language selector for the Chinese edition.

## Documentation

<div class="typol-doc-grid">
  <section class="typol-doc-card">
    <span class="typol-card-kicker">01 / Start here</span>
    <h3>Quick start</h3>
    <p>Build a typed configuration loader and see the full Schema-to-Rust workflow.</p>
    <ul>
      <li><a href="guides/quickstart/">Quick start guide</a></li>
    </ul>
  </section>
  <section class="typol-doc-card">
    <span class="typol-card-kicker">02 / Learn by task</span>
    <h3>Guides</h3>
    <p>Choose a guide for command-line workflows, advanced patterns, or no_std apps.</p>
    <ul>
      <li><a href="guides/cli/">CLI guide</a></li>
      <li><a href="guides/advanced/">Advanced tutorial</a></li>
      <li><a href="guides/no_std/">no_std guide</a></li>
    </ul>
  </section>
  <section class="typol-doc-card">
    <span class="typol-card-kicker">03 / Reference</span>
    <h3>Specifications</h3>
    <p>Look up language rules, exported data format, and implementation behavior.</p>
    <ul>
      <li><a href="specification/schema-dsl/">Schema DSL</a></li>
      <li><a href="specification/config-dsl/">Config DSL</a></li>
      <li><a href="specification/config-ir/">Config IR v1</a></li>
      <li><a href="specification/schema-implementation/">Schema implementation</a></li>
      <li><a href="specification/config-implementation/">Config implementation</a></li>
      <li><a href="specification/regex-implementation/">Regex implementation</a></li>
    </ul>
  </section>
</div>

## Run the documentation site

From the repository root:

```sh
cd docs
npm install
npm run dev
```

The production site is generated with `npm run build`.
