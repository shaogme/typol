---
title: Config IR v1 specification
description: JSON shape, ordering, sensitive-value markers, and resource boundaries for Config IR v1.
sidebar:
  order: 5
---

## Quick reference

:::tip[Use this page when]
You are consuming or producing Config IR v1 JSON and need exact value shapes, ordering, or validation behavior.
:::

| Topic | Rule to remember |
| --- | --- |
| Envelope | The ordered root members are `format`, `version`, `root_type`, and `value`; successful output ends in one LF. |
| Export mode | `Validated` is the default and checks constraints; `WithDefaults` fills defaults without constraint validation; `AsProvided` skips both. |
| Sensitive values | `secret: true` marks data but does not redact or encrypt it; consumers must protect the entire marked subtree. |

This document defines the output structure, ordering, sensitive-value markers, and resource boundaries for version 1 of the `typol-config-ir` JSON format (Config IR v1). Config IR exports a compiled Schema and a complete `ConfigValue`; it is not the `.tconf` configuration syntax. See the [Config DSL](../config-dsl/) for configuration values and the [Schema DSL](../schema-dsl/) for types and constraints.

## 1. Format identity and export boundary

Config IR is UTF-8 JSON text. The top-level object contains exactly these members, in this output order:

| Member | JSON type | Meaning |
| --- | --- | --- |
| `format` | string | Always `typol-config-ir`. |
| `version` | integer | Always `1`. |
| `root_type` | string | Qualified Schema name of the entry struct. |
| `value` | object | Root value encoded according to the entry struct type. |

Every successful output ends with exactly one LF (`U+000A`), in both compact and pretty modes. Output has no BOM. Pretty mode changes only JSON whitespace, not the data model.

V1 exports the complete configuration value. It does not include source locations, value origins, default origins, constraint definitions, or validation results. Type aliases are transparent: the format describes the expanded type shape, while structs and enums retain their qualified Schema names. The key type of `map<string, T>` is always string and is not encoded separately in the map type description.

`ConfigIr::serialize` does not read environment variables. `ConfigIrOptions` selects the value handling mode before serialization:

| Mode | Fill defaults | Schema constraints and assertions | Use |
| --- | --- | --- | --- |
| `Validated` (default) | Yes | Full validation | Fill defaults, then check fields, types, scalar and collection constraints, and struct and active enum variant assertions. |
| `AsProvided` | No | No | Explicitly skip normalization and constraint validation, encode the already constructed value, and check the value shapes listed in this specification. |
| `WithDefaults` | Yes | No | Fill omitted fields and optional values, including nested structs and active enum struct payloads; explicit values, including `none`, are kept. |

`WithDefaults` normalizes values without failing because an explicit value violates a Schema constraint. Serialization still checks value types, integer ranges, fixed array lengths, and payload shapes. `Validated` uses the same configuration resource and execution limits, with constraints checked after defaults are filled. All modes produce the same IR v1 shape.

## 2. Type descriptions

Each value node has a `type` member containing a type description object. A description contains only the members for its type shape:

| Schema type | Type description |
| --- | --- |
| `i8` through `i128`, `u8` through `u128`, `f32`, `f64`, `string`, `bool` | `{"kind":"<built-in type name>"}` |
| `option<T>` | `{"kind":"option","inner":T-description}` |
| `[T]` | `{"kind":"array","element":T-description,"length":null}` |
| `[T; N]` | `{"kind":"array","element":T-description,"length":N}` |
| `map<string, T>` | `{"kind":"map","value":T-description}` |
| Struct | `{"kind":"struct","name":"<qualified name>"}` |
| Enum | `{"kind":"enum","name":"<qualified name>"}` |

Qualified names come from the compiled Schema and separate module paths with `::`. Root-module names have no prefix. An anonymous struct enum payload has no separate type description; its fields are encoded directly in the payload.

Recursive Schema types keep these descriptions finite: a struct or enum description stops at its qualified nominal type name. Recursive Config values are emitted as ordinary nested value nodes, so the existing `max_nesting_depth` applies to every value edge. This behavior does not change the Config IR v1 format or version.

## 3. Value nodes

Every value node is a JSON object. It writes `type` first, then the data members for that type. If the node is sensitive, `secret` is last. The member is omitted when false; `false` is never emitted.

### 3.1 Scalars and optional values

| Type | Value member | Encoding |
| --- | --- | --- |
| Integer | `value` | Base-10 JSON string with no grouping or radix prefix; full signed or unsigned precision is preserved. |
| `f32`, `f64` | `value` | Finite float as a JSON string formatted with Rust `Display`; negative zero is always `"-0"`. Non-finite values cannot be encoded. |
| `string` | `value` | JSON string containing the configuration value's UTF-8 text. |
| `bool` | `value` | JSON boolean `true` or `false`. |
| `option<T>` | `value` | A full node encoded as `T`, or JSON `null` when empty. |

Numbers are strings by fixed v1 rule; consumers must not assume integer or float payloads are JSON numbers. Integer strings preserve the full `i128` and `u128` ranges. Float payloads must be finite; NaN and positive or negative infinity make export fail.

### 3.2 Arrays and maps

An array node uses an `items` array of value nodes encoded using the element type. Item order matches `ConfigValue`. A fixed-size array must have the length declared by the Schema.

A map node uses an `entries` array whose items have the form `{"key":"<string key>","value":<value node>}`. Entries are sorted by ascending key string for deterministic output. This does not give maps semantic ordering. Keys compare by exact Unicode contents without normalization. Values are encoded using the Schema map value type.

### 3.3 Struct fields

A struct node uses a `fields` array. Each item is `{"name":"<field name>","value":<value node>}`. Fields appear in Schema declaration order, independent of storage or input order in `ConfigValue`. Each Schema field must have exactly one value. Missing fields, extra fields, or type mismatches prevent export.

### 3.4 Enums

An enum value node contains `type`, `variant`, then `payload`. `variant` names the selected variant. Payload shape follows the variant declaration:

| Variant payload | `payload` object |
| --- | --- |
| Unit | `{"kind":"unit"}` |
| Single value | `{"kind":"single","value":<value node>}` |
| Anonymous struct | `{"kind":"struct","fields":[field items...]}` |

Field items in an anonymous struct payload match ordinary struct field items and are output in Schema declaration order. An unknown variant or a mismatched payload kind or value shape prevents export.

## 4. Sensitive-value markers

When the Schema marks a type or field with `@secret`, the serializer writes `"secret":true` on the corresponding value node. Secret markers propagated through aliases use the effective marker from the compiled Schema. If a single-payload enum variant's payload type is secret, the marker is written on the selected enum value node. A field marker applies to that field's value node.

A secret marker does not remove, mask, or encrypt values. Even when `secret: true` appears, the same node's `value`, `items`, `entries`, `fields`, or `payload` still contains the actual data. Consumers must treat a marked node and its entire subtree as sensitive. Child nodes may also carry their own markers from the Schema.

## 5. Example

This example shows an integer encoded as a string, an empty optional value, a key-sorted map, and a unit enum variant:

```json
{
  "format": "typol-config-ir",
  "version": 1,
  "root_type": "Example",
  "value": {
    "type": { "kind": "struct", "name": "Example" },
    "fields": [
      {
        "name": "count",
        "value": { "type": { "kind": "u64" }, "value": "184" }
      },
      {
        "name": "note",
        "value": {
          "type": { "kind": "option", "inner": { "kind": "string" } },
          "value": null
        }
      },
      {
        "name": "labels",
        "value": {
          "type": { "kind": "map", "value": { "kind": "string" } },
          "entries": [
            { "key": "alpha", "value": { "type": { "kind": "string" }, "value": "first" } },
            { "key": "zeta", "value": { "type": { "kind": "string" }, "value": "last" } }
          ]
        }
      },
      {
        "name": "state",
        "value": {
          "type": { "kind": "enum", "name": "State" },
          "variant": "Ready",
          "payload": { "kind": "unit" }
        }
      }
    ]
  }
}
```

## 6. Determinism and JSON member order

Compact output has no unnecessary whitespace; pretty output uses JSON pretty formatting. Both modes write object members in a stable order:

- Top level: `format`, `version`, `root_type`, `value`.
- Type descriptions: `kind` first, followed by `inner`, `element`/`length`, `value`, or `name` as required by the type.
- Scalar, optional, array, map, and struct value nodes: `type`, then `value`, `items`, `entries`, or `fields`; `secret` is last if present.
- Enum value nodes: `type`, `variant`, `payload`; `secret` is last if present.
- Field items: `name`, `value`. Map items: `key`, `value`.
- Enum payloads: `kind`, followed by `value` or `fields` as required.

Arrays keep their order, struct fields follow Schema declaration order, and map entries are sorted by key. This order makes serializer output deterministic. Consumers should identify JSON values by member names and must not depend on object member order.

## 7. Resource limits and failures

`ConfigIr::serialize` is provided by the `config-ir` feature of the `typol` crate and consumes a `ConfigValue`. JSON encoding begins only after any requested normalization or validation succeeds. A failure at any stage returns no partial JSON. Defaults in `ConfigIrOptions` are:

| Option | Default | Purpose |
| --- | ---: | --- |
| `pretty` | `false` | Select compact or pretty JSON. |
| `max_output_bytes` | 16,777,216 bytes | Limit final JSON bytes, including the final LF. |
| `max_workspace_bytes` | 16,777,216 bytes | Limit temporary reference workspace used to sort map entries and struct fields. |
| `max_nesting_depth` | 128 | Limit recursive depth for value nodes and nested container type descriptions. |
| `value_mode` | `Validated` | Select default normalization and Schema validation. Skipping validation requires `AsProvided`. |
| `validation_options` | `ConfigValidationOptions::default()` | In `WithDefaults` and `Validated` only, configure value resource limits and execution budget. |

Callers may override output and workspace limits. Nesting depth can be lowered but has a fixed maximum of 128; zero rejects the root value or type description. Workspace is measured as the logical byte size of references required for sorting, not total process memory.

The JSON output buffer starts at 8 bytes and grows geometrically. Each reservation is capped at `max_output_bytes`, and the final length is checked before appending. The buffer does not reserve separately for every small Serde write; the output limit includes the final LF. This limits output length and requested buffer capacity, not process peak memory: during growth, an allocator may hold old and new buffers at once, and sorting workspace has its own limit.

| Error | Meaning |
| --- | --- |
| `SchemaAccess` | Required compiled Schema information or a qualified name cannot be read. |
| `ValueShapeMismatch` | Value and Schema shapes disagree, including scalar type/range, fixed array length, struct field set, or enum payload. |
| `UnsupportedFloat` | A float is NaN or positive/negative infinity. |
| `OutputLimitExceeded` | Final JSON exceeds `max_output_bytes`. |
| `WorkspaceLimitExceeded` | Sorting map or struct references exceeds `max_workspace_bytes`. |
| `NestingLimitExceeded` | A value node or type description exceeds the nesting limit. |
| `InvalidNestingDepthLimit` | Requested nesting limit exceeds the fixed safety maximum. |
| `AllocationFailed` | Output buffer or temporary sorting-vector allocation failed. |
| `SerializationFailed` | Another JSON serialization failure occurred. |
| `NormalizationFailed` | `WithDefaults` normalization failed; the error carries secret-policy-filtered `ConfigValueErrors` diagnostics. |
| `ValidationFailed` | `Validated` normalization or Schema validation failed, with secret-policy-filtered diagnostics. |

`ConfigIrError::kind()` returns a stable `ConfigIrErrorKind` for comparing error categories, not complete error values. `ConfigIrError` does not implement `PartialEq` or `Eq`, since that would ignore differences in normalization and validation diagnostics. Those failures retain `ConfigValueErrors`, which callers can read through the error variant or `Error::source()` for safe diagnostics and available Schema locations. Diagnostics do not include secret values. Serialization errors do not contain the serialized configuration. Implementations may change internal storage, but the v1 output shape and limit/failure semantics remain stable.

## 8. Version compatibility

Consumers should check both `format` and `version` and must not interpret an unknown version as v1. V1 field meanings, node shapes, string-encoded numbers, ordering, and secret-marker semantics are format contracts. Incompatible changes require a new IR version.
