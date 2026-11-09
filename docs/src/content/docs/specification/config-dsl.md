---
title: Config DSL specification
description: Lexical, grammatical, and schema-guided value semantics for .tconf configuration files.
sidebar:
  order: 1
---

## Quick reference

:::tip[Use this page when]
You are writing `.tconf` values or need to understand how a compiled Schema controls parsing and defaults.
:::

| Topic | Rule to remember |
| --- | --- |
| File shape | A config file contains one root struct object matching the Schema entry type. |
| Field resolution | Explicit values win; omitted fields use a Schema default, then `none` for an optional field, otherwise they are required. |
| Environment | `env(...)` is valid only for a field explicitly authorized with a matching `@env` annotation. |

This document defines the lexical and grammatical rules for `.tconf` files and their value semantics relative to a Schema. Types, fields, enum variants, defaults, and constraints are declared by the [Schema DSL](../schema-dsl/).

## 1. Files and lexical rules

A configuration file is UTF-8 text. Parsing requires its root Schema; the configuration file does not declare a Schema path or entry type. The root Schema's `entry struct` is the configuration root type, so a file must contain exactly one struct object followed only by whitespace or comments.

Line endings are LF (`U+000A`) or CRLF (`U+000D U+000A`). A standalone CR is invalid, including inside a comment. Whitespace between tokens is limited to spaces, horizontal tabs, LF, and CRLF. Other Unicode whitespace is not a separator and causes a lexical error.

`//` begins a line comment. `/*` and `*/` delimit a non-nesting block comment, which must be closed.

Identifiers begin with an ASCII letter or underscore and continue with ASCII letters, digits, or underscores. They are case-sensitive. Reserved names cannot be identifiers: `extern`, `mod`, `type`, `struct`, `entry`, `enum`, `option`, `map`, built-in type names (`i8` through `i128`, `u8` through `u128`, `f32`, `f64`, `string`, `bool`), `true`, `false`, and `none`. `env` is not reserved; it is recognized as an environment expression only in a field value position where the Schema permits it.

Single-line strings use double quotes. Supported escapes are `\\`, `\"`, `\n`, `\r`, and `\t`. Other backslash escapes and unescaped control characters are invalid. Non-ASCII characters are represented directly as UTF-8.

Triple double quotes introduce a block string. The opening `"""` must be followed immediately by LF or CRLF, with no same-line whitespace or comment. The closing `"""` is the first triple quote after optional spaces or tabs at the start of a physical line. Its indentation and the structural line ending immediately before it are excluded from the value; the closing delimiter may be followed by ordinary DSL tokens on that line. A triple quote at the beginning of a body line closes the string; escape its first quote to include it as content.

The body is dedented before escapes are decoded. Blank lines containing only spaces and tabs become empty lines. For nonblank lines, the lexer removes the smallest count of leading ASCII spaces and tabs shared by all nonblank lines; a tab counts as one character, and additional indentation remains. Physical CRLF is normalized to LF. Supported escapes remain `\\`, `\"`, `\n`, `\r`, and `\t`; only physical source line endings and tabs in the remaining line-leading indentation may appear unescaped as control characters. A physical line ending creates a value newline, while `\\n` creates a newline only after dedenting. The shared rules are also used for Schema string literals.

For example, this field value decodes to `First line.\nSecond line.`:

```text
{
    description: """
        First line.
        Second line.
        """,
}
```

Integers are decimal or hexadecimal with a lowercase `0x` prefix. A leading zero without that prefix remains decimal. A float requires at least one decimal digit on both sides of the decimal point. Numbers may have a leading minus sign; a leading plus sign, exponent notation, and digit separators are unsupported. Boolean literals are `true` and `false`.

## 2. Grammar

Identifiers, strings, integers, and floats follow Section 1. Whether a value is valid for a field depends on its target Schema type.

```text
config          ::= object EOF
object          ::= "{" [field ("," field)* [","]] "}"
field           ::= identifier ":" value
map             ::= "{" [map_entry ("," map_entry)* [","]] "}"
map_entry       ::= string_literal ":" value
array           ::= "[" [value ("," value)* [","]] "]"
value           ::= string_literal | number | bool_literal | "none"
                  | object | map | array | enum_value | env_call
enum_value      ::= "." identifier
                  | "." identifier "(" value ")"
                  | "." identifier object
env_call        ::= "env" "(" string_literal ["," scalar_literal] ")"
scalar_literal  ::= string_literal | number | bool_literal
number          ::= integer | float
integer         ::= ["-"] (decimal_integer | hex_integer)
decimal_integer ::= digit+
hex_integer     ::= "0x" hex_digit+
float           ::= ["-"] digit+ "." digit+
bool_literal    ::= "true" | "false"
digit           ::= "0" | ... | "9"
hex_digit       ::= digit | "a" | ... | "f" | "A" | ... | "F"
```

Struct field names are unquoted; map keys must be strings. Arrays, struct objects, and maps may have a trailing comma. Duplicate field names, duplicate decoded map keys, unknown fields, unknown enum variants, and values incompatible with the target type are invalid.

## 3. Structs and collections

Fields in a struct object correspond to fields with the same names in the Schema. They may appear in any order. At the end of the object, each omitted field is resolved as follows:

1. Use its Schema default if one exists.
2. If its type is `option<T>` and it has no default, use `none`.
3. Otherwise it is required, and omission makes the configuration invalid.

These rules also apply to nested structs and anonymous struct enum payloads. An explicitly supplied field wins over its default. An explicit `none` overrides an optional field's default; a non-optional field cannot be `none`.

```tconf
{
    primary_host: {
        host: "db.internal",
        port: 5432,
    },
    read_replicas: [
        { host: "replica-a.internal", port: 5432 },
        { host: "replica-b.internal", port: 5432 },
    ],
}
```

Arrays use square brackets and interpret each element using the declared element type. `[T]` accepts any number of elements; `[T; N]` requires exactly `N`. Arrays preserve element order.

A `map<string, T>` uses object syntax with quoted string keys and values of type `T`; an empty map is `{}`. Decoded keys are compared by exact Unicode string contents, without normalization. Duplicate keys are invalid. Maps have no semantic key-value ordering.

```tconf
{
    labels: {
        "tier": "production",
        "owner": "platform",
    },
}
```

The target type determines the meaning of object syntax: a struct accepts unquoted field names and a map accepts string keys. Parsing does not retry the other interpretation. A type mismatch, unknown field, or incorrect key form makes the configuration invalid.

## 4. Enums and optional types

Enum variants start with a dot and are case-sensitive. Their payload form must match the Schema declaration:

- A unit variant is written `.Variant`.
- A single-value variant is written `.Variant(value)`, with exactly one value in parentheses.
- An anonymous struct variant is written `.Variant { fields... }`, following struct field rules.

An undeclared variant, a payload on a unit variant, a missing single payload, or a payload with the wrong form or type is invalid. Anonymous struct variants accept only declared fields. Defaults, optional values, and constraints for variant fields follow their Schema declarations.

For `option<T>`, a value of type `T` represents the present state and `none` represents the empty state. If the optional field has a default, omission uses that default; explicit `none` still means empty and overrides the default. No implicit conversion occurs beyond wrapping a present `T` value.

```tconf
{
    protocol: .Tcp,
    tls: .Tls({
        cert_path: "/etc/service/cert.pem",
        key_path: "/etc/service/key.pem",
    }),
    role: .Follower {
        leader_endpoint: { host: "leader.internal", port: 9000 },
        sync_timeout_ms: 5000,
    },
}
```

```tconf
{
    optional_int: 42,
    optional_tag: { id: 10, name: "observability" },
}
```

## 5. Scalar values and constraints

A scalar literal must match its target field type: strings, booleans, signed and unsigned integers, and floats are not converted between types. Integers must fit the target width. After conversion to the target `f32` or `f64`, a float must be finite. Conversion uses IEEE 754 round-to-nearest, ties-to-even. Typol does not support `f128`.

Schema aliases do not change the underlying configuration syntax. Values use the representation of the expanded target type and must satisfy constraints from the field, aliases, and type. Definitions of `@length`, `@regex`, collection constraints, and other constraints are in the [Schema DSL annotation specification](../schema-dsl/#5-annotations). String length counts Unicode scalar values and performs no Unicode normalization.

Field values and collection elements are checked against their Schema constraints as the complete configuration value is built. Array and map size, uniqueness, all-item, and sum constraints follow the Schema DSL. Struct assertions and assertions for the active enum variant run after the complete value is built. Assertions on inactive variants do not apply. If a field is missing or invalid so that an assertion cannot be evaluated, no additional assertion failure is produced.

## 6. Environment expressions

An environment variable is referenced explicitly with `env("NAME")`. The expression is valid only for a scalar field authorized by `@env("NAME")` in the Schema. The target may also be `option<T>`, where `T` is scalar. The name in the configuration must exactly match the field's authorized name. Environment names must match `[A-Za-z_][A-Za-z0-9_]*`.

`env` can be used in nested struct fields and anonymous struct enum payload fields. Array elements, map entries, and a single-value enum payload cannot declare environment authorization on their own. Schema `@env` authorizes a name but does not read it when a field is omitted; a read occurs only when `env(...)` appears.

Environment text is interpreted according to the target scalar type: strings are used as-is, booleans accept only `true` or `false`, and numbers follow Section 1. Leading or trailing whitespace is not trimmed, and the text must be valid UTF-8. An empty string is a set value; the target type and constraints determine whether it is valid. A set value that fails parsing or validation invalidates the configuration; it does not fall back.

An expression may provide a scalar literal fallback: `env("NAME", fallback)`. The fallback must match the target scalar type and satisfy the field's constraints even if the environment variable is set. The expression fallback takes precedence over a Schema default. If the environment value is set, it is used; otherwise the explicit fallback is used. If the variable is unset and there is no fallback, normal field omission rules apply: Schema default, optional `none`, or a required-field error. For an `option<T>` field, a successfully resolved environment value or fallback is present.

## 7. Canonical formatting

`typol config fmt` formats a configuration only after it has been parsed and validated with its root Schema. Formatting changes whitespace and line breaks while preserving every source value token, comment text, and entry order. It does not serialize the resolved value, write environment contents, or insert omitted Schema defaults.

Nonempty arrays, struct objects, maps, and anonymous struct enum payloads use one item per line, four spaces per nesting level, and a trailing comma after each item. Empty arrays and objects remain `[]` and `{}` unless they contain comments. A colon is followed by one space. Single-value enum payloads use a multiline parenthesized group without a trailing comma; nested containers follow the same rules. `env(...)` calls keep compact parentheses. Comments remain in source order, and line comments always end their line. The formatter preserves CRLF when the input uses CRLF and otherwise uses LF; formatted output ends with a newline.

Formatting is deterministic and idempotent: formatting an already formatted document produces the same bytes. The CLI writes the complete result back to the input file by default. `--check` reports whether a change is needed without writing the file.
