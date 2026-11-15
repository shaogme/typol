---
title: Config implementation
description: How Typol parses, constructs, validates, and safely reports .tconf values.
sidebar:
  order: 3
---

## Quick reference

:::tip[Use this page when]
You need to trace a configuration read through parsing, environment access, validation,
or resource limits.
:::

| Area | Current behavior |
| --- | --- |
| Parsing | Incremental token reads; no AST. Documents also keep bounded source and parser spans. |
| Environment | The caller's provider is queried only for authorized `env(...)` expressions. |
| Formatting | `ConfigFormatter` scans validated documents and retains source tokens and comments. |
| Limits and errors | Parsing and validation use resource budgets and structured failures. |

This page describes the current `.tconf` implementation: entry points, streaming input,
Schema-guided value construction, environment reads, validation, recovery, and safe diagnostics.
It supplements the [Config DSL](../config-dsl/) and
[Schema implementation](../schema-implementation/) pages without changing DSL semantics.

The behavior described here comes from the current source. The Config and Schema DSL pages
define language semantics; the source defines runtime resource defaults.

## 1. Scope and boundaries

Configuration handling is split across four layers:

1. `typol-syntax` reads UTF-8 bytes from `Read` on demand and emits tokens with source spans.
   It does not build a complete token array for configuration.
2. `typol-config` reads tokens against read-only `CompiledSchema` views and builds an owned
   `ConfigValue`. Its document entry point also retains the original text and locations from
   the same parse; it does not re-lex the source to find values.
3. `typol-semantic` supplies views for types, fields, variants, constraints, assertions,
   environment authorization, and compiled Regex programs, along with expression execution.
4. `typol` combines parsing with Rust conversion, Serde, loading sessions, and optional
   Config IR export. `typol-std` provides file input and process-environment adapters.

The core uses `no_std + alloc`. It does not open files, read process variables, or write to a
terminal. Environment access happens only through the caller's `Environment` implementation.
Successful values own their strings, keys, and data; they do not borrow input or provider buffers.

For application setup with the facade in a `no_std + alloc` environment, see the
[no_std guide](../../guides/no_std/).

| Responsibility | Main source locations |
| --- | --- |
| Public entry points, limits, and sessions | `typol-config/src/{api,options,session}.rs` |
| Parser and recovery | `typol-config/src/parser.rs`, `parser/` |
| Environment access and conversion | `typol-config/src/{environment,scalar}.rs` |
| Defaults and instance paths | `typol-config/src/{value_normalization,instance}.rs` |
| Validation | `typol-config/src/{validation.rs,validation/,value_validation.rs}` |
| Diagnostics and secret handling | `typol-config/src/{diagnostics,secret}.rs` |
| Documents and formatting | `typol-config/src/{document.rs,document/formatter/}` |
| Token stream and UTF-8 input | `typol-syntax/src/{input,lexer,cursor}.rs` |
| Value origins and execution budget | `typol-core/src/{value,budget}.rs` |
| Host adapters | `typol-std/src/{filesystem,environment}.rs` |
| Facade loading, binding, and codecs | `typol/src/{loading,binding_impl,codec}.rs` |

## 2. Entry points and sessions

### 2.1 Parsing configuration text

General callers use `ConfigParser::new(schema).parse(request, environment)`. Success returns
`ValidatedConfigValue`, which certifies that value constraints and full-tree assertions passed.
Callers can borrow its `ConfigValue`, or consume the marker with `into_config_value()`.

`ConfigParseRequest` contains a readable `Read`, display name, and `ConfigParseOptions`.
Options configure `ExcerptLimits`, `RegexOptions`, `ConfigLimits`, generic execution limits,
lexical `StreamLimits`, and a `Translator`. Foreign-value validation options also carry a
translator. The default language is English; select another language before parsing or validation.

Schema and Config share the block-string lexer rules. A `Read` source need not support replay,
so the lexer stages each block token body in charged workspace, then computes indentation and
decodes it into a second charged buffer. Scanning consumes the shared execution budget.

The source-token limit includes delimiters, closing indentation, and original CRLF bytes.
The string limit counts only the final decoded UTF-8 bytes. A display name is diagnostic
metadata and is not part of the configuration input.

`ConfigParser::parse_document` and `LoadedSchema::read_document` retain the parser's UTF-8
source buffer and semantic location index. The document adopts that buffer as its source text,
avoiding a second full-size copy. Before the first edit, the source itself also supplies the
`DocumentSnapshot` write expectation.

On the first successful edit, the previous source buffer becomes the charged snapshot without
another text allocation. A successful write advances the expectation to the current source and
releases that separate snapshot.

`ConfigDocument::write_to` writes the retained text exactly. `apply_edits` resolves typed
replacements, insertions, and removals through parser-produced locations. It groups index entries
by parent and prepares each touched container's bounds, item positions, commas, and layout once.

The editor rejects overlapping patches, assembles one candidate, then reparses and validates
it before committing the in-memory document. That parse records locations without retaining
another source copy; after validation, the candidate becomes the document's retained source.

Replacements encode the requested subtree. Insertions infer the container's line ending,
indentation, and key spacing, and write values directly into the final patch buffer. Map-entry
checks use the resolved parent node instead of cloning the complete path.

Removal and composite replacement preserve comment bytes from the touched range. Untouched
source ranges remain byte-for-byte unchanged. Edit counts, sorting, index traversal, and bounded
source scans consume the shared execution budget.

Batch related edits with one `apply_edits` or `replace_values` call so the candidate is parsed
and validated once. Separate `set_value` calls each incur that cost. The editor treats both
string forms as opaque spans, so delimiters and comment markers inside block strings are ignored.

`set_value` groups its path and value in `ConfigReplacement`; `insert_field` uses
`ConfigFieldInsertion`. The options default to `Compact`. `with_layout(TextLayout::Pretty)`
changes whitespace only in newly written value fragments; `remove` uses the default options.

Pretty layout preserves `ConfigValue` entry order and leaves the surrounding document alone.
Nonempty arrays, maps, and structures use one entry per line, four-space indentation, and trailing
commas. Empty containers stay inline. Single-value enum payloads use multiline parentheses
without a trailing comma; unit variants and `env(...)` stay compact.

New edit-fragment lines use CRLF if the source contains any CRLF, and LF otherwise. A fragment
has no final file newline. Replacement indentation starts with the target line's leading spaces
or tabs, plus four spaces for each earlier encoding container still open on that line.

New insertion lines use the container layout's indentation. Compact encoding and existing
outer insertion punctuation remain unchanged. The facade exports `ConfigEditOptions`; `TextLayout`
is available from `typol_config`, `typol`, `typol::core`, and `typol::config`.

Candidate text is subject to both `max_source_bytes` and `max_constructed_text_bytes`. Before
allocation, the editor charges the candidate's full length once; patch bytes are not charged a
second time. Parser-created text during validation follows the normal parser accounting path.

If candidate parsing reports allocation or resource/capacity diagnostics, the edit error keeps
that terminal category and retains the complete `ConfigParseErrors` collection.

`ConfigFormatter::format(&document, options, out)` formats only a successfully parsed document.
It scans retained source with an explicit delimiter stack; it does not build a configuration AST
or re-encode `ConfigValue`. `FormattedConfig` owns the complete result and exposes a read-only view.

`ConfigFormatter::is_formatted` runs the same formatter while comparing output chunks with the
source, without allocating a complete output buffer. The CLI uses it for `config fmt --check`.
Formatting leaves the document and its source locations unchanged.

Formatting preserves every non-whitespace token and comment in order, including string and
number spellings and `env(...)` expressions. A block string is one opaque token, so its bytes,
indentation, and LF/CRLF spelling stay unchanged. Formatting is idempotent.

Nonempty containers expand to one entry per line with four-space indentation and trailing
commas. Empty containers stay compact unless they contain comments. Single-value enum payloads
use multiline parentheses without a trailing comma; `env(...)` calls stay compact.

`ConfigFormatOptions` sets output-byte, formatter-workspace, and nesting limits. Formatting also
observes document source and constructed-text limits, and charges stack storage and scanning to
the shared execution budget. It has no style switches that create multiple canonical formats.

`format` builds and charges an owned output buffer. `is_formatted` uses the same output limit
while comparing chunks. Trailing-comment comma lookahead runs once per consecutive comment run,
so a run of `k` comments adds linear scan work.

The formatter returns an owned result only after scanning, bounds checks, and output accounting
succeed. Structure errors, resource exhaustion, allocation failure, and sink errors remain
distinct; an incomplete buffer is never returned as a successful result.

`LoadedSchema::read_file` returns a `ConfigDocument`. Use `read_file_value` when only the
validated-value result is needed. With `std`, `LoadedSchema::write_file` uses the document's
snapshot expectation and enforces source and file-input byte limits.

Local filesystems cannot atomically compare file bytes and replace a path, so the adapter returns
`AtomicReplaceUnsupported` for `ExactSnapshot`. Use `MustBeAbsent` for a new target or `ReplaceAny`
for an intentional overwrite. Unix creates new targets with a same-directory hard link; Windows
uses a same-volume no-replace write-through move.

Both new-target operations refuse a target created concurrently. `ReplaceAny` uses atomic
replacement. Unix syncs the parent directory after commit and reports when durability cannot be
confirmed; Windows uses a same-volume write-through move. A `no_std` caller implements
`SourceWriter` and provides the selected expectation semantics.

`ConfigValueProcessor::validate` and `normalize` accept a `ConfigValidationRequest`. Its options
configure excerpts, Regex, Config, and generic execution limits, but no token stream. Validation
and normalization read neither configuration input nor an environment provider.

`TypolOptions` and `SchemaLoader::new(TypolOptions)` are shared by `no_std` and `std` builds.
`LoadedSchema::read_value` and `BoundSchema` reads accept `ReadRequest<R>`, whose reader follows
the `typol_core::Read` contract used by the lexer.

In-memory text can use `ReadRequest::from_text`; `no_std` callers can pass `SliceSource` or a
custom reader. Every read receives an explicit `Environment` provider. With `std`, `read_file`
uses `FileConfigInput`, `read_io` adapts `std::io::Read` through `ReadSource`, and
`SchemaLoader::compile_file` adapts file Schemas into the same compiler flow.

To read process variables, pass a `ProcessEnvironment` configured with `ProcessEnvironmentLimits`.
Those limits cover names and values, but the value limit is checked after `var_os` returns an
owned `OsString`; it cannot bound that lookup's temporary allocation.

With `std`, `TypolOptions` also carries host limits. Configure file input with
`with_file_input_limits`; use `with_files`, `with_names`, and `with_name_edits` for file Schemas,
source names, and name edits. `with_config_limits` and `with_binding_limits` adjust parsing and
binding limits.

Configuration policies are available from `typol::config`; host adapters are in `typol::host`.
File input limits cover configuration files and path bytes. Schema imports use `FileSchemaLimits`,
while execution limits remain in Schema and Config options.

`ConfigLimits` has finite defaults and named, chainable `with_max_*` updates. Its fields are
private, it has no positional constructor or mutable setters, and read-only getters expose values.
Pass it to `ConfigParseOptions::with_config`. Codec options also use named updates.

`ConfigParser::parse` creates a generic `ExecutionBudget`. Use `parse_with_budget` to share its
non-Regex capacity with an outer load, bind, or codec operation. The supplied limits must exactly
match the request's generic execution limits, or parsing returns an internal configuration error.

Parsing, environment handling, validation, and diagnostic path processing share that generic
budget. Regex checks share a separate `RegexExecutionBudget` created from
`ConfigParseOptions::regex()`; Regex compilation happens during Schema compilation.

### 2.2 Validating foreign values

`ConfigValueProcessor::validate` accepts an owned `ConfigValue`, copies and normalizes it against
the Schema, then applies the same constraints and assertions as text parsing. It has no
`Environment` parameter and cannot read process variables.

Use `validate_with_budget` to share generic execution capacity. Regex checks still use the
separate budget from `ConfigValidationOptions::regex()`. Normalize-only operations do not create
or consume a Regex budget.

Normalization checks value shapes, field names, enum payloads, and scalar targets. It rebuilds
structures in Schema field order, fills Schema defaults and optional fields without defaults,
and creates a new owned tree.

Missing required fields, unknown or duplicate fields, invalid map keys or values, and incorrect
payload shapes fail. Maps have no DSL ordering; the foreign-value path sorts keys for stable
reconstruction.

Normalize-only operations reject environment-origin values because they do not perform complete
authorization and fallback checks. Use `validate` when a foreign value must retain an environment
origin.

### 2.3 Generating configuration from Schema defaults

`LoadedSchema::default_config_to_string` and `write_default_config` generate `.tconf` from the
compiled entry Schema without binding a Rust type or receiving an `Environment`. The facade
creates an empty structure root and validates it once through `ConfigValueProcessor`.

That validation fills defaults and checks field rules, collection rules, and full-tree assertions.
Generation errors use `TypolError` at stage `Encoding`; value diagnostics remain available through
`codec_errors().config_value_errors()`.

For each missing field, completion selects an explicit Schema default, then `none` for an
`option<T>` without a default, or a required-field diagnostic with the field path and Schema
definition location. Only the entry structure is synthesized as the root.

A nested structure, array, map, or enum is not created just because its contents look constructible;
the field needs an explicit default. A selected nested object or enum payload is then completed
recursively. `@env` authorizes input expressions, but generation never evaluates `env(...)`.

`DefaultConfigOptions` defaults to `Compact`, a 1 MiB UTF-8 byte limit, and permission to output
actual `@secret` values. `with_layout(TextLayout::Pretty)` selects the canonical four-space layout
used by `config fmt`, with multiline compound values and one final LF.

The removed `with_pretty(bool)` and `pretty()` methods were a source-breaking API change. Use
`with_layout(TextLayout)` and `layout()`. Options can also reject values marked secret; this check
finishes before the first sink write and returns an encoding diagnostic without value text.

Allowed secret values remain plaintext and must be protected as sensitive configuration. The
string API returns only after complete validation and encoding. The sink API may have written a
prefix if encoding, I/O, budget, or output limits fail.

For atomic file creation or replacement, generate the complete string and pass its bytes to
`FileSourceWriter::replace_atomically` with an explicit `SourceExpectation`.
`LoadedSchema::write_file` operates on a `ConfigDocument` snapshot, not generated defaults.

## 3. Streaming parse and value construction

### 3.1 Input and root value

The parser allocates a `SourceId`, starts at `schema.entry()`, and confirms that the root type is
a structure. The lexer reads through a fixed-size window from `Read`; the token cursor caches the
current event and at most one lookahead. Input tracks line and column positions, while token spans
use UTF-8 byte offsets.

The root is parsed as a structure. After it closes, the parser reads to EOF; any extra token makes
the input invalid. A call therefore accepts one root object followed only by DSL whitespace and
comments. Tokens are consumed into the value tree, and no configuration AST is retained.

### 3.2 Interpreting tokens by Schema type

`ValueParser` uses an explicitly budgeted frame stack, so input nesting does not use host
recursion. Each frame stores the current Schema type and continuation state for a container or
payload.

- A scalar consumes one literal of the exact built-in Schema type. Strings use decoded content,
  integers are checked against target width, and floats must remain finite after conversion.
- `option<T>` accepts `none` or parses and wraps a `T`. No other implicit conversion occurs.
- Arrays build elements in input order and check fixed lengths when closed.
- Structures and anonymous structure payloads match unquoted field names through a Schema
  field index; input fields may appear in any order.
- Maps accept string keys only. A per-call AVL index detects duplicate decoded keys and emits
  entries using input slots; map semantics do not promise ordering.
- Enums resolve a declared variant and require its unit, single-value, or anonymous structure
  payload shape to match exactly.

Recursive Schema references remain nominal type IDs during parsing, while each finite Config
instance is traversed as an ordinary value tree. Empty arrays and maps, `none`, and terminal enum
variants can end a recursive value.

Every actual child still consumes depth, node, execution, and workspace budgets. A parsing error
never returns a partial `ConfigValue`.

A structure frame tracks each Schema field as unseen, parsing, complete, failed, or deferred.
At object close, it builds `FieldValue`s in Schema declaration order. Explicit values win; otherwise
the parser validates a Schema default, uses `none` for an optional field without a default, or
reports a missing required field.

Explicit `none` is an explicit value and overrides an optional field default.

### 3.3 Value origins

Each `ConfigValue` records one source category: `ConfigLiteral`, `SchemaDefault`,
`EnvironmentVariable`, `ExplicitFallback`, or `RustValue`. These origins help diagnostics and
later conversions identify how a value was produced.

Successful environment resolution also stores a structured reference name and any validated,
typed scalar fallback. The provider's actual value is not kept in this metadata. The option node
and its present scalar child share the reference record.

Copies and rebases preserve that sharing and charge reference text to the value budget.
Foreign-value normalization rejects environment references it cannot validate rather than
silently changing their origin to `RustValue`.

## 4. Environment variables

The parser resolves the environment only when it encounters an `env` call in a value position.
The call must appear on a scalar field or scalar optional field authorized by `@env`, and the
authorization name must exactly match the expression.

Name syntax and authorization are checked before the provider is called. An unauthorized name,
invalid position, or invalid fallback therefore does not trigger a host read.

Schema compilation assigns normalized slots to authorized names; identical names share a slot.
During each Config call, `EnvironmentCache` stores sparse entries only for names actually read.
Entries are sorted by Schema slot and searched with binary search.

An entry records an unset value, copied raw bytes, or a provider failure. Missing values still
occupy an entry. The entry limit counts distinct authorized names read; the byte limit counts raw
bytes retained in the cache.

Before a first read, the cache checks name length and authorization, then checks the entry limit,
reserves an entry, and charges insertion work. A quota or capacity failure occurs before the
provider is called.

After the provider returns, the cache checks string length and cumulative environment bytes,
then copies bytes into charged storage. It commits the entry only after the copy succeeds.
Cache lookup and entry movement consume steps; entry and byte capacity use shared workspace.

The cache does not store field conversions. Each field using a cached name converts the raw value
to its own target type and applies its own constraints.

`None` from a provider means unset; `Some(&[])` means set to an empty value. Returned bytes must
be valid UTF-8. Strings keep their text, booleans accept only `true` or `false`, and numbers follow
Config numeric syntax and the target range.

A provider error, invalid UTF-8, or conversion/validation failure for a set value is an error.
These cases do not fall back to the expression's fallback.

An explicit fallback is parsed and validated as the field's scalar type even when the environment
is set. If the variable is unset, that fallback wins and the value origin is `ExplicitFallback`;
the result does not retain an environment reference.

Without an explicit fallback, an unset variable defers the field. Object completion then applies
the usual default, optional-value, or required-field rule. A successful provider value retains the
expression name and validated fallback for typed round trips.

A Schema-aware writer emits retained references as `env("NAME"[, fallback])` and emits a chosen
fallback as a scalar literal. Before writing its first byte, it checks the reference name, scalar
or option shape, exact field authorization, and typed fallback.

`ConfigDocument` edits share the writer's scalar and token rules. Their optional Pretty layout
uses edit-fragment whitespace rules. A candidate is reparsed and validated before commit, so an
invalid edit leaves the retained document unchanged. The formatter preserves untouched tokens
and comments.

`typol_config::Environment` is independent of process variables. The optional
`typol_std::ProcessEnvironment` adapter queries named variables without enumerating or modifying
the environment. Unix bytes are checked as UTF-8 by Config; Windows converts to UTF-8 and rejects
unpaired surrogates.

## 5. Validation pipeline

### 5.1 Fields and local values

When a value is complete, the validator confirms that its runtime shape matches its Schema type.
It then gathers use-site rules and rules inherited through optional wrappers from compiled
Schema views.

`ScalarValidator` checks scalar values with `SchemaRules` and precompiled Regex programs. Runtime
Regex uses the call's `RegexExecutor` scratch space and execution budget; patterns are not
recompiled.

Rules on optional values apply to present scalar or container content. `none` has no leaf value
for scalar comparisons. `@variants` resolves present enums to compiled `VariantId`s and checks
allow/deny lists; `none` satisfies a valid option restriction.

Nested values are checked at each target Schema type, so rules on an array element, map value, or
enum payload apply only to that node. Defaults and fallbacks pass the same shape and constraint
checks before entering the final tree.

A validated fallback carries a per-call validation token. When inserted, its instance node is
recorded without running its rules again.

### 5.2 Collection constraints

Arrays and maps enter `CollectionValidator` when their containers complete. Size rules check item
counts; arrays can also check sums, uniqueness, uniqueness by field path, and `@all` expressions.

`@all` visits a container once in source order and binds the current array item or map key/value
directly to the expression. Errors point to the relevant element or map entry. Runtime consumes
expression IR and Regex programs created during Schema compilation.

### 5.3 Full-tree assertions

After the root parses and the remaining input reaches EOF, the parser walks the completed instance
tree and evaluates structure and active enum-variant assertions. `InstanceWalk` uses explicit
frames. Expressions receive the current instance as `self` and the root object as `root`.

For anonymous structure variant payloads, `self` is bound to the payload fields. Assertions on
unselected variants do not run.

If parsing, local validation, or EOF checking reports an error, root assertions do not run and no
partial `ConfigValue` is returned. `ConfigValueProcessor::validate` uses the same full-tree
validation entry point after normalization, so foreign and Rust values cannot bypass Schema rules.

### 5.4 Result types for foreign values

`normalize` and `normalize_with_budget` return `NormalizedConfigValue`. It certifies Schema shape
and default completion, but not constraints or full-tree assertions.

`ConfigValueProcessor::validate`, `validate_with_budget`, `ConfigParser::parse`, and
`parse_with_budget` return `ValidatedConfigValue`, which certifies normalization, constraints,
and full-tree assertions.

Both wrappers privately own a `ConfigValue` and expose read-only `as_config_value()` or consuming
`into_config_value()` accessors. `ValidatedConfigValue` also retains an opaque identity for the
compiled Schema used during validation; `belongs_to_schema()` checks that association.

## 6. Error recovery and diagnostics

When a structure field, array element, or map entry fails, the parser may seek the next comma or
matching container terminator at the current depth, then report independent sibling errors.
Recovery tracks bracket, square-bracket, and brace depth.

Recovery stops on a mismatched terminator, EOF, loss of a reliable separator, a fatal lexical
event, or budget termination. A failed field keeps its slot, so duplicate input is not mistaken
for a new field and the same field is not reported missing again.

`ConfigParseErrors` is a nonempty owned collection at stage `Input` or `Parsing`. It stores
neither source text nor raw environment bytes. Ordinary diagnostics retain structured codes,
source spans, optional Schema labels, value origin, and a safely processed instance path.

`span_location()` reads the returned error's location snapshot after input is dropped.
`excerpts()` reads bounded, safe source excerpts. Source positions are recorded while scanning;
Schema label positions and safe rule excerpts are copied from `CompiledSchema`.

An emergency terminal error may have no line, column, or excerpt. Before errors are frozen,
ordinary diagnostics sort by primary-span source ID, start byte, and end byte. Equal or missing
spans retain production order; terminal diagnostics, including a limit slot, remain last.

Sorting consumes the shared execution budget. Exhaustion is reported as a resource error.

`Display` for `ConfigParseErrors` and `ConfigValueErrors` writes reports with symbolic codes,
reasons, locations, labels, and available excerpts. Truncated collections say that later errors
were not collected. Both types expose `is_truncated()`; `Debug` shows stage, count, and truncation.

Formatting into `fmt::Formatter` does not allocate a report buffer. `to_string()` still allocates
its returned `String`.

Each diagnostic exposes a stable `kind().id()`, its stage, semantic `related_locations()`, and
statuses for primary span, instance path, and value origin. The facade's `TypolError` provides
indexed `diagnostic_count()`, `diagnostic(index)`, and `diagnostic_context(index)` queries in both
`std` and `no_std` builds.

Statuses distinguish inapplicable fields from unavailable or redacted data. `Diagnostic::message()`
returns the already translated reason as a fallible result; translation errors propagate instead
of being replaced with fallback text. The report writer cannot change the reason's language.

Diagnostics are collected under the caller's cap. A cap of zero is treated as capacity one.
At capacity one, the first real diagnostic is preserved and collection stops with truncation;
it is not replaced by `DIAGNOSTIC_LIMIT_REACHED`.

At capacity two or greater, the final slot is reserved and receives `DIAGNOSTIC_LIMIT_REACHED`
when more errors arrive. Use `is_truncated()` to detect early stop; at capacity one, the final
diagnostic code is not necessarily the limit code.

Resource exhaustion, allocation/capacity failure, input failure, and internal invariant errors
terminate parsing. Lexical errors also stop later recovery. Public diagnostic categories include
lexical, Config syntax/value/environment/validation, resource, allocation, input, diagnostic limit,
and internal errors. Stable codes are defined in `typol_core::DiagnosticCode`.

## 7. Resource and allocation boundaries

A Config call has independent excerpt, Regex, Config, lexical-stream, and generic execution
settings. Parse and validation options expose `ExcerptLimits` and `RegexOptions` separately;
reusing the generic `ExecutionBudget` does not skip Regex accounting.

| Limit | Scope |
| --- | --- |
| `ExcerptLimits` | Total excerpt bytes, per-line bytes, record count, and context rows |
| `RegexOptions::limits()` | Pattern size/count, nesting, instructions, ranges, and scratch states |
| `RegexOptions::execution()` | Regex steps, search-state and workspace caps; each is 67,108,864 |
| `ConfigLimits::max_source_bytes` | Maximum bytes read from one configuration source |
| `max_string_bytes` | Decoded strings, field/variant names, map keys, and environment text |
| `max_value_nodes` | Values parsed or built during foreign-value normalization |
| `max_nesting_depth` | Value, recovery, normalization, walk, and expression depth; root is zero |
| `max_diagnostics` | Total ordinary and terminal Config diagnostics |
| `max_environment_entries` | Distinct authorized names cached in one call |
| `max_environment_bytes` | Raw environment bytes cached in one call |
| `max_constructed_text_bytes` | Text constructed or copied during one call |
| Formatter output | Output bytes, also bounded by source and constructed-text limits |
| Formatter workspace | Frame limit; allocations also count toward shared workspace |
| Formatter nesting | Maximum delimiter depth |
| `max_display_name_bytes` | Diagnostic display-name length |
| `StreamLimits::chunk_bytes` | Streaming window size; `try_new` rejects zero |
| `StreamLimits::max_token_bytes` | Source length of one token |
| `ExecutionLimits` | Cumulative steps and workspace for non-Regex work in one call |

Current `ConfigLimits::default()` values include 1 MiB per source, 100 diagnostics, depth 128,
256 KiB strings, 100,000 value nodes, 1,024 environment entries, 8 MiB environment bytes,
16 MiB constructed text, and a 4,096-byte display name. Default stream limits are an 8,192-byte
window and a 1 MiB token.

Excerpt defaults come from `ExcerptLimits`; structural Regex defaults come from `RegexLimits`;
Regex execution defaults come from `RegexExecutionLimits`; generic execution defaults come from
`ConfigParseOptions::default()`. These are overridable runtime policies, not DSL semantics.

The nine `ConfigLimits` fields have named update methods: `with_max_source_bytes`,
`with_max_diagnostics`, `with_max_nesting_depth`, `with_max_string_bytes`,
`with_max_value_nodes`, `with_max_environment_entries`, `with_max_environment_bytes`,
`with_max_constructed_text_bytes`, and `with_max_display_name_bytes`.

Unchanged limits keep their defaults. A zero value retains the semantics of that individual limit.

Default excerpts allow 65,536 total bytes, 512 bytes per line, 128 records, and two additional
context rows per span. Set `ExcerptLimits` on parse or validation options. A zero required capacity
disables excerpts, not diagnostics.

Candidate windows, masking ranges, and final excerpts have separate byte caps and use the generic
session workspace budget. Exhaustion or optional allocation failure degrades excerpt availability.

Value depth counts parent-child edges: root depth is zero, and present optional payloads and
single enum payloads add one level. Values at the configured depth limit are allowed. An anonymous
structure enum payload adds a level between the payload and its fields, so fields start at depth
`d + 2` for an enum at `d`.

Empty containers are checked at their own depth. Recovery scanning converts delimiter-stack
limits from the root object's baseline instead of treating absolute delimiter count as value depth.

Direct `typol-core` callers copying a `ValueView` with `ConfigValueBuilder::append_view` must
provide `ValueCopyLimits`. Root depth is zero, and values at the limit are allowed. Copying uses
an explicit post-order frame stack with fallible allocation.

Frame and child storage count against the builder's shared execution budget when present. This
per-copy depth limit is separate from `ExecutionLimits`, which caps cumulative steps and workspace.

Dynamic workspace uses fallible allocation and is charged to execution. Limit exhaustion,
insufficient workspace, and allocation failure are distinct terminal errors.

Parsing, normalization, and value-view copying use explicit frames, so their depth limits do not
depend on the host call stack. Temporary values, caches, and workspaces are released after failure;
partial results are never published.

## 8. Secrets and sensitive data

`SecretPolicy` uses the compiled Schema, instance path, fields, variants, and aliases to find the
outermost secret boundary. Before a diagnostic enters its buffer, sensitive messages are hidden
and paths are truncated. Final error snapshots apply the policy again.

Array indices, map keys, and payload suffixes beneath a secret subtree are not exposed. If a
dynamic path cannot be mapped safely to the Schema, an internal diagnostic without that path
terminates processing rather than leaking an unknown map key.

The same policy protects Rust conversion diagnostics from `TypedSchema::serialize` and
`TypedSchema::deserialize` before they enter `CodecErrors`. Unmappable paths are cleared. If
redaction budget or allocation fails, only a safe diagnostic without the original path is returned.

Before a source line enters an error object, known secret ranges are masked byte-for-byte; secret
map entries hide keys and values. An unfinished secret value is masked through its input boundary.

If range recording fails, a span is invalid, or excerpt capacity is exhausted, the affected
excerpt is omitted instead of exposing the raw line. Raw environment values never enter the
source window. Default reports use a truncated display name, not a real file path or OS error text.

`ConfigValue`'s generic `Debug` reports value kind, string/container lengths, and origin, not
literal values. Requests, error collections, and outer value wrappers do not print input or error
bodies.

Secret markers protect diagnostics and safe display; they do not encrypt a successful
`ConfigValue`. Serialization and application code can still access its actual values.

Raw environment bytes are held only in the per-call cache and are not attached to diagnostics
after a string or scalar is constructed. Consistency across reads is determined by the host
provider; the process adapter does not promise one snapshot across different names.

## 9. Host integration

`SchemaLoader` owns the compiled Schema and source names. File input uses
`typol_std::FileConfigInput` and is read on demand by the lexer. Standard I/O readers use
`ReadSource` to adapt `std::io::Read` to `typol_core::Read`.

These adapters handle host I/O and bounded error categories; `typol-config` implements Config
syntax and Schema validation.

With the `config-ir` feature, Rust callers can export Config IR v1 through `ConfigIr::serialize`;
see the [Config IR specification](../config-ir/). Raw `ConfigValue` input receives defaults and
full Schema validation before encoding.

Callers holding a `ValidatedConfigValue` can use `ConfigIr::serialize_validated` to skip a second
validation walk. Its credential must belong to the same compiled Schema. The facade handles type
and value traversal, secret markers, deterministic ordering, resource limits, and error categories.

Output and sort-workspace failures have distinct categories. Maximum nesting depth is 128 and
can be lowered through `ConfigIrOptions`, but not raised. `typol-cli` passes the validation
credential from configuration loading to this fast path and writes the complete result to stdout.

The CLI's `config fmt` command instead loads a `ConfigDocument`, formats retained source, and
atomically replaces the same file. `--check` compares formatted text without writing. Formatting
does not serialize environment results or default-filled values back into source.

See the [CLI guide](../../guides/cli/) for command usage. Config IR's `secret: true` marker
accompanies the actual value, which consumers must still treat as sensitive.

`TypedSchema::deserialize` reuses configuration parsing and full validation before converting
the owned value to Rust. The `typol` codecs and contextual Serde validate external or serialized
values with `ConfigValueProcessor::validate_with_budget` before encoding or decoding.

Binding, parsing, and conversion may share an outer generic execution budget. Each validation
operation creates its Regex budget from its own options. Ordinary Serde traits without Schema
context are outside this validation guarantee.

## 10. Acceptance and maintenance

Config coverage lives in `crates/typol-config/src/parser/tests/`,
`src/value_validation/tests.rs`, and `crates/typol-config/tests/`. Parser maintenance should cover
streaming, lexical and syntax errors, nested recovery, default completion, and enum payloads.

It should also cover environment caching, fallback and conversion; scalar and collection rules;
full-tree assertions; secret-path truncation; location snapshots; execution/workspace limits; and
allocation-failure atomicity.

Formatter coverage includes token/comment preservation, delimiters, `env` and enum expressions,
LF/CRLF, trailing comments, idempotence, output/workspace limits, and failure without partial
results. CLI coverage includes atomic replacement, idempotent reruns, read-only check mode,
localized help, usage errors, and formatting failures.

Tests in `typol` and `typol-std` cover files, readers, process environment, type bindings, and safe
diagnostic integration.

If implementation behavior conflicts with the Config or Schema DSL, revise the relevant semantics
specification or decision record first. A parser or host adapter must not establish new DSL
semantics on its own.

## 11. Bounded diagnostic source excerpts

`ExcerptLimits` independently bounds total storage bytes, bytes per line, record count, and
context rows per span. Zero capacity disables excerpts. Each span selects a bounded window from
its first line; a multi-line span extending beyond the window is marked truncated.

Context rows are split before and after the span. Candidate windows, redaction buffers, and final
excerpts each have byte caps and count against the generic call workspace. Optional allocation
failure does not change the business diagnostic.

An owned `SourceExcerptSet` reports `Complete`, `Truncated`, or `Unavailable`. Missing records
never fall back to reading or displaying raw source. Columns count Unicode scalar values, tabs
count as one column, and CRLF is excluded from line contents.

Truncation stays on UTF-8 boundaries and adds an ellipsis marker. Streaming Config input keeps
bounded candidates and a recent-line window, retaining lines referenced by diagnostics until the
call ends. Delayed diagnostics can use only retained candidates.

At parse completion, only lines referenced by actual diagnostics and their context are published.
Known secret values and secret map source ranges are masked; unfinished secret values are masked
to end of input. Skipped recovery content and unbound values are hidden conservatively.

If range recording fails, excerpts are unavailable. Raw environment values never enter the line
window. Schema excerpts are captured only while the source catalog is alive, with default-value
spans masked, including defaults whose secret aliases have not yet resolved.

If a complete AST is unavailable, the Schema excerpt is omitted. Errors and compiled artifacts do
not retain full source, and new `Debug` implementations expose only status and counts.
