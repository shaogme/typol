---
title: Schema implementation
description: How Typol loads, compiles, and consumes .tschem Schemas.
sidebar:
  order: 4
---

## Quick reference

:::tip[Use this page when]
You need to understand Schema loading, compile diagnostics, imported files, or how
compiled types reach Config parsing and Rust binding.
:::

| Stage | Current behavior |
| --- | --- |
| Input | A streaming lexer and parser produce an owned Schema AST with source spans. |
| Compilation | The loader resolves imports, checks semantics, and builds an immutable Schema. |
| Consumers | Config parsing and Rust binding read compiled views; host access stays in adapters. |

This page describes the current implementation of `.tschem`, from source input to
compiled Schema and its use by Config parsing and Rust bindings. The [Schema DSL
specification](../schema-dsl/) defines language behavior and remains normative.

## 1. Scope and architecture

Schema processing has four main layers:

1. `typol-syntax` lexes UTF-8 input and parses it into an owned AST.
2. `typol-schema` loads the root and imported sources, resolves declarations, and compiles them.
3. `typol-semantic` stores the immutable compiled model and exposes read-only views.
4. `typol-config`, `typol`, and `typol-std` parse values, coordinate application APIs,
   and provide host adapters.

Core syntax and semantic crates use `no_std + alloc`. Schema compilation receives a
`SchemaResolver` instead of accessing files, while Config parsing builds values
directly without a separate Config AST. `typol-macros` derives Rust type descriptions
without reading Schema files; `TypedSchema::bind` checks compatibility at runtime.

For application setup without the standard library, see the
[no_std guide](../../guides/no_std/).

| Responsibility | Main source locations |
| --- | --- |
| Lexer, token cursor, Schema AST/parser | `crates/typol-syntax/src/` |
| Imports, modules, symbols, and type resolution | `typol-schema`: loader/, module_tree.rs, symbols.rs, types.rs |
| Annotations, expressions, defaults, and records | `typol-schema`: annotations/, expressions/, defaults/, compiled/ |
| Compiled model and semantic views | `typol-semantic`: model, views, constraints, expressions |
| Config parsing and validation | `typol-config`: parser.rs, validation.rs, value_validation.rs |
| Host loading and Rust binding | `typol-std`: schema_compiler.rs; `typol`: loading/, binding_impl.rs |

## 2. Source, lexer, and parser

### 2.1 Streaming input

Schema input must be valid UTF-8, and its lexical rules are defined by
[Schema DSL §1](../schema-dsl/#1-lexical-rules). The `Read` interface supplies bytes
sequentially: a short read is not EOF, and zero bytes means EOF. Parsing does not
require replayable input or retain the complete source.

The lexer scans on demand instead of building a token array. `TokenCursor` caches at
most the current event and one lookahead, and each token carries a `SourceSpan` with
a source ID and UTF-8 byte range. Identifier and number tokens can expose their
source text; string tokens contain decoded text, and EOF has an empty span.

Token and AST text use owned, budgeted storage. Block strings stage only the current
token body, since an input source may not be replayable; staging and decoded output
are charged while both are live. Token limits count delimiters and source line
endings, while string limits apply to the decoded result.

The shared lexer reads through a reusable window and does not expose stable slices
into source text. Debug output for tokens and AST nodes avoids literal values and
shows only metadata such as kind, span, and length.

### 2.2 AST and parse results

`SchemaParser::parse` accepts a byte source, `SchemaParseOptions`, and an
`ExecutionBudget`. `parse_str` adapts a string to a `SliceSource`; use
`parse_with_positions` when later location lookup is needed. Its result owns both
the AST and a `SourcePositionIndex` of byte offsets, line numbers, and Unicode
scalar columns, without retaining source bytes.

`SchemaParseOptions::with_translator` selects the language of syntax diagnostics.
Compilation accepts the same setting through `SchemaCompileOptions`; tools that
parse and compile separately should use the same translator. English is the default,
and later report writers do not translate a diagnostic again.

The parser reads until EOF and uses the confirmed byte length for the AST span.
Success returns a complete AST; lexical or syntax errors return non-empty
`SyntaxErrors`, never a partial AST. At reliable declaration, module, field, and
variant boundaries, it recovers to collect independent errors.

Recovery and token access share the parser's execution budget. Resource exhaustion,
allocation failure, and internal errors stop parsing. AST nodes are read-only views
owned by their AST, so callers cannot forge node IDs or storage tables.

The AST preserves source order and spans, type paths and alias targets, annotation
arguments, all enum payload forms, and the original numeric text. It also retains
whether brace-literal keys were identifiers or strings, plus expression structure,
paths, method calls, and closure parameters.

Parsing checks syntax only. It does not resolve names, check type compatibility or
annotation applicability, or execute expressions. An empty `{}` remains an
unclassified brace literal until compilation determines whether its target is an
object or a map.

### 2.3 Limits and parse-only documents

`SchemaLimits::syntax()` supplies parse depth, AST node, decoded string, source-byte,
and diagnostic limits. `SchemaLimits::stream_limits()` sets the lexer window and
per-token byte cap; `StreamLimits::try_new()` rejects a zero-sized window. The
loader also caps total source bytes and file count, and successful compilation
requires every source to reach EOF.

The default token window is 8,192 bytes and its token cap is 1 MiB. The default
syntax limits include depth 128, 100 diagnostics, 256 KiB decoded strings, and
100,000 AST nodes per file. Counts include nodes later discarded during recovery.
Callers can inspect `SchemaAst::node_count()` and `structure_height()`.

Tools that need to retain original text can call `SchemaParser::parse_document`.
It returns a `SchemaDocument` borrowing the exact input string and owning the
successful AST. This path does not resolve imports or check static semantics; the
compiler's streaming path still does not retain the complete source.

`SchemaFormatter::format` consumes a document and formatting options, then returns
a complete `FormattedSchema`. It uses AST spans to recognize delimiters while
preserving tokens and comments. It standardizes layout and trailing commas, keeps
block-string bytes intact, and emits CRLF if the input uses CRLF, otherwise LF.

Formatter workspace, output size, and nesting depth have separate limits.
`FormattedSchema::as_str()` provides read access; `write_to()` writes the completed
result to a `fmt::Write` sink.

## 3. Compilation sessions and imports

### 3.1 Entry points

Create a one-shot compile session with `SchemaCompiler::new(SchemaCompileOptions)`.
Call `compile(root, resolver, observer)` when imports are possible, or
`compile_without_imports(root)` for an in-memory source that must not import other
files. The observer variant registers source names for the no-import path.

`SchemaSource::from_source` accepts a single-use byte reader, while `from_str`
adapts an in-memory string. Source identities compare byte-for-byte and are not
stored in the compiled Schema. A source explicitly tied to another compile context
fails with `SCHEMA_CONTEXT_MISMATCH`; using an ended context fails with
`SCHEMA_SESSION_FINISHED`.

`SchemaResolver::resolve` receives the current source identity, one module name,
the remaining source-byte allowance, and the compile context. The host decides how
module names map to files or memory. `SourceObserver` can register each loaded
identity with its `SourceId` for host-readable names.

`typol_std::FileSchemaCompiler` provides file loading and relative imports. It keeps
file handles open while the parser reads source bytes on demand. The application
facade offers `SchemaLoader::compile_file`, `compile_text`, and
`compile_with_resolver`; a resulting `LoadedSchema` can be bound once and reused
for multiple Config reads.

### 3.2 Import graph

The root source receives `SourceId(0)`; later sources receive IDs in load order.
Imports are read depth-first with an explicit stack. Each import edge records its
state and optional target, so a failed edge never points to a valid source.

An identity already being loaded is an import cycle; an identity loaded earlier is
a duplicate import. Different identities remain distinct even if their contents
match. The loader registers each source before parsing, but deduplicates its
identity before reading its body.

The file resolver uses stable file IDs where available: device and inode on Unix,
or volume serial number and file index on Windows. This makes hard links share an
identity. If stable IDs are unavailable, it uses canonical paths; on Linux it can
resolve an import base from `/proc/self/fd`.

Reads enforce per-file and total byte limits, file count, and import depth. The
loader may request one probe byte to distinguish an exact limit from an overflow.
Schema imports use file-scope `extern mod`; ordinary `mod` declarations create
namespaces within a source.

### 3.3 Shared budgets

One `ExecutionBudget` covers source loading, parsing, semantic checks, record
creation, indexing, location mapping, and freezing. A separate
`RegexExecutionBudget` covers pattern compilation and regex checks on Schema
defaults. Each budget reports its own steps, workspace, and failure.

`SchemaCompileUsage` exposes generic usage in `steps`, `workspace_bytes`,
`peak_workspace_bytes`, and `failure`; its `regex_*` fields report regex usage.
Resource exhaustion and allocation failure remain distinct errors, and stage labels
identify the current generic compilation phase.

Default resource policy is configured by `SchemaLimits`, `SyntaxLimits`,
`StreamLimits`, `ValueLimits`, `RegexLimits`, `RegexExecutionLimits`, and
`ExcerptLimits`. For example, the default Schema limits allow 1 MiB per source,
8 MiB total source bytes, and 64 files. These values are overridable runtime
policies, not DSL semantics.

## 4. Static compilation

`SchemaCompilation::compile` runs the stages below in order. Any diagnostic
prevents a usable `CompiledSchema` from being returned; compilation succeeds only
when all stages pass.

### 4.1 Parsing, modules, and symbols

The loader parses the root and each imported source. A source with syntax errors
does not enter the module tree, though other loadable sources may still contribute
diagnostics unless a terminal budget error occurs.

`ModuleTree` places declarations into their source modules and adds ordinary
`mod` declarations recursively. The root file's `entry struct` is the only Config
entry; an entry marker in an imported file does not replace it.

`SymbolTable` indexes modules, types, fields, variants, and anonymous-struct
payload fields. It detects duplicate declarations, fields, and variants, as well as
invalid entry counts. Lookups report missing, unique, or ambiguous results instead
of selecting one duplicate silently.

### 4.2 Type resolution

The first segment of a type path is resolved from the current module upward through
its parents. After that segment resolves, remaining segments are searched only in
module namespaces. A type used as an intermediate module, a non-type at the final
segment, or an invalid symbol produces a semantic error.

The type graph assigns IDs to built-ins, user types, containers, aliases, and
anonymous struct enum payloads. Aliases retain their named records and underlying
shape, while use sites retain applicable alias constraints and `@secret` metadata.

Alias cycles are checked separately. A budgeted worklist then finds which user
types can construct finite values: structs require all fields to be constructible,
while enums need one constructible variant. Positive-length fixed arrays require a
constructible element; zero-length arrays do not.

Recursive components with no finite value fail with a semantic diagnostic.
Declarations already invalid for other reasons are excluded to avoid secondary
recursion errors. Named struct and enum IDs are allocated before resolving fields
and payloads, so legal recursive references remain finite ID edges.

### 4.3 Annotations and expressions

Annotations attach to the exact recursive type-expression node they follow.
Compilation assigns declaration-root annotations to their owner and nested
annotations to a `TypeUse` slot. It checks target types, arguments, duplicates,
bound ordering, and compatibility among combined constraints.

`@regex` strings are compiled by `typol-regex` into bounded Thompson NFAs.
Compilation checks pattern size, regex count, instruction counts, and character
ranges; runtime validation reuses the compiled programs. `@env` is allowed only on
an eligible scalar or optional-scalar field root and creates authorization
metadata without reading an environment provider.

`@secret` marks its exact type node for diagnostic, display, and Rust-conversion
redaction; an alias-root marker also applies to uses of that alias. `@variants`
resolves names through transparent aliases and stores `VariantId`s. Unknown or
repeated names, invalid targets, and an empty effective set invalidate the Schema.

The expression compiler type-checks `@assert` and `@all`, including paths, methods,
operators, literal types, and the available `self`, `root`, closure, and map
contexts. Every expression must return `bool`. Programs are stored in
`typol-semantic`; assertions run later, while `@all` is attached to its array or
map type use.

### 4.4 Defaults and freezing

Default compilation builds `ConfigValue`s against resolved field types. It checks
scalar shape, numeric range, finite floats, containers, objects, enum payloads, and
applicable constraints. Integer fields accept integer tokens; float fields accept
float tokens, and `f32` defaults are rounded directly to binary32.

Object defaults may omit fields with defaults or optional types, but missing
required fields, unknown fields, and duplicate fields invalidate the Schema.
Default dependencies use explicit frames and detect cycles, including cycles caused
by recursive Schema types. Defaults never read environment variables.

Successful default validation produces `SchemaDefaultValidation`, which the full
compiler passes to `Builder::freeze_with_validated_defaults` to avoid a second
validation walk. A direct `Builder::freeze` receives no such evidence and validates
its inputs. Value limits and the generic budget cover default construction; regex
checks use the compile session's regex budget.

`RecordBuilder` freezes module, symbol, type, constraint, default, expression, and
location data into `CompiledSchema`. It builds sorted lookup indexes while keeping
fields and variants in declaration order. Callers use immutable `TypeView`, field,
variant, and rule views; `CompiledSchema::entry()` returns the root entry view.

Source spans remain UTF-8 byte ranges. The compiled model stores 1-based line and
column locations derived from parser indexes, with columns counted in Unicode
scalars. Source excerpts are captured under separate bounds; the compiler does not
retain complete source text or reread a source to fill a missing excerpt.

## 5. Diagnostics

Schema failures return non-empty `SchemaCompileErrors` or a non-allocating
emergency error. Diagnostics carry stable codes, primary spans, optional labels,
and location status. Duplicate-name errors can identify both declarations.

`Diagnostic::message()` returns a `Result`, so a translation failure remains
visible. `Display` writes the code, message, available location, and safe excerpt
without first building a report string; `Debug` exposes only counts and status.
`internal_cause()` provides structured internal details when available.

| Category | Code |
| --- | ---: |
| Allocation failure | 2 |
| Lexical error | 3 |
| Schema syntax error | 4 |
| Diagnostic limit reached | 5 |
| Regex syntax error | 6 |
| Schema static semantic error | 7 |
| Resolver or host import error | 8 |
| Root compile-context mismatch | 19 |
| Compile session ended | 20 |

Ordinary diagnostics are stably sorted by source ID and byte span. Terminal
resource, allocation, and internal errors take priority, and a diagnostic-limit
slot is preserved. Core diagnostics do not copy full source lines, raw environment
values, sensitive defaults, arbitrary host error text, or real file paths.

Syntax errors originate in `typol-syntax`; import, name, type, annotation, and
expression errors originate during Schema compilation. Config parsing must stop
when compilation fails because partial records are not valid input.

## 6. Config parsing

`ConfigParser::parse` receives a valid `CompiledSchema`, a Config source and
options, and an `Environment` implementation. It starts at `schema.entry()`,
parses tokens according to the expected type, and builds the value tree directly.
The compiled type determines whether `{}` denotes a struct or a map.

When a struct or anonymous struct payload is complete, the parser applies explicit
values, field defaults, and optional-value rules. It also reports unknown,
duplicate, or missing required fields. Arrays, fixed arrays, maps, and enum payloads
are checked against compiled shapes; any error prevents returning a partial value.

Local constraints run as values complete, collection constraints when arrays or
maps close, and full-tree assertions last. Assertions run only for the active enum
variant. Environment lookup occurs only for an explicit `env(...)` expression
authorized by a field-root `@env`; a name is read once per call, while each
reference is converted and validated independently.

Schema defaults do not come from the environment. `ValueSource` records whether a
value came from Config text, a Schema default, an environment lookup, a fallback,
or Rust conversion. Runtime validation reads compiled rules and regex programs
through immutable views.

## 7. Rust binding and codecs

`#[derive(Typol)]` describes a Rust type without opening a Schema file.
`LoadedSchema::bind` checks that description against the entry type and returns a
reusable binding. Ordinary binding requires `BudgetedTypol`; recursive derived
types are matched by Rust `TypeId` and Schema view while configured limits bound
the values they convert.

Handwritten `Typol` implementations use `bind_trusted` and produce visibly trusted
binding types. Library-managed work remains budgeted, but callbacks can allocate,
block, or compute outside those limits, and handwritten decoders are not
revalidated. `BudgetedTypol` is a capability marker, not a sandbox.

`Sourced<T>` retains the origin of a scalar field, including the name and fallback
metadata for an environment reference. Its Schema shape matches `T`; references
are accepted only for authorized scalar or optional-scalar fields. The wrapper has
no `Debug` implementation so it cannot accidentally print secrets or fallbacks.

Map bindings include `OrderedMap<T>` without `std`, `HashMap<String, T>` with
`std`, and optional `hashbrown` and `btree-map` feature implementations.
`OrderedMap` sorts keys; hash maps reserve capacity before decoding. BTreeMap uses a
checked logical allocation estimate, but node allocation follows the global
allocator because it has no fallible reservation API.

Text parsing performs full-tree validation. For foreign `ConfigValue`s,
normalization checks shape and fills defaults; `ConfigValueProcessor::validate`
then checks constraints and assertions. Rust serialization and contextual Serde
also validate the complete value, so Rust values cannot skip Schema rules.
The text writer validates before writing, follows Schema field order, and offers
compact and pretty layouts.

| Serde data-model operation | `serialize_serde` | `deserialize_serde` |
| --- | --- | --- |
| Boolean, integer, finite float, string | Supported | Supported |
| `Option<T>` | Supported; nested options rejected | Supported; nested options rejected |
| Sequence | Schema array | Schema array |
| Tuple | Same-length fixed Schema array | Same-length fixed Schema array |
| Map | String keys only | String keys only |
| Named struct | Type and field names must match | Type and field names must match |
| Enum variant | Unit, newtype, and named-field payloads | Unit, newtype, and named-field payloads |
| Other: char, bytes, unit, tuple struct/variant, `collect_str` | Rejected | Not applicable |
| `deserialize_any`/ignored-any, root identifier, char, bytes, unit, tuple struct/variant | Not applicable | Rejected |

The bridge reports unsupported operations, user callback rejection, and Schema
shape mismatches through stable `DiagnosticContext::failure_point()` identifiers.
Custom Serde error text is not stored or displayed. Callbacks remain a user-code
trust boundary, and the bridge can budget only work it controls.

`typol-std` supplies file, reader, process-environment, and diagnostic adapters.
Shared facade APIs use the same loading, binding, and error types in `std` and
`no_std + alloc` builds; callers pass an `Environment` explicitly.

## 8. Security and source excerpts

`@secret` and alias-propagated markers protect diagnostics and safe display by
hiding sensitive messages and truncating paths at the secret boundary. Rust
conversion errors follow the same policy. If a path cannot be mapped safely, or
redaction runs out of resources, the original path is not exposed.

Source snapshots retain only bounded, safe lines. Known default-value spans and
secret Config values are masked before excerpts are published; secret map entries
hide both keys and values. If masking cannot be proven complete, the excerpt is
omitted rather than copied in raw form.

Excerpt limits cover total bytes, line bytes, record count, and context rows.
Columns count Unicode scalar values, tabs count as one column, and CRLF is excluded
from line text. Truncated text stays on UTF-8 boundaries; missing excerpts are
never recovered by rereading the source.

Secrets remain present in successful values and serialized output. Safe display
and diagnostic redaction do not encrypt data; applications must protect actual
values they choose to retain or write.

## 9. Maintenance

Keep coverage for lexer and AST behavior, import graphs, name and type resolution,
recursive types, annotation rules, default construction, expression typing,
regex limits, diagnostics, resource failures, runtime validation, and Rust
bindings. Compare stable codes, locations, and structured results rather than
debug strings.

When implementation behavior conflicts with the Schema DSL, revise the
specification or decision record before changing parser or compiler semantics.
Implementation pages describe the current source; they do not establish language
rules independently.
