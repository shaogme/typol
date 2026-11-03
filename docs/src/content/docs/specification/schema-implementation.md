---
title: Schema implementation
description: How Typol loads, compiles, freezes, and consumes .tschem Schemas.
sidebar:
  order: 4
---

## Quick reference

:::tip[Use this page when]
You need to understand Schema loading, compilation diagnostics, imported files, or how compiled types reach Config parsing and Rust binding.
:::

| Stage | Current behavior |
| --- | --- |
| Input | Shared streaming lexer and parser produce an owned Schema AST with source spans. |
| Compilation | The loader resolves imports, checks static semantics, and builds an immutable compiled Schema. |
| Consumers | Config parsing and Rust binding read compiled Schema views; host file access stays in adapters. |

This document describes how Typol currently implements `.tschem`: crate responsibilities, processing from source text to an immutable compiled result, static semantic checks, and how Config parsing and Rust type binding consume that result. It supplements the [Schema DSL specification](../schema-dsl/) without changing its lexical, grammatical, or semantic rules.

This description follows the current source. The Schema DSL remains normative when behavior is unclear.

## 1. Scope and boundaries

Schema processing has four main layers:

1. `typol-syntax` lexes UTF-8 source into tokens with byte spans and parses Schema into an owned AST.
2. `typol-schema` uses a host-supplied resolver to load the root and `extern mod` sources, builds module/symbol/type models, and performs static compilation.
3. `typol-semantic` stores compiled types, fields, variants, constraints, expression programs, regex programs, source locations, and indexes behind read-only views.
4. `typol-config`, `typol`, and `typol-std` respectively build and validate values using a Schema, provide application-level orchestration, and connect files and process environment.

Core syntax and semantic crates use `no_std + alloc`. Schema compilation does not access the filesystem itself; `typol-schema::SchemaResolver` is the import boundary. Config parsing does not create a configuration AST; it consumes tokens and constructs values directly. `typol-macros` generates Rust type descriptions at compile time and does not read Schema files. Compatibility with a compiled Schema is checked at runtime by `TypedSchema::bind`.

For application-level setup with the facade in a `no_std + alloc` environment, see the [no_std guide](../../guides/no_std/).

| Responsibility | Main source locations |
| --- | --- |
| Lexer, tokens, cursor, Schema AST/parser | `crates/typol-syntax/src/` |
| Imports, compilation session, modules, symbols | `crates/typol-schema/src/resolver.rs`, `loader.rs`, `session.rs`, `module_tree.rs`, `symbols.rs` |
| Type resolution, aliases, recursion checks | `crates/typol-schema/src/types.rs` and `types/` |
| Annotations, regex, expression analysis | `crates/typol-schema/src/annotations.rs`, `regex.rs`, `expressions.rs` |
| Defaults and final records | `crates/typol-schema/src/defaults.rs`, `compiled.rs` |
| Compiled model and semantic views | `crates/typol-semantic/src/model.rs`, `views.rs`, `constraints.rs`, `expressions.rs` |
| Config parse and runtime validation | `crates/typol-config/src/parser.rs`, `validation.rs`, `value_validation.rs` |
| File loading and application facade | `crates/typol-std/src/schema_compiler.rs`, `crates/typol/src/loading.rs`, `binding.rs`, `descriptor.rs` |

## 2. Source, lexer, and parser

### 2.1 Byte sources and tokens

Schema input must be valid UTF-8. Lexical rules are defined by [Schema DSL §1](../schema-dsl/#1-lexical-rules). `ByteSource` provides bytes sequentially: a short read is not EOF, and zero bytes indicates EOF. The parser does not require replayable input and does not retain the complete source. The lexer scans on demand rather than building a token array; `TokenCursor` caches at most the current event and one lookahead. Tokens carry a `SourceSpan` containing a source ID and UTF-8 byte range. Identifier and number tokens can expose their source text, while strings expose decoded content. EOF has an empty span at the end of the source.

Identifier and number token text uses budgeted owned buffers, then the Schema parser copies it into an AST text pool. Strings always use a budgeted owned decode buffer, even when no escape is present, and that buffer is transferred to the AST. The shared lexer reads through a reusable window and does not expose stable source-text slices. Token and AST debug output avoids literal values and shows only kinds, spans, lengths, and similar metadata.

### 2.2 Schema AST and parser limits

`SchemaParser::parse(source_id, byte_source, options, budget)` accepts `ByteSource<Diagnostic>` and uses `SchemaParseOptions` to configure `SyntaxLimits` and `StreamLimits`. `parse_str` wraps an in-memory string as a `SliceSource`. Use `parse_with_positions` when source-location lookup is needed; success includes the AST and a budgeted `SourcePositionIndex`. The index stores token endpoint byte offsets, line numbers, and Unicode scalar columns without retaining source bytes. The EOF token provides actual source length and position.

The parser does not pre-read or cache the entire input. Source limits constrain reads; the actual byte offset at EOF is used for `SchemaAst::span()`, and successful AST storage tightens span boundaries to that length. Diagnostics use the current known position when EOF is unknown and the exact EOF-token span for EOF syntax errors. Success returns a complete AST; lexical or syntax failure returns non-empty `SyntaxErrors`, never a partial AST. At reliable top-level, module, field, and variant boundaries, the parser can recover to collect independent errors. Recovery shares the parser execution budget; terminal resource, allocation, and internal errors stop parsing.

The AST uses table-based nodes and owned text. Callers receive read-only views and cannot forge node IDs or storage tables. It preserves declaration order and spans, alias/container structure, enum payload kinds, annotation argument order, literal syntax, expression structure, and whether object/map literal keys were identifiers or strings. Parsing checks syntax only; it does not resolve names, check types or annotations, or execute expressions. Empty `{}` stays unclassified until compilation determines whether the target is a struct or map.

`SyntaxLimits` bounds parse depth, AST nodes, decoded strings, per-source bytes, and diagnostics. `StreamLimits` separately bounds the token window and bytes per token. Current defaults are depth 128, 100 diagnostics, 256 KiB decoded strings, 100,000 AST nodes per file, an 8,192-byte token window, and a 1 MiB token cap. The parser uses explicit continuation frames for nested declarations, types, defaults, and expressions; recovery is also budgeted.

## 3. Compilation sessions and imports

### 3.1 Entry points

A general or `no_std` caller creates a one-shot session with `SchemaCompiler::new(SchemaCompileOptions)`. For imports, call `compile(root, resolver, observer)`. For in-memory sources without imports, call `compile_without_imports(root)`; an encountered import produces a clear diagnostic. The observer variant registers source names. `SchemaSource::from_source` accepts a one-shot `ByteSource<SourceReadError>`; `from_str` adapts an in-memory string. Source identities compare byte-for-byte and are not written to the compiled artifact. Source objects cannot be copied, and their input is consumed by the loader. An optional `SchemaCompileContext` ties root reads, resolver work, and compiler phases to one shared budget.

`SchemaResolver::resolve` receives the current source identity, one module name, remaining source-byte capacity, and compile context. It returns the next Schema source or a bounded `ResolverError`. The host chooses how module names map to files or in-memory sources. An optional `SourceObserver` registers each identity-to-`SourceId` association for host-readable names. Duplicate identities are not registered twice.

For standard file use, `typol_std::FileSchemaCompiler::compile` opens and checks file handles while the parser reads the Schema body on demand; it does not create a complete file-text buffer. It also handles relative imports, host file identities, and source-name mapping. The application facade `SchemaLoader::compile_file` loads a file, `compile_text` provides an import-free in-memory path, and `compile_with_resolver` supports custom imports. After loading, `LoadedSchema::bind<T>()` creates a reusable binding so multiple configurations can be read without repeatedly matching Rust and Schema types.

A root source explicitly bound to a different call fails with `SCHEMA_CONTEXT_MISMATCH`; an ended execution context fails with `SCHEMA_SESSION_FINISHED`. `SchemaCompileErrors::context_failure()` exposes the structured reason and preserves the root span. These contract/lifecycle failures do not call the resolver. Host resolver failures use `SCHEMA_RESOLVER_ERROR`.

### 3.2 Import graph loading

The root receives `SourceId(0)`; subsequently registered sources receive IDs in load order. The loader requests sources for root AST `extern mod` declarations and loads them depth-first using an explicit stack. Each import edge has a state and optional target; failed edges have no valid target.

Identities compare byte-for-byte. An identity still in `Loading` is an import cycle; an already loaded identity is a duplicate import. Different identities are not treated as the same source merely because they return identical bytes. The loader registers each `SourceId` with the observer before parsing, but reads the body only after identity deduplication. Read failures map from bounded `SourceReadError` categories to structured diagnostics; arbitrary host error text is excluded.

`FileSchemaResolver` maps stable IDs of open files to source identities and keeps handles alive for the session so IDs cannot be reused. Unix uses device/inode; Windows uses volume serial number/file index, so hard links share identity. Platforms without stable IDs deduplicate by canonical path. On Linux, when available, `/proc/self/fd` resolves the import base from the handle actually read; otherwise the requested path is normalized. File contents are still read incrementally.

The loader checks per-file bytes, total source bytes, Schema file count, and import nesting during reads. It counts actual bytes and may request at most one extra byte to distinguish exactly reaching a limit from exceeding it. A low-level error may report bytes already consumed. Schema imports occur only at AST file scope; ordinary DSL `mod` is a namespace within the same source.

### 3.3 Shared session budget

One `ExecutionBudget` covers source loading, parsing, semantic passes, record creation, indexing, location mapping, and freezing. In addition to language resource limits, it bounds execution steps and workspace bytes. Resource exhaustion and allocation failure are distinct errors. Stage labels identify current work for host observation.

Frozen indexes store sorted name lookup tables by field/variant slot. Index allocation and comparisons use the same budget. Indexes contain slots only; fields and variants remain in declaration order. Read-only variant lookup may use budgeted binary search.

Current `SchemaLimits` defaults include 1 MiB per source, 8 MiB total source bytes, and 64 sources. Nested `SyntaxLimits` include depth 128, 100 diagnostics, 256 KiB decoded strings, and 100,000 AST nodes per file. `StreamLimits` use an 8,192-byte window and 1 MiB token. Schema default validation allows 100,000 value nodes. `RegexLimits` govern pattern bytes/count, nesting, instructions, character ranges, search steps, and scratch states. Default source excerpts are 65,536 bytes total, 512 bytes per line, 128 records, and two context rows per span. These values are overridable runtime policy, not DSL semantics.

## 4. Static compilation pipeline

`typol-schema::SchemaCompilation::compile` runs the stages below in order. Any diagnostic prevents creation of a usable `CompiledSchema`; the final artifact is returned only when every stage succeeds.

### 4.1 Parse each source

The loader runs `SchemaParser` for the root and every newly imported source. Parse errors retain their `SourceId` and source span. A source with syntax errors does not enter the module tree. Other independently loadable sources may still be used to collect diagnostics unless a terminal budget error occurs.

### 4.2 Module tree and namespaces

`ModuleTree` places each source's root declarations in the corresponding module and recursively adds ordinary `mod` declarations. Modules and type declarations use stable IDs, and module members keep insertion order. The root file's `entry struct` is the sole configuration entry. An entry marker in an imported file does not replace it; that type remains referenceable in the declaration tree.

`SymbolTable` builds a shared module/type namespace index for every module and indexes struct fields, enum variants, and anonymous struct payload fields. It checks duplicate declarations, entry count, duplicate fields, and duplicate variants; duplicate-name diagnostics may point to both declarations. Lookup reports missing, unique, or ambiguous results instead of silently choosing one duplicate.

### 4.3 Type paths and the resolved type graph

Type references search for the first path segment from the current module upward through parent modules. After the first segment resolves, remaining segments are resolved only within module namespaces. A middle segment resolving to a type instead of a module, a final segment that is not a type, or an invalid module/symbol produces a semantic error.

The type graph assigns semantic IDs to built-ins, user types, containers, alias links, and anonymous struct enum payloads. Aliases are transparent shapes: compiled results retain the named alias record and its underlying shape, while use sites can retain alias constraints and `@secret` metadata. The compiler detects unknown paths, alias cycles, user-type recursion through containers/aliases, nested options, and fixed-array lengths outside their permitted range.

Type records include built-ins, options, arrays with optional fixed lengths, maps, structs, and enums. Struct fields and enum variants retain declaration order. Each enum variant records whether it has unit, single-value, or anonymous struct payload.

### 4.4 Annotations and regex

The annotation compiler stores AST annotations by alias, field, and single-payload variant. Each slot contains compiled constraints, a secret bit, and an optional environment authorization. It checks target types, argument forms, repeated annotations, bound ordering, and compatibility among combined constraints. An effective rule view applies both alias-level and local constraints.

Schema compilation passes each `@regex` string to `typol-regex`, which compiles a bounded Thompson NFA. See the [regex implementation](../regex-implementation/). Invalid patterns become regex syntax diagnostics. Pattern bytes, regex count, per-pattern and cumulative instruction counts, and character-range counts are bounded. Compiled constraints retain references to regex programs, not a pattern to recompile for every Config validation.

`@env` compiles to a field authorization record; the Schema compiler does not read the environment. Aliases cannot carry environment authorization. `@secret` metadata propagates to use sites for Config diagnostics, safe display, and Rust-value conversion.

### 4.5 Static expression analysis

The expression compiler processes `@assert` and `@all` ASTs. It classifies `self`, `root`, closure parameters, and map `key`/`value` context; then it type-checks paths, methods, literal inference, operators, and comparisons node by node. The program must return boolean. Invalid fields, methods, contexts, operands, or a non-boolean result fail the Schema compilation.

Successful expressions become typed expression nodes and program IDs in `typol-semantic`. `@all` constraint slots are bound to expression programs later. Instance assertions do not execute during Schema compilation. Collection constraints on static defaults are checked by the default-construction stage.

### 4.6 Static default construction

Default compilation uses each field's resolved type to construct a `ConfigValue` from the AST. It checks scalar shape, numeric range, finite floats, and recursively constructs arrays, maps, objects, and enum payloads. Integer defaults accept integer tokens only; float defaults accept float tokens only, matching configuration literals. An `f32` token is parsed directly to binary32 to avoid an intermediate `f64` rounding. A brace literal is classified as struct or map according to its target. Unknown or duplicate object fields and missing required fields invalidate the Schema.

Object defaults may omit fields supplied by field defaults or optional-type rules. Field-default dependencies are evaluated with explicit frames and cycles are detected. Compiled nested defaults are copied to applicable fields. Schema defaults never read the environment. Construction recursively checks target shape and local, inherited alias, field, and active enum-payload constraints. It retains recovery state, element locations, and labels. Successful validation produces `SchemaDefaultValidation`; full Schema compilation passes that result with the corresponding defaults to `Builder::freeze_with_validated_defaults` to avoid a duplicate traversal. Ordinary `Builder::freeze` receives no validation result and fully validates arbitrary inputs. Value nodes, depth, strings, regex execution, and rule work use Schema compilation's `ValueLimits`, `RegexLimits`, and shared `ExecutionBudget`. An invalid default cannot produce validation evidence or be frozen. Cross-field `@assert` depends on a complete configuration instance and `root`, so it runs only after a Config value is built.

### 4.7 Freeze and return records

`RecordBuilder` freezes temporary module/symbol/type graphs, constraint slots, defaults, expression programs, and source positions into `typol-semantic::CompiledSchema`. Freezing builds field/variant slot indexes and validates record structure. Ordinary `Builder::freeze` also fully validates each default; full Schema compilation reuses validation results tied to the defaults. The compiled model is returned only when all checks succeed.

The final model is read through immutable `TypeView`, field, variant, and effective-rule views; callers cannot mutate records, indexes, or compile state. Struct field lookup uses sorted slot indexes. Expression field reads confirm the declared slot agrees with the value item's name before reading it. A sorted variant index maps back to records that remain in declaration order. `CompiledSchema::entry()` returns a view of the root entry type.

The final record also indexes source spans to 1-based line/column `SpanLocation`s. Parser-time `SourcePositionIndex` supplies endpoint locations. At freeze time, `SourceCatalog` checks spans against source length and UTF-8 boundaries without reading source text. Lines and columns count Unicode scalar values; spans remain UTF-8 byte offsets. A successful record with a missing endpoint is a compile failure. On error, if a location cannot be provided from the diagnostic or index, the original diagnostic is retained and `LocationStatus` reports unavailable; the source is not reread.

`LoadedFile` retains identity, AST, actual byte length, position index, and a bounded candidate-capture buffer, not full source text. Candidate byte and record budgets are divided among the maximum number of sources so total captured candidates stay within `SchemaLimits::excerpts()`; all storage counts against shared workspace. Once an AST is complete, default-value spans are masked before candidate lines referenced by diagnostics or compiled records are frozen. Successful artifacts own bounded safe `SourceExcerptSet` data. On failure, needed excerpts are extracted before the source catalog is released and transferred to `SchemaCompileErrors`. Missing candidates are marked truncated or unavailable; the source is not reread and the main diagnostic is unchanged.

## 5. Static errors and diagnostic contract

Schema failures return a non-empty `SchemaCompileErrors` collection or a non-allocating inline emergency error. Public readers include `diagnostics()`, `span_location()`, and `location_status()`. Diagnostics have stable codes, primary spans, and optional labels; duplicate-name diagnostics can point to both the new and previous declaration.

`SchemaCompileErrors` implements `Display` and the standard error interface. Display writes error-code name and number, message, available location, and safe source excerpt without first allocating an owned report string. Debug shows diagnostic count and location status only. `internal_cause()` exposes structured `SchemaInternalCause`, preserving AST access error variant, full `usize` index, and range endpoints; internal causes are also included in display output. Read source snapshots with `excerpts()`. With zero excerpt capacity or collection failure, diagnostics and available locations remain, while excerpt status is `Unavailable` or `Truncated`.

Relevant current `typol-core::DiagnosticCode` values are: resource limit 1, allocation 2, lexical error 3, Schema syntax 4, diagnostic limit 5, regex syntax 6, Schema static semantics 7, resolver/host import 8, root compile-context mismatch 19, and finished compile session 20. Resource, allocation, and internal failures take terminal priority. Ordinary diagnostics sort stably by source ID and byte span; a terminal slot is preserved at the diagnostic limit. Diagnostics must not copy full source lines, raw environment values, or sensitive defaults. Host errors use bounded categories and exclude arbitrary OS text.

The syntax layer creates lexical and Schema syntax errors. Missing imports, name/type mismatches, annotation errors, and expression type errors are created during Schema compilation. Callers must stop Config parsing when the Schema is invalid; partial records are not valid for loading.

## 6. Use during Config parsing

`typol-config::ConfigParser::parse` requires a valid `CompiledSchema`, Config source/options, and an `Environment` implementation. It starts at `schema.entry()`, reads tokens according to the expected type, and constructs the value tree directly. The compiled type determines whether braces mean struct or map; parsing does not retry the other interpretation to recover from a type error.

When a struct or anonymous struct payload completes, the parser applies explicit values, field defaults, optional `none`/promotion rules, and diagnostics for unknown, duplicate, or missing required fields. Arrays, fixed arrays, maps, and enum payloads are checked against compiled shape. Struct and active-variant assertions run only after all values are complete. Any construction or validation error prevents a partial instance from being returned.

Local scalar/field rules run as values complete; array/map collection rules run when containers close; full-tree assertions run last. Environment reads occur only for explicit `env(...)` expressions authorized by a Schema field. The same raw environment value is read once per name in a call, while each field reference is converted and validated independently. Defaults do not come from the environment. `typol-core::ValueSource` distinguishes configuration literal, Schema default, environment, fallback, and Rust conversion origins.

Compiled alias, field, and payload constraints, expression programs, regex programs, secret markers, and source positions are consumed through read-only views. Runtime regex search uses the caller's limits and bounded scratch; budget exhaustion is a resource error, not a non-match.

## 7. Rust binding and host entry points

`#[derive(Typol)]` generates type descriptions and conversion implementations from Rust types alone; it does not read a Schema during macro expansion. `LoadedSchema::bind` and `TypedSchema::bind(compiled_schema, options)` require the hidden `BudgetedTypol` marker. Derive adds marker bounds for nested fields and enum payloads; built-in scalars and containers implement it subject to their child types. If a derived type contains a handwritten `Typol` type, mark it with `#[typol(trusted)]` and use `bind_trusted`.

Handwritten mappings enter through `LoadedSchema::bind_trusted` or `TypedSchema::bind_trusted` and produce `TrustedBoundSchema` or `TrustedTypedSchema`, keeping that choice visible in the Rust type. Library-managed work through `DescribeContext`, `ConversionContext`, and subtype dispatch remains budgeted. The library cannot constrain callback allocation, blocking, or arbitrary computation, and does not revalidate a handwritten decoder's returned Rust value. `BudgetedTypol` is a capability marker for ordinary APIs, not a sandbox or security boundary against deliberate forgery. Isolate untrusted implementations in a process or Wasm boundary.

Prefer derive for ordinary struct and enum mappings. Keep handwritten implementations for special mappings whose callbacks have been reviewed and are trusted. When migrating, inspect direct `impl Typol` and transitive nesting through derive fields/payloads. Either bind the entire parent containing a handwritten type as trusted or refactor it into ordinary fields/enums and derive. Configuration loading should explicitly compile the Schema, bind the Rust type, then read values; trusted types use `bind_trusted` after loading.

Built-in Rust map binding implements `Typol` only for `HashMap<String, T>`. Decoding reserves capacity with `try_reserve` and charges actual capacity against the target budget. The standard library's `BTreeMap` has no fallible node-reservation API, so it has no automatic `Typol` implementation. Applications that need ordered maps can bind with `HashMap` and convert to `BTreeMap` at the boundary.

Text parsing, foreign `ConfigValue` normalization, and Rust serialization share full-tree Schema validation. Rust values cannot skip field/collection constraints or full-tree assertions. Serialization follows Schema field order and decoded map key order. Contextual Serde methods use the same validation after binding; ordinary Serde traits without Schema context do not promise validation against an actual `.tschem`.

### Serde bridge support

`TypedSchema::serialize_serde` and `TypedSchema::deserialize_serde` are context-aware Typol bridges that consume or produce Serde data-model events under a successfully bound Schema. Passing a normal Serde trait directly to another serializer/deserializer bypasses this bridge and carries no validation guarantee for a particular `.tschem`. Caller-provided `Serialize`/`Deserialize` code remains a user-code trust boundary even with a `BudgetedTypol` binding; the bridge charges only work it controls.

| Serde data-model operation | `serialize_serde` | `deserialize_serde` |
| --- | --- | --- |
| Boolean, integer, finite float, string | Supported | Supported |
| `Option<T>` | Supported; nested option rejected | Supported; nested option rejected |
| Sequence | Supported as Schema array | Supported as Schema array |
| Tuple | Supported only for a same-length fixed Schema array | Supported only for a same-length fixed Schema array |
| Map | Supported with string keys only | Supported with string keys only |
| Named struct | Type and field names must match Schema | Type and field names must match Schema |
| Enum variant | Unit, newtype, and named-field payloads | Unit, newtype, and named-field payloads |
| `serialize_char`, bytes, standalone unit, unit/newtype/tuple struct, tuple enum variant, `collect_str` | Rejected | Not applicable |
| `deserialize_any`, `deserialize_ignored_any`, root-level `deserialize_identifier`, char, bytes, standalone unit, unit/newtype/tuple struct, tuple enum variant | Not applicable | Rejected |

The bridge uses three stable `DiagnosticContext::failure_point()` identifiers under `RUST_CONVERSION_ERROR`: `serde-data-model-operation-unsupported` for unsupported operations; `serde-user-custom-rejection` for a user callback rejecting with Serde's custom error; and `serde-schema-shape-mismatch` when a supported operation conflicts with Schema type, names, fields, enum payload, or fixed-array length. Serde internally uses `deserialize_identifier` to read struct field and enum variant names; the root-level restriction means decoding the target itself directly as an identifier is unsupported.

Callers can read these static contexts with `CodecErrors::diagnostic_context(index)`. Custom error text is not stored in the error container, Display, Debug, or diagnostics, preventing sensitive input leakage. Full Schema value validation occurs after Serde serialization events are converted; resource, allocation, and Schema errors retain their own categories.

`typol-std` supplies standard adapters such as `FileSchemaResolver`, `FileSchemaCompiler`, and `ProcessEnvironment`. `SchemaLoader` and `LoadedSchema` coordinate Schema loading, source names, binding, and configuration reads. Core parser/compiler code does not call file APIs, `std::env`, or terminal output.

The loading facade uses the same `SchemaLoader`, `LoadedSchema`, `BoundSchema`, `ConfigInput`, `TypolError`, and `TypolStage` types in `no_std + alloc` and `std` builds. Shared reads receive an explicit `Environment`; host adapters add file and reader inputs, and applications can pass a `ProcessEnvironment` provider when they want process-variable lookups.

## 8. Security and data handling

- Source spans use byte offsets internally; line and column conversion happens only for display.
- Raw environment bytes live only in the current Config call's cache and are not copied to ordinary diagnostics.
- `@secret` and alias-propagated secret markers drive safe diagnostics and display; secret containers hide child paths and map keys.
- Rust conversion diagnostics from `TypedSchema::serialize`/`deserialize` are hidden and path-truncated before entering `CodecErrors`. Unresolvable paths are cleared, and redaction failure never falls back to an original path.
- Default-value source ranges and secret Config subtrees are masked before source snapshots are published. Secret map items mask key and value. If masking cannot be proven complete, the line is omitted.
- Errors and compiled artifacts retain bounded safe source lines, not full input. Default reports exclude raw environment values, real file paths, and native OS error text.
- Encoded values must retain secrets for round-tripping; safe display and serialization are separate purposes.
- Generic `Debug` for `ConfigValue`, Schema tokens, and compile errors must not recursively expose sensitive literals.
- Resource or allocation failure must not panic, recurse without bound, or return partial AST, Schema, or Config values.
- Unicode string length, regex characters, source columns, and map-key equivalence follow their DSL definitions; UTF-8 byte count, locale, and implicit normalization are not substitutes.

## 9. Maintenance

Coverage should be organized by lexical/AST structure; missing, cyclic, and duplicate imports; namespaces and root entry; path resolution, alias cycles, and recursive types; annotation applicability and bound conflicts; complete default construction and constraints; expression types/context; regex subset and cumulative limits; source positions, diagnostic caps, and allocation failures; runtime consumption of compiled records; and Rust binding/contextual codecs. Tests compare stable diagnostic codes, locations, and structured results rather than debug strings. Public end-to-end examples and resource-policy reviews belong in regression coverage. If implementation conflicts with the Schema DSL, record and revise the specification/decision before changing parser or compiler behavior.

### 9.1 Bounded diagnostic source excerpts

`SchemaLimits::excerpts()` configures `ExcerptLimits` for total storage bytes, line bytes, record count, and context rows per span. Zero disables excerpts. A bounded window starts at the first line of each span; a span extending beyond that window is marked truncated, and context lines are split before/after it. Candidate windows, redaction ranges, and final snapshots each have byte caps and use the call workspace budget. Optional allocation failure does not change the main diagnostic. An owned `SourceExcerptSet` has status `Complete`, `Truncated`, or `Unavailable`; missing records never reread or display source. Columns count Unicode scalars, tabs count as one column, CRLF is excluded from line text, and UTF-8-safe truncation adds an ellipsis.

For Config input, streaming parsing keeps bounded candidates and a recent-line window, pinning lines referenced by diagnostics until the call ends. Delayed diagnostics may use only retained candidates. At completion, only lines referenced by real diagnostics and their context are published. Known secret values and secret map spans are masked; unfinished secret values are masked to input end. Recovery-skipped or unbound content is hidden conservatively. If range recording fails, excerpts are unavailable. Raw environment values never enter the window.

Schema candidates are masked for default-value spans only after a complete AST exists, then lines referenced by diagnostics and compiled records are extracted. On syntax failure, uncertain masking boundaries, or capture failure, raw candidates are discarded. Errors and compiled artifacts retain no full source; new debug output exposes only state and counts.
