---
title: Config implementation
description: How Typol parses, constructs, validates, and safely reports .tconf values.
sidebar:
  order: 3
---

## Quick reference

:::tip[Use this page when]
You need to trace a configuration read through the parser, environment provider, validation pipeline, or resource limits.
:::

| Area | Current behavior |
| --- | --- |
| Parsing | Value reads consume tokens incrementally without retaining an AST; document reads additionally retain bounded source text and parser-produced value spans. |
| Environment | Reads happen only for authorized `env(...)` expressions through the provider passed by the caller. |
| Formatting | `ConfigFormatter` scans a validated document with an explicit delimiter stack and retains source tokens and comments. |
| Limits and errors | Parsing, validation, diagnostics, and execution use explicit resource budgets and structured failures. |

This document describes the current implementation of `.tconf` handling: entry points, streaming lexical input, Schema-guided value construction, environment reads, constraint validation, error recovery, and safe diagnostics. It supplements the [Config DSL](../config-dsl/) and [Schema implementation](../schema-implementation/) documents without changing DSL semantics.

This description is based on the current source. Config and Schema DSL specifications define semantics; current source defines resource defaults.

## 1. Scope and boundaries

Configuration handling is shared across four layers:

1. `typol-syntax` reads UTF-8 bytes from `Read` on demand and emits tokens with source spans. It does not build a complete token array for configuration.
2. `typol-config` reads token input against read-only views of `CompiledSchema` and directly builds an owned `ConfigValue`. The document entry point also retains the original text and `LocationIndex` spans from that same parser pass; it does not re-lex the text to locate values.
3. `typol-semantic` supplies read-only views for types, fields, variants, constraints, assertion expressions, environment authorization, and regex programs, as well as expression execution.
4. `typol` combines parsing with Rust conversion, Serde, loading sessions, and optional Config IR export. `typol-std` supplies file input and process environment adapters.

The core uses `no_std + alloc`. It does not open files, read the process environment, or write to a terminal. Environment access occurs only through the caller's `Environment` implementation. Successful results own their strings, keys, and values; they do not borrow configuration input or provider buffers.

For application-level setup with the facade in a `no_std + alloc` environment, see the [no_std guide](../../guides/no_std/).

| Responsibility | Main source locations |
| --- | --- |
| Public entry points, requests, limits, sessions | `typol-config/src/api.rs`, `options.rs`, `session.rs` |
| Schema-guided parser state machine | `parser.rs` and `parser/` |
| Recovery and duplicate map-key index | `parser/recovery.rs`, `parser/map_index.rs` |
| Environment access, cache, scalar conversion | `environment.rs`, `scalar.rs` |
| Defaults, foreign-value normalization, instance paths | `value_normalization.rs`, `instance.rs` |
| Scalar, collection, assertion, and full-tree validation | `validation.rs`, `validation/`, `value_validation.rs` |
| Diagnostics, path truncation, secret hiding | `diagnostics.rs`, `secret.rs` |
| Source formatting and lexical comment scanning | `document.rs`, `document/formatter.rs`, `document/formatter/format_scan.rs` |
| Token stream and UTF-8 input | `typol-syntax/src/input.rs`, `lexer.rs`, `cursor.rs` |
| Value origins and execution budget | `typol-core/src/value.rs`, `budget.rs` |
| Host adapters and facade | `typol-std` filesystem/environment modules and `typol` loading/binding/codec modules |

## 2. Entry points and sessions

### 2.1 Parsing configuration text

General callers use `ConfigParser::new(schema).parse(request, environment)`. Success returns `ValidatedConfigValue`, which certifies that value constraints and full-tree assertions passed. Callers can borrow its `ConfigValue`; `into_config_value()` consumes the validation marker when an unmarked owned value is required.

`ConfigParseRequest` contains a readable `Read`, display name, and `ConfigParseOptions`. Options hold `ExcerptLimits` and `RegexOptions` independently, alongside `ConfigLimits`, generic execution/workspace `ExecutionLimits`, and lexical `StreamLimits`. The shared lexer gives Schema and Config the same block-string behavior. Because a `Read` source need not support replay, it stages the current block token body in charged workspace, then computes indentation and decodes into a second charged buffer. Scanning uses the shared execution budget; the source token ceiling includes delimiters, closing indentation, and original CRLF bytes, while the string ceiling counts only final decoded UTF-8 bytes. The display name is diagnostic metadata, not part of the configuration input.

`ConfigParser::parse_document` and `LoadedSchema::read_document` retain the parser's UTF-8 source buffer and semantic location index. The document adopts that buffer as its source text instead of copying it into a second full-size allocation. Until the first edit, the source itself also supplies the `DocumentSnapshot` write expectation; on the first successful edit, the previous source buffer becomes the charged snapshot without another text allocation. A successful write advances the expectation to the current source and releases that separate snapshot.

`ConfigDocument::write_to` returns the exact retained text. `apply_edits` resolves typed replacements, field/map/array insertions, and removals through the parser index. It groups child index entries by parent and prepares each touched container's bounds, item positions, commas, and layout once per transaction. It rejects overlapping source patches, assembles one candidate, then reparses and validates it before committing the in-memory document. That parse records locations without retaining another copy of the candidate; after validation, the candidate allocation becomes the new document's retained source. Replacements encode the requested subtree; insertions infer the container line ending, indentation, and key spacing. Inserted values are encoded directly into their final patch buffer. Map-entry existence checks use the resolved parent node and do not clone the full path. Removal and composite replacement extract comments from the touched range and preserve their bytes in the replacement area. Untouched source ranges remain byte-for-byte unchanged. Edit counts, sorting, index traversal, and bounded source scans consume the shared execution budget. Batch related changes with one `apply_edits` or `replace_values` call to pay for one candidate parse and validation; separate `set_value` calls each pay that cost. The editor scans bounded source slices for delimiters and comments and treats both string forms as opaque spans, so brackets, commas, and comment markers inside a block string do not affect container matching. It does not build a separate token/trivia CST.

`ConfigFormatter::format(&document, options, out)` formats only a successfully parsed `ConfigDocument`. It scans retained source with an explicit delimiter stack; it does not build a configuration AST or re-encode `ConfigValue`. `FormattedConfig` owns the complete result, provides a read-only string view, and can be consumed by writing it to a `typol_core::Write` sink. `ConfigFormatter::is_formatted` runs the same formatter while comparing each produced chunk with the source, without allocating an output buffer; the CLI uses this path for `config fmt --check`. The original document and its source locations remain unchanged. Formatting keeps every non-whitespace source token and comment in order, including string and number spellings and `env(...)` expressions. Each block string is one opaque token whose internal bytes, indentation, and LF/CRLF spelling are preserved; `format` and `is_formatted` use the same boundary scanner. Nonempty containers expand to one entry per line with four-space indentation and trailing commas; empty containers stay compact unless they contain comments. Single-value enum payloads use multiline parentheses without a trailing comma, while `env(...)` calls stay compact. LF or CRLF is preserved, output ends in that line ending, and formatting is idempotent.

`ConfigFormatOptions` sets the output-byte, formatter-workspace, and nesting limits; it has no style switches that could create multiple canonical formats. Formatting also observes the document's source and constructed-text limits and charges stack storage and scanning work to the document's shared execution budget. `format` builds an owned charged output buffer and accounts for the constructed text; `is_formatted` uses the same output limits but only compares chunks and does not construct or retain that buffer. The comma lookahead for trailing comments runs once per consecutive comment run, so a run of `k` comments adds linear scan work. The owned result is returned only after scanning, bounds checks, and output accounting succeed. Structure errors, resource exhaustion, allocation failure, and a later output-sink error remain distinct; an incomplete buffer is never returned as a successful result.

`LoadedSchema::read_file` returns a `ConfigDocument`; callers that only need the previous validated-value result use `read_file_value`. With `std`, `LoadedSchema::write_file` uses the document snapshot expectation and enforces both the source and file input byte limits. Ordinary filesystems cannot atomically compare file bytes and replace a path, so the local adapter returns `AtomicReplaceUnsupported` for `ExactSnapshot`. Use `write_file_with_expectation` with `MustBeAbsent` for a new target or `ReplaceAny` for an intentional overwrite. New-target commit uses same-directory hard-link creation on Unix and a same-volume no-replace write-through move on Windows; both atomically refuse a target created by a concurrent writer. `ReplaceAny` uses atomic replacement; Unix syncs the parent directory after commit and reports a distinct error if durability cannot be confirmed. Windows uses a same-volume write-through move. no_std callers implement `SourceWriter` for their target and must provide the requested expectation semantics.

`ConfigValueProcessor::validate` and `ConfigValueProcessor::normalize` accept a `ConfigValidationRequest`. Its options configure `ExcerptLimits`, `RegexOptions`, `ConfigLimits`, and generic execution limits, but no token stream. Validation and normalization do not read configuration input or an environment provider and have independent limits from text parsing.

`TypolOptions` and `SchemaLoader::new(TypolOptions)` are shared by no_std and std builds. `LoadedSchema::read_value` and the `BoundSchema` read methods accept a generic `ReadRequest<R>`; the reader implements the same `typol_core::Read` contract used by the lexer. In-memory text can use `ReadRequest::from_text`, while no_std callers can pass a `SliceSource` or custom reader. All reads explicitly receive an `Environment` provider. With `std`, `read_file` opens configuration through `FileConfigInput`, and `read_io` adapts a standard I/O reader through `ReadSource`; `SchemaLoader::compile_file` adapts file schemas into the same compiler flow. To use process variables, callers construct `ProcessEnvironment` with their chosen `ProcessEnvironmentLimits` and pass it to those read methods.

With `std`, the same `TypolOptions` also carries host limits. Configure file input limits with `with_file_input_limits`; file Schema, source name, and source name edit limits use `with_files`, `with_names`, and `with_name_edits`. Configuration and binding limits use `with_config_limits` and `with_binding_limits`. Access lower-level configuration policies from `typol::config` and host adapters from `typol::host`. File input limits cover configuration files and path bytes, process environment limits cover names and values on the `ProcessEnvironment` provider, Schema imports use `FileSchemaLimits`, and execution limits remain in Schema/Config options.

`ConfigLimits` has finite defaults and named, chainable `with_max_*` updates. Its fields are private; it has no positional constructor or mutable setters. Read-only getters expose values, and it can be passed to `ConfigParseOptions::with_config`. Codec options use named updates as well.

`ConfigParser::parse` creates a generic `ExecutionBudget` for the call. Use `ConfigParser::parse_with_budget` to share its non-Regex capacity with an outer load, bind, or codec operation. The supplied budget's limits must exactly match the request's generic execution limits, or parsing returns an internal configuration error. Parsing, environment handling, value validation, and diagnostic path processing share that generic budget; Regex compilation is not part of Config parsing, while all Regex checks in the call share a separate `RegexExecutionBudget` created from `ConfigParseOptions::regex()`.

### 2.2 Validating foreign values

`ConfigValueProcessor::validate` accepts an owned `ConfigValue`, copies and normalizes it against the Schema, then performs the same full constraints and assertions as the text path. It does not accept an `Environment` and cannot read environment variables. Use `ConfigValueProcessor::validate_with_budget` to share generic call capacity; Regex checks still use the separate budget from `ConfigValidationOptions::regex()`. Normalize-only operations do not create or consume a Regex budget.

Normalization checks each value shape, field name, enum payload, and scalar target, rebuilds structs in Schema field order, and fills Schema defaults and optional fields without defaults. It creates a new owned tree. Missing required fields, unknown or duplicate fields, invalid map keys/values, and incorrect payload shapes fail. Maps have no DSL ordering; the foreign-value path sorts keys for stable reconstruction.

Normalize-only operations reject values with environment origins because they do not run the complete authorization and fallback validation for those references. Use `ConfigValueProcessor::validate` when an external value must retain an environment origin.

### 2.3 Generating configuration from Schema defaults

`LoadedSchema::default_config_to_string` and `LoadedSchema::write_default_config` generate `.tconf` from the compiled entry Schema without binding a Rust type or receiving an `Environment`. The facade creates an empty structure root and calls `ConfigValueProcessor::validate_with_budget` once, so default filling, field and collection constraints, and full-tree assertions use the same validation pipeline as other external values. Generation errors are returned through `TypolError` with stage `Encoding`; value diagnostics remain available through `codec_errors().config_value_errors()`.

For each missing field, the shared completion rule selects its explicit Schema default first, then `none` for an `option<T>`, otherwise a required-field diagnostic with the field path and Schema definition location. An entry struct is the only synthesized structure root. A nested struct, array, map, or enum is not created because its contents look constructible; the field itself needs an explicit default. A selected nested object or enum payload is then completed recursively. `@env` only authorizes expressions in input text: generation never evaluates `env(...)` or consults a provider.

`DefaultConfigOptions` defaults to `TextLayout::Compact`, a 1 MiB UTF-8 byte limit, and allowing actual `@secret` values. `with_layout(TextLayout::Pretty)` selects the same four-space canonical layout used by `config fmt`: multiline compound containers, multiline single-value enum payload groups, and one final LF. The removed `with_pretty(bool)` and `pretty()` methods are a source-breaking API change; migrate callers to `with_layout(TextLayout)` and `layout()`. The options can also reject values marked secret; rejection finishes before the first sink write and returns a stable encoding diagnostic without the value text. Generated secrets remain plaintext when allowed and must be protected as sensitive configuration. The string API returns only after complete validation and encoding; the sink API can leave a prefix if encoding, sink I/O, execution budget, or output limit fails.

For atomic file creation or replacement, generate the complete string first and pass its bytes to `FileSourceWriter::replace_atomically` with an explicit `SourceExpectation`. `LoadedSchema::write_file` operates on a `ConfigDocument` source snapshot and is not the default-generation file API.

## 3. Streaming parse and value construction

### 3.1 Input and root value

The parser allocates a new `SourceId` for the configuration source, starts at `schema.entry()`, and confirms that the root type is a struct. The lexer reads through a fixed-size window from `Read`; the token cursor caches the current event and at most one lookahead. Streaming input tracks line and column positions, while token spans use UTF-8 byte offsets.

The root is parsed as a struct object. Once it is complete, the parser continues to EOF; any extra token makes the configuration invalid. Thus each call accepts one root object followed only by DSL whitespace and comments. Tokens are consumed into the value tree; no configuration AST is retained for callers.

### 3.2 Interpreting tokens by Schema type

`ValueParser` uses an explicitly budgeted frame stack to handle nested values without mapping input nesting onto host recursion. Each frame stores the current Schema type and continuation state for a container or payload:

- A scalar consumes one literal of the exact built-in Schema type. Strings use lexer-decoded content; integers are checked against target width; floats must remain finite after conversion.
- `option<T>` accepts `none` or parses and wraps a `T`. No other implicit conversion occurs.
- Arrays build elements in input order and check fixed lengths when closed.
- Structs and anonymous struct payloads match unquoted field names through a Schema field index; input fields may be in any order.
- Maps accept string keys only. A per-call AVL index detects duplicate decoded keys and emits entries using input slots; map semantics do not promise ordering.
- Enums resolve a declared variant and require its unit, single-value, or anonymous struct payload shape to match exactly.

Recursive Schema references remain nominal type IDs while parsing; each finite Config value is traversed as an ordinary value tree. Empty arrays and maps, `none`, and enum terminal variants can end a recursive value. Every actual child still consumes the existing depth, node, execution, and workspace budgets, and a failure returns no partial `ConfigValue`.

A struct frame tracks each Schema field as unseen, parsing, complete, failed, or deferred. At object close, it builds `FieldValue`s in Schema declaration order. An explicit value wins; otherwise the parser copies and validates a Schema default, uses `none` for an optional field without a default, or reports a missing required field. Explicit `none` is an explicit value and overrides an optional default.

### 3.3 Value origins

Each `ConfigValue` records a source category so diagnostics and later conversions can distinguish `ConfigLiteral`, `SchemaDefault`, `EnvironmentVariable`, `ExplicitFallback`, and `RustValue`. Successful environment resolution also stores a structured reference name and the already validated, typed scalar fallback when present. The provider's actual value is not part of this metadata. A reference record is shared by the option node and its present scalar child; copies and rebases preserve that sharing and charge reference text to the value budget. Foreign-value normalization rejects environment references that it cannot validate instead of silently reducing them to `RustValue`.

## 4. Environment variables

The parser resolves the environment only when it encounters an `env` call in a value position. The call must be on a scalar field or scalar optional field authorized by `@env`; the field's authorization name must exactly match the expression. Name syntax and authorization are checked before calling the provider, so unauthorized names, invalid positions, and invalid fallbacks do not trigger host reads.

Schema compilation assigns normalized slots to authorized names; identical names share a slot. During a configuration call, `EnvironmentCache` stores sparse entries only for names actually read, rather than preallocating for every Schema authorization. Entries are sorted by Schema slot and searched with binary search. Missing slots mean not yet read; inserted entries store unset state, copied raw bytes, or a provider failure. An unset value still occupies an entry. The environment-entry limit counts distinct authorized names actually read, and the environment-byte limit counts cached raw bytes.

Before a first read, the parser checks name length, authorization slot, cache hit, entry limit, capacity reservation, and insertion work. If a limit or capacity check fails, it does not call the provider. Returned bytes are then checked against string length and total environment-byte limits and copied into storage charged to the shared workspace. The cache entry is committed only after a complete copy. Cache searches and entry moves consume execution steps; entries and byte capacity consume shared workspace. The cache stores no field conversions: each field reference is converted and validated independently for its target type.

Provider `None` means unset; `Some(&[])` means set to an empty value. Returned bytes must be UTF-8. Strings retain their text, booleans accept only `true`/`false`, and numbers follow Config number syntax and target ranges. Provider errors, invalid UTF-8, and conversion or validation failures on a set value are errors and do not select a fallback.

When an expression has a fallback, the parser parses and validates it against the field's scalar type even if the environment is set. If the variable is unset, the explicit fallback wins and the resulting value has `ExplicitFallback` origin without an environment reference. Without a fallback, the field is deferred until object close, where normal Schema-default, optional, or required-field rules apply. Only a successfully converted and validated provider value retains the expression name and fallback for typed round trips.

The Schema-aware writer emits a retained reference as `env("NAME"[, fallback])`; a fallback-selected value is written as a scalar literal. Full value and Schema validation checks the reference name, scalar/option shape, exact field authorization, and typed fallback before the writer sends its first byte. `ConfigDocument` uses the same scalar encoding for replacements and insertions, then reparses and validates the candidate before committing it, so an invalid candidate leaves the retained source unchanged. Its formatter continues to preserve untouched source tokens and comments.

`typol_config::Environment` is independent of the process environment. The optional host adapter `typol_std::ProcessEnvironment` queries named variables without enumerating or modifying the environment. Unix bytes are checked as UTF-8 in the configuration layer; Windows values are converted to UTF-8 and isolated surrogate values are rejected.

## 5. Validation pipeline

### 5.1 Fields and local values

When a value is complete, the validator confirms its runtime shape matches the Schema type, then gathers use-site rules and rules inherited through optional wrappers from compiled Schema views. `ScalarValidator` checks scalar values using `SchemaRules` and precompiled regex programs. Runtime regex uses the call's `RegexExecutor` scratch space and budget; patterns are not recompiled.

Rules on optional values apply to present scalar/container content; `none` has no leaf value for scalar comparison. Nested values are checked at each target Schema type. Defaults and fallbacks also pass the same shape and constraint checks before entering the final tree. A validated fallback carries a per-call validation token; when inserted, its instance node is recorded without running the rules a second time. All Regex checks and environment fallbacks within one parse or validation call share that call's Regex budget; a new search still receives its own `max_search_steps` allowance.

### 5.2 Collection constraints

Arrays and maps enter `CollectionValidator` when their container completes. Size rules check element or entry count. Arrays can also check sums, uniqueness, uniqueness by field path, and `@all` expressions. `@all` visits the container once in source order and binds the current array item or map key/value directly to the expression. Errors point to the corresponding element or map entry. Runtime consumes expression IR and regex programs produced by Schema compilation.

### 5.3 Full-tree assertions

After the root object parses successfully and the remaining input reaches EOF, the parser walks the completed instance tree and evaluates struct and active enum-variant assertions. `InstanceWalk` uses explicit frames. Expressions receive the current instance as `self` and root object as `root`; anonymous struct variant payloads bind to the relevant payload fields. Assertions on unselected variants do not run.

If structured parsing, local validation, or EOF checking has an error, root assertions do not run and no partial `ConfigValue` is returned. `ConfigValueProcessor::validate` uses the same full-tree validation entry point after normalization, so foreign and Rust values cannot bypass Schema rules.

### 5.4 Result types for foreign values

`ConfigValueProcessor::normalize` and its budgeted variant return `NormalizedConfigValue`: the input tree matches Schema shape and defaults are filled, but this type does not certify constraints or full-tree assertions. `ConfigValueProcessor::validate`, its budgeted variant, `ConfigParser::parse`, and `ConfigParser::parse_with_budget` return `ValidatedConfigValue`, which certifies normalization, constraints, and full-tree assertions. Both wrappers privately own a `ConfigValue` and expose read-only `as_config_value()` or consuming `into_config_value()` accessors.

## 6. Error recovery and diagnostics

When parsing a struct field, array element, or map entry fails, the parser may seek the next comma or matching container terminator at the current container depth and continue collecting independent sibling errors. The recovery cursor tracks bracket, square-bracket, and brace depth. Recovery stops on a mismatched terminator, EOF, loss of a reliable separator, a fatal lexical event, or budget termination. A failed field remains in its slot, preventing duplicate input from being treated as a new field and avoiding a second missing-field error for that same field.

`ConfigParseErrors` is a non-empty owned collection with stage `Input` or `Parsing`. It stores neither source text nor raw environment bytes. Ordinary diagnostics retain structured codes, source spans, optional Schema labels, value origin, and a safely processed instance path. `span_location()` reads the location snapshot owned by the returned errors after input is dropped; `excerpts()` reads bounded, safe source excerpts. Source positions are recorded while scanning. Schema label positions and safe rule excerpts are copied from `CompiledSchema`. An emergency terminal error may not have a line/column or excerpt.

Before errors are frozen, ordinary diagnostics are sorted by source ID, start byte, and end byte of their primary span. If primary spans match or are absent, production order is retained. Terminal diagnostics, including a diagnostic-limit slot, are always last. Sorting consumes the shared execution budget; exhaustion is a resource error.

`Display` for `ConfigParseErrors` and `ConfigValueErrors` writes diagnostic reports with symbolic codes, reasons, locations, labels, and available excerpts. If collection hit its limit, the report says that later errors were not collected. Both types provide `is_truncated()`; `Debug` exposes stage, count, and truncation state. Formatting into `fmt::Formatter` does not allocate a report buffer, though `to_string()` allocates its returned string.

Diagnostics are collected under the caller's cap. A cap of zero is treated as effective capacity one. At capacity one, the first real diagnostic is preserved and collection stops with the truncated flag; it is not replaced by `DIAGNOSTIC_LIMIT_REACHED`. At capacity two or greater, the final slot is reserved and the limit diagnostic is added when more errors arrive. Use `is_truncated()` to detect early stop; with capacity one, do not assume the final diagnostic code is the limit code. Resource exhaustion, allocation/capacity failure, input failure, and internal invariant errors terminate parsing; lexical errors also stop later recovery. Public diagnostic categories include lexical, config syntax, config value, environment, config validation, resource, allocation, input, diagnostic limit, and internal errors. Stable codes are defined in `typol-core::DiagnosticCode`.

## 7. Resource and allocation boundaries

A Config call uses independent excerpt, Regex, Config, lexical-stream, and generic execution settings. `ConfigParseOptions` and `ConfigValidationOptions` expose `ExcerptLimits` and `RegexOptions` separately; reusing the generic `ExecutionBudget` never skips Regex accounting.

| Limit | Scope |
| --- | --- |
| `ExcerptLimits` | Total source-excerpt bytes, per-line bytes, record count, and context rows |
| `RegexOptions::limits()` | Structural Regex bounds: pattern bytes/count, nesting, instructions, character ranges, and scratch states |
| `RegexOptions::execution()` | Regex steps, per-search NFA states, and Regex-owned workspace, each defaulting to 67,108,864 |
| `ConfigLimits::max_source_bytes` | Maximum bytes read from one configuration source |
| `max_string_bytes` | Decoded strings, field/variant names, map keys, and environment text |
| `max_value_nodes` | Value nodes parsed or built during foreign-value normalization |
| `max_nesting_depth` | Value, recovery scan, normalization, instance walk, and expression depth; root is depth zero and the limit itself is allowed |
| `max_diagnostics` | Total ordinary and terminal Config diagnostics |
| `max_environment_entries` | Distinct authorized environment names cached in one call |
| `max_environment_bytes` | Total raw environment bytes cached in one call |
| `max_constructed_text_bytes` | Cumulative text constructed or copied during one call |
| `ConfigFormatOptions::max_output_bytes` | Formatted output bytes, also capped by the document source and constructed-text limits |
| `ConfigFormatOptions::max_workspace_bytes` | Logical formatter frame storage; actual allocations also count against shared execution workspace |
| `ConfigFormatOptions::max_nesting_depth` | Delimiter depth traversed by the formatter |
| `max_display_name_bytes` | Diagnostic display-name length |
| `StreamLimits::chunk_bytes` | Streaming window size; `try_new` rejects zero |
| `StreamLimits::max_token_bytes` | Source length of one token |
| `ExecutionLimits` | Cumulative steps and workspace bytes for non-Regex work in a call |

Current `ConfigLimits::default()` values include 1 MiB per source, 100 diagnostics, depth 128, 256 KiB strings, 100,000 value nodes, 1,024 environment entries, 8 MiB environment bytes, 16 MiB constructed text, and a 4,096-byte display name. Default stream limits are an 8,192-byte window and a 1 MiB token. Excerpt defaults come from `ExcerptLimits`; structural Regex defaults come from `RegexLimits`; Regex execution defaults come from `RegexExecutionLimits`; generic execution defaults come from `ConfigParseOptions::default()`. These are overridable runtime policies, not DSL semantics.

Each of the nine `ConfigLimits` fields has a named update method: `with_max_source_bytes`, `with_max_diagnostics`, `with_max_nesting_depth`, `with_max_string_bytes`, `with_max_value_nodes`, `with_max_environment_entries`, `with_max_environment_bytes`, `with_max_constructed_text_bytes`, and `with_max_display_name_bytes`. Unchanged limits retain defaults; zero retains each limit's own semantics.

Default excerpt limits are 65,536 total bytes, 512 bytes per line, 128 records, and two additional context rows per span. Set `ExcerptLimits` directly on the parse or validation options. A zero required capacity disables excerpts, not diagnostics. Candidate windows, masking ranges, and final excerpts have separate byte caps and use the generic session workspace budget; exhaustion or optional allocation failure only degrades excerpt status.

Value depth counts parent-child edges: the root is depth zero, and a present optional payload and single enum payload each add one level. Anonymous struct enum payloads add another level between the payload struct and its fields: for enum depth `d`, payload fields start at `d + 2`. Empty containers are checked at their own depth. Recovery scanning converts delimiter-stack limits from the root object's baseline rather than treating absolute delimiter count as value depth.

Direct `typol-core` callers that copy a `ValueView` with `ConfigValueBuilder::append_view` must provide `ValueCopyLimits`; the root is depth zero and the operation accepts values at the limit. Copying uses an explicit post-order frame stack, and frame and child storage uses fallible allocation charged to the builder's shared execution budget when present. This per-copy depth limit is separate from `ExecutionLimits`, which continues to cap cumulative steps and workspace bytes.

Dynamic workspace uses fallible allocation and is charged to the execution budget. Limit exhaustion, insufficient workspace, and allocation failure are distinct terminal errors. Parsing, normalization, and value-view copying use explicit frames, so their depth limits do not rely on the host call stack. Temporary values, caches, and workspace are released after failure; partial results are never published.

## 8. Secrets and sensitive data

`SecretPolicy` uses the compiled Schema, instance path, fields/variants, and alias information to find the outermost secret boundary. Before a diagnostic enters its buffer, sensitive messages are hidden and paths truncated; final error snapshots apply the policy again. Array indices, map keys, and payload suffixes beneath a secret subtree are not exposed. If a dynamic path cannot be safely mapped to the Schema, an internal diagnostic without the path terminates processing rather than leaking an unknown map key.

The same policy protects Rust conversion diagnostics from `TypedSchema::serialize` and `TypedSchema::deserialize` before they enter `CodecErrors`. Unmappable paths are cleared. If redaction budget or allocation fails, only a safe diagnostic without the original path is returned. Paths that never enter conversion diagnostics, such as ordinary rejection errors, do not expose values or field data.

Before a source line enters an error object, known secret ranges are masked byte-for-byte; secret map entries hide both keys and values. An unfinished secret value is masked through its input boundary. If range recording fails, a span is invalid, or excerpt capacity is exhausted, the affected excerpt is omitted rather than falling back to its raw line. Raw environment values never enter the source window. Default reports use a truncated display name and do not add real file paths or operating-system error text.

The generic `Debug` for `ConfigValue` reports kind, string/container lengths, and origin, not literal values. Requests, error collections, and outer value wrappers do not print input or error bodies. Secret markers protect diagnostics and safe display; they do not encrypt data in a successful `ConfigValue`. Serialization and application code can still access actual values.

Raw environment bytes are held only by the cache for one call and are not attached to diagnostics after the environment string/scalar is constructed. Consistency across environment reads is determined by the host provider; the process adapter does not promise one snapshot for different names.

## 9. Host integration

`SchemaLoader` owns the compiled Schema and source names. File input uses `typol_std::FileConfigInput` and is read on demand by the lexer; standard I/O reader input uses `ReadSource` to adapt `std::io::Read` to `typol_core::Read`. These adapters handle host I/O and bounded error categories, while `typol-config` implements syntax and Schema validation.

With the `config-ir` feature enabled, Rust callers can export Config IR v1 through `ConfigIr::serialize`; see the [Config IR specification](../config-ir/). The default mode fills defaults and performs full Schema validation. To skip constraint validation, callers must explicitly choose `AsProvided`. The facade handles type/value traversal, secret markers, deterministic ordering, output/sorting-workspace/depth limits, and serialization error categories. Output and sort-workspace failures are distinct. Maximum nesting depth is 128 and can be lowered but not raised. `typol-cli` has already validated values during configuration loading, so it encodes them as `AsProvided` to avoid repeating validation; it chooses the environment mode, loads the value, invokes the API, and writes the complete result to stdout. Its `config fmt` command instead loads a `ConfigDocument`, formats retained source, and atomically replaces the same file; `--check` compares the formatted text without writing. It never serializes environment results or default-filled values back into source. See the [CLI guide](../../guides/cli/) for command usage. `secret: true` still accompanies the actual value, which consumers must treat as sensitive.

`TypedSchema::deserialize` reuses configuration parsing and full validation before converting the owned value to Rust. The `typol` codecs and contextual Serde validate external/serialized values with `ConfigValueProcessor::validate_with_budget` before encoding or decoding; binding, parsing, and conversion may share an outer generic execution budget, while each validation operation creates its Regex budget from its options. Ordinary Serde traits without Schema context are outside this validation guarantee.

## 10. Acceptance and maintenance

Config implementation coverage lives in `crates/typol-config/src/parser/tests/`, `src/value_validation/tests.rs`, and `crates/typol-config/tests/`. Parser maintenance should cover stream chunking, lexical/syntax errors, nested recovery, default and optional completion, enum payloads, environment cache/fallback/conversion, scalar and collection rules, full-tree assertions, secret-path truncation, input-location snapshots, execution/workspace limits, and allocation-failure atomicity. Formatter coverage includes token/comment preservation, delimiter forms, env and enum expressions, LF/CRLF, trailing comments, idempotence, output/workspace limits, and no partial results. CLI tests cover atomic in-place replacement, idempotent reruns, read-only check mode, localized help and usage errors, and formatting failures. Tests in `typol` and `typol-std` cover file, reader, process-environment, binding, and safe-diagnostic integration.

If implementation behavior conflicts with the Config or Schema DSL, revise the relevant semantics specification or decision record first. A configuration parser or host adapter must not establish new DSL semantics privately.

## 11. Bounded diagnostic source excerpts

`ExcerptLimits` provides independent limits for total storage bytes, per-line bytes, record count, and context rows per span. Zero capacity disables excerpts. Each span selects a bounded window starting at its first line; a multi-line span extending beyond the window is marked truncated. Context rows are split before and after the span. Candidate windows, redaction buffers, and final excerpts each have byte caps and count against the generic call workspace; optional allocation failure does not change the business diagnostic. An owned `SourceExcerptSet` reports `Complete`, `Truncated`, or `Unavailable`. Missing records never fall back to reading or displaying raw source. Columns count Unicode scalar values, tabs count as one column, and CRLF is excluded from line contents. Truncation stays on UTF-8 boundaries and adds an ellipsis marker.

Streaming Config input keeps bounded candidates and a recent-line window, retaining window lines referenced by diagnostics until the call ends. Delayed diagnostics can use only retained candidates. On parse completion, only lines referenced by actual diagnostics and their context are published. Known secret values and secret map source ranges are masked; unfinished secret values are masked to end of input. Skipped recovery content and unbound values are hidden conservatively. If range recording fails, the excerpt is unavailable. Raw environment values never enter the line window. Schema excerpts are taken only while the source catalog is alive, with default-value spans masked, including defaults whose secret aliases have not yet resolved. If a complete AST is unavailable, the source excerpt is omitted. Errors and compiled artifacts do not retain full source, and new `Debug` implementations expose only status and counts.
