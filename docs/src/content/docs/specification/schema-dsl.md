---
title: Schema DSL specification
description: Lexical, grammatical, and static semantic rules for .tschem Schema files.
sidebar:
  order: 2
---

## Quick reference

:::tip[Use this page when]
You are declaring configuration types, defaults, annotations, or cross-field validation rules in `.tschem` files.
:::

| Topic | Rule to remember |
| --- | --- |
| Entry and imports | The root Schema has exactly one `entry struct`; imported declarations use `extern mod`. |
| Types | Types include scalars, options, arrays, string-keyed maps, structs, and enums; recursive types are allowed when they have a finite value. Nested options are unsupported. |
| Validation | Defaults and annotations are checked during Schema compilation; assertions run after a configuration value is constructed. |

This document defines the lexical, grammatical, and static semantic rules for `.tschem` Schema files. A Schema describes configuration types, defaults, and validation rules. The syntax for configuration instances is defined by the [Config DSL specification](../config-dsl/).

## 1. Lexical rules

Schema source is UTF-8. Whitespace between tokens is limited to space (`U+0020`), horizontal tab (`U+0009`), LF (`U+000A`), and CRLF (`U+000D U+000A`). A standalone CR is invalid. Other Unicode whitespace, vertical tab, and form feed are also invalid. LF and CRLF both represent line breaks.

The language supports `//` line comments and non-nesting `/* ... */` block comments. Block comments must be closed. Single-line strings use double quotes and support `\"`, `\\`, `\n`, `\r`, and `\t`. Other escapes and unescaped control characters are invalid. Non-ASCII string characters retain their Unicode content.

Triple double quotes introduce a block string. The opening `"""` must be followed immediately by LF or CRLF, with no same-line whitespace or comment. The closing `"""` is the first triple quote after optional spaces or tabs at the start of a physical line. Its indentation and the structural line ending immediately before it are excluded from the value; the closing delimiter may be followed by ordinary DSL tokens on that line. A triple quote at the beginning of a body line closes the string; escape its first quote to include it as content.

The body is dedented before escapes are decoded. Blank lines containing only spaces and tabs become empty lines. For nonblank lines, the lexer removes the smallest count of leading ASCII spaces and tabs shared by all nonblank lines; a tab counts as one character, and additional indentation remains. Physical CRLF is normalized to LF. Supported escapes remain `\"`, `\\`, `\n`, `\r`, and `\t`; only physical source line endings and tabs in the remaining line-leading indentation may appear unescaped as control characters. A physical line ending creates a value newline, while `\\n` creates a newline only after dedenting.

For example, this default decodes to `First line.\nSecond line.`:

```text
entry struct Release {
    description: string = """
        First line.
        Second line.
        """,
}
```

The same block-string rules apply in `.tconf` values and in every Schema position that accepts a string literal.

Identifiers begin with an ASCII letter or underscore, followed by ASCII letters, digits, or underscores. They are case-sensitive. The following keywords cannot be declaration names, field names, or path segments: `extern`, `mod`, `type`, `struct`, `entry`, `enum`, `option`, `map`, every built-in type name, `true`, `false`, and `none`.

An integer is a sequence of decimal digits or hexadecimal digits with a lowercase `0x` prefix. Unprefixed digits are decimal, including those with leading zeroes. A float consists of non-empty decimal digits on each side of a decimal point. Number tokens do not include a sign; a leading plus, exponent notation, and digit separators are unsupported. A minus is parsed as unary negation in value positions and expressions where negative numbers are permitted.

## 2. Files and declarations

A Schema file consists of top-level declarations. The root Schema must contain exactly one `entry struct`. An imported file may omit an entry or declare one; an imported entry is treated as an ordinary type declaration. `extern mod` is permitted only at file scope.

```text
schema_file       ::= top_level_item*
top_level_item    ::= extern_mod_decl | declaration | entry_decl
extern_mod_decl   ::= "extern" "mod" identifier ";"
entry_decl        ::= "entry" struct_decl
declaration       ::= module_decl | type_decl | struct_decl | enum_decl
module_decl       ::= "mod" identifier "{" declaration* "}"
type_decl         ::= "type" identifier "=" type annotation* ";"
```

`extern mod name;` introduces a module whose contents come from an external Schema file. Modules may be nested. Module and type declarations share a namespace within their scope, so duplicate names in the same scope are invalid. Declaration order does not affect type resolution.

A type path starts with an identifier and can contain more segments separated by `::`. There is no separate absolute path syntax. Resolution searches for the first segment from the current module and then each parent module; the remaining segments are resolved from the matching module. The same path may therefore resolve to a nearer declaration in a nested module. From the root module, `common::network::Endpoint` refers to the type in the root's `common` module.

```text
extern mod common;

mod network {
    struct Endpoint {
        host: string = "127.0.0.1",
    }
}

entry struct ServiceConfig {
    endpoint: network::Endpoint,
    shared: common::SharedSettings,
}
```

A type alias uses `type Name = Type;` and may have annotations applicable to its target. Aliases are transparent and do not introduce a runtime representation. Both alias constraints and constraints at the use site apply. Aliases cannot be cyclic. `@env` applies only to fields, not aliases.

## 3. Types, structs, and enums

### 3.1 Types

```text
type          ::= builtin_type
                | "option" "<" type ">"
                | "[" type "]"
                | "[" type ";" decimal_integer "]"
                | "map" "<" "string" "," type ">"
                | path
builtin_type  ::= "i8" | "i16" | "i32" | "i64" | "i128"
                | "u8" | "u16" | "u32" | "u64" | "u128"
                | "f32" | "f64" | "string" | "bool"
path          ::= identifier ("::" identifier)*
```

Integer types are signed or unsigned integers of the stated width. `f32` and `f64` are IEEE 754 binary32 and binary64. `option<T>` is an optional `T`; `[T]` is an array of any length; `[T; N]` has exactly `N` elements; and `map<string, T>` maps unique string keys to `T`. `N` is a non-negative decimal integer in the range 0 through `u32::MAX`.

Container elements may themselves use containers or user-declared types. After aliases are fully expanded, nested `option<option<T>>` is unsupported. User-declared types may refer to themselves or each other through fields, enum payloads, containers, and non-cyclic aliases when the type graph has a finite value. Options, dynamic arrays, maps, and zero-length fixed arrays can stop recursive construction; a recursive enum needs at least one constructible variant. Positive-length fixed arrays require a constructible element type. A required cycle with no finite branch, such as `struct Loop { next: Loop }`, is a static error. Type aliases must still be acyclic.

```tschem
struct Node {
    value: string,
    children: [Node],
}

enum Expr {
    Number(i64),
    Add([Expr]),
}

entry struct Config {
    root: Node,
    expression: Expr,
}
```

### 3.2 Structs and fields

```text
struct_decl   ::= "struct" identifier "{" field_list? "}" struct_assert*
field_list    ::= field ("," field)* ","?
field         ::= identifier ":" type annotation* ("=" default_value)?
struct_assert ::= "@assert" "(" expression "," string_literal ")"
```

A struct may be empty. Field names must be unique within that struct. A field may have zero or more annotations after its type; a default, when present, follows the annotations. Fields are comma-separated, and the final comma is optional.

### 3.3 Enums

```text
enum_decl       ::= "enum" identifier "{" variant_list ","? "}"
variant_list    ::= variant ("," variant)*
variant         ::= identifier variant_assert*
                  | identifier "(" type annotation* ")" variant_assert*
                  | identifier "{" field_list? "}" variant_assert*
variant_assert  ::= "@assert" "(" expression "," string_literal ")"
```

An enum must contain at least one variant, and variant names must be unique. A variant can have no payload, one typed payload, or an anonymous struct payload. Anonymous struct fields follow ordinary field rules for types, annotations, and defaults.

An assertion following a variant belongs to that variant and runs only when that variant is selected. Assertions following a struct belong to that struct. In an assertion, `self` is the current struct instance or variant payload, and `root` is the entry struct instance. In an entry struct assertion, both refer to the same instance. A unit variant has no payload and cannot use `self`. Assertions run after a configuration value is fully constructed.

## 4. Defaults

A field may declare a default with `= value`. The default must match the field type and satisfy applicable constraints. Defaults never read environment variables.

Scalar defaults follow strict token typing, as do configuration literals: integer types accept integer tokens only; `f32` and `f64` accept float tokens only; strings and booleans accept their corresponding literals. An integer token is not implicitly converted to a float default. A float default is rounded directly to the field's target precision using IEEE 754 round-to-nearest, ties-to-even; the result must be finite.

```text
default_value ::= scalar_literal | "none" | object_literal
                | array_literal | map_literal | enum_value
scalar_literal ::= string_literal | signed_numeric_literal | bool_literal
signed_numeric_literal ::= "-"? numeric_literal
numeric_literal ::= integer_literal | float_literal
integer_literal ::= decimal_integer | hex_integer
array_literal ::= "[" (default_value ("," default_value)* ","?)? "]"
map_literal ::= "{" (map_entry ("," map_entry)* ","?)? "}"
map_entry ::= (string_literal | identifier) ":" default_value
object_literal ::= "{" (object_field ("," object_field)* ","?)? "}"
object_field ::= identifier ":" default_value
enum_value ::= "." identifier
              | "." identifier "(" default_value ")"
              | "." identifier object_literal
bool_literal ::= "true" | "false"
hex_integer ::= "0x" hex_digit+
float_literal ::= digit+ "." digit+
```

A minus in a default is a separate unary prefix, not part of an integer or float token, and is parsed only where the default grammar permits it.

An object default may omit fields that have their own defaults or optional types. Omitting a required field invalidates the Schema. Undeclared object fields are invalid. Values in arrays, maps, objects, and enum payloads must match their target types. An enum default must use the payload shape declared for that variant. `none` is valid only for optional types; an optional field may instead contain a default of its inner type, which means the value is present.

Defaults and all nested default values are checked for type and constraints during Schema validation. An invalid default makes the Schema invalid.

The Rust facade can generate a complete `.tconf` from these declarations through `LoadedSchema::default_config_to_string` or `write_default_config`. Generation uses the entry struct as an empty root, fills fields with their defaults, and uses `none` for optional fields without defaults. Any other required field must have an explicit default. A nested struct, array, map, or enum field is not inferred from its contents; its field needs a default that selects the value. Generation does not evaluate environment expressions, and it still runs the complete value constraints and assertions after filling defaults.

## 5. Annotations

Annotations may be used on type aliases, fields, anonymous struct variant fields, and the payload type of a single-value variant, subject to the target types in the table below. An annotation may appear at most once on a declaration. Alias and use-site constraints apply together.

```text
annotation ::= "@min" "(" number ")"
             | "@max" "(" number ")"
             | "@range" "(" number "," number ")"
             | "@length" "(" decimal_integer "," decimal_integer ")"
             | "@regex" "(" string_literal ")"
             | "@host" | "@email"
             | "@cidr" "(" "v4_only" "=" bool_literal ")"
             | "@env" "(" string_literal ")"
             | "@size" "(" size_args ")"
             | "@unique" ("(" "by" "=" string_literal ")")?
             | "@all" "(" expression "," string_literal ")"
             | "@sum" "(" sum_args ")"
             | "@secret"
size_args ::= decimal_integer "," decimal_integer
            | size_bound ("," size_bound)?
size_bound ::= ("min" | "max") "=" decimal_integer
sum_args ::= sum_bound ("," sum_bound)?
sum_bound ::= ("min" | "max") "=" number
number ::= numeric_literal | "-" numeric_literal
```

| Annotation | Applicable target | Meaning |
| --- | --- | --- |
| `@min(n)`, `@max(n)`, `@range(min, max)` | Numeric type or optional numeric type | Inclusive comparisons on present numeric values. The lower range bound cannot exceed the upper bound. Integer targets use integer bounds. |
| `@length(min, max)` | `string` or optional string | Unicode scalar-value count must be within the inclusive range. No normalization is performed. |
| `@regex("pattern")` | `string` or optional string | The pattern must be valid and at least one substring must match. Use `^` and `$` for a whole-string match. |
| `@host` | `string` or optional string | Validates an IP literal or DNS hostname; schemes, ports, and whitespace are not accepted. |
| `@email` | `string` or optional string | Validates a practical ASCII email form: a non-empty local part, one `@`, and a valid DNS domain. It does not promise full RFC 5322 coverage. |
| `@cidr(v4_only = bool)` | `string` or optional string | Validates an IP address and prefix length. `true` permits only IPv4; `false` permits IPv4 or IPv6. |
| `@env("NAME")` | String, boolean, or numeric field, including optional forms | Authorizes reading the named environment variable for a field. It is not valid on aliases. |
| `@size(...)` | Array or map | Limits element or entry count. Positional arguments are min and max; named arguments may specify either or both. |
| `@unique` | Array | Requires array elements to be unique. |
| `@unique(by = "field.path")` | Array of structs | Requires the selected field path to be unique. |
| `@all(expression, "message")` | Array or map | Requires every array element or map value to satisfy the expression. |
| `@sum(...)` | Numeric array | Sums elements in array order and checks inclusive bounds; either or both of `min` and `max` may be given. |
| `@secret` | Alias, field, or single-payload type | Marks a value sensitive for diagnostics and safe display. A marker on an alias propagates to uses of that alias. |

The bounds of `@min`, `@max`, `@range`, `@size`, and `@sum` are inclusive. Numeric bounds are contextual: an integer bound may be used for a float target and is rounded directly to that target precision using IEEE 754 round-to-nearest, ties-to-even. A float bound cannot be used for an integer target. Bounds and combinations must be compatible with the target type; an inapplicable annotation, duplicate annotation, invalid argument, or incompatible combination invalidates the Schema.

String equality and string values compared by `@unique` use exact Unicode text. No Unicode normalization, case folding, DNS trailing-dot removal, or IP parsing is performed. `@host` validates string form without changing equality or uniqueness. DSL expressions do not offer normalized host comparison. Host code can explicitly call `ValueComparison::host_equal`, which compares parseable IP addresses as addresses, or otherwise removes one trailing dot and compares text using ASCII case-insensitive rules.

### 5.1 Regex syntax and matching

Regex patterns use the text after Schema string escape decoding and have search semantics.

```text
regex ::= alternation?
alternation ::= concatenation ("|" concatenation)*
concatenation ::= repeat*
repeat ::= consuming_atom quantifier? | anchor
consuming_atom ::= literal | "." | character_class | "(" regex ")" | "\\" escape
anchor ::= "^" | "$"
quantifier ::= "*" | "+" | "?" | "{" decimal_integer "}"
             | "{" decimal_integer "," decimal_integer? "}"
character_class ::= "[" "^"? class_item+ "]"
class_item ::= class_escape | class_char ("-" class_char)? | "-"
class_escape ::= "\\" escape
escape ::= "n" | "r" | "t" | regex_metachar | "-"
         | "d" | "D" | "w" | "W" | "s" | "S"
literal ::= an unescaped, non-metacharacter Unicode scalar value
class_char ::= one scalar other than "]", backslash, or "-"
```

`{m}` repeats exactly `m` times; `{m,}` repeats at least `m` times; and `{m,n}` repeats within an inclusive range. Bounds cannot exceed 255 and the lower bound cannot exceed the upper. The whole pattern and groups may be empty and match an empty string, but an alternative branch separated by `|` cannot be empty. A quantifier applies to one consuming atom; repeated quantifiers and quantified anchors are invalid. Character classes cannot be empty. Range endpoints are single Unicode scalar values, and the start cannot exceed the end. Both endpoints must be literal characters. An unescaped hyphen is allowed only at the start or end of a class, or as a range marker. A leading `^` in a class means complement.

`\d`, `\w`, and `\s` match ASCII digits, ASCII letters/digits/underscore, and ASCII whitespace (space and `U+0009` through `U+000D`). Uppercase forms match the complement within the Unicode scalar-value range. Literals and ranges match Unicode scalar values without case folding, normalization, or locale conversion. A dot matches any scalar except LF (`U+000A`). `^` and `$` mean the beginning and end of the entire input, not line boundaries.

Backreferences, look-around, named captures, inline flags, Unicode property escapes, word-boundary escapes, and unlisted escapes are unsupported. Groups change precedence but do not capture text.

## 6. Assertions and expressions

`@assert(expression, "message")` may follow a struct or enum variant. `@all(expression, "message")` uses the same expression language. Expressions are statically type-checked during Schema validation and must return a boolean. Unknown fields, unavailable methods, type mismatches, and non-boolean results invalidate the Schema.

```text
expression ::= or_expr
or_expr ::= and_expr ("||" and_expr)*
and_expr ::= compare_expr ("&&" compare_expr)*
compare_expr ::= sum_expr (compare_op sum_expr)?
sum_expr ::= product_expr (("+" | "-") product_expr)*
product_expr ::= unary_expr (("*" | "/") unary_expr)*
unary_expr ::= ("!" | "-") unary_expr | postfix_expr
postfix_expr ::= primary_expr ("." identifier ("(" method_argument? ")")?)*
primary_expr ::= scalar_literal | "none" | value_path | "(" expression ")"
value_path ::= ("self" | "root" | "item" | "key" | "value" | identifier)
               ("." identifier)*
method_argument ::= expression | "|" identifier "|" expression
compare_op ::= "==" | "!=" | "<" | "<=" | ">" | ">="
```

Precedence, from highest to lowest, is unary operators, multiplication/division, addition/subtraction, comparison, `&&`, then `||`. Comparisons cannot be chained. Field paths use dots. Except for names bound by `@all` and closure parameters, a path must begin with `self` or `root`. In `@all`, an array element is `item`; a map entry may use `key` and `value`. These names exist only in their corresponding expression.

Read-only methods are: arrays and maps support `len() -> u64` and `is_empty() -> bool`; optional values support `is_some() -> bool` and `is_none() -> bool`; strings support `starts_with(string) -> bool`; enums support `is_variant(string) -> bool`; arrays support `any(|name| expression) -> bool` and `all(|name| expression) -> bool`; numeric arrays support `sum() -> element_type`. Closure parameters are visible only inside their closure and cannot be named `self`, `root`, `item`, `key`, or `value`.

Boolean operators accept booleans only. Comparison operands must have the same type. A numeric literal may infer its type from the other operand; otherwise integers infer as `i128` and floats as `f64`. A float literal is rounded directly to its inferred precision using IEEE 754 round-to-nearest, ties-to-even. If an integer token is inferred as a float, it is parsed exactly before direct conversion to the target precision. Contextual inference does not change strict token typing for defaults. There is no implicit numeric promotion.

Equality supports same-type scalars, enums, and comparable optional values; structs, arrays, and maps do not support equality. String equality compares exact Unicode text. Ordered comparison supports same-type numbers and strings; strings are ordered lexicographically by Unicode scalar values. Arithmetic supports same-type numbers. Unary minus supports signed integers and floats only. Integer overflow, division by zero, or a non-finite floating-point result is an evaluation error. `none` can be compared only with an optional value of the same type and cannot be dereferenced.

`&&` and `||` short-circuit. `any` and `all` stop when the result is known; on an empty array, `any` is false and `all` is true. Assertions only read values; they do not modify configuration or read environment variables. Runtime assertions run only for completed instances and the selected enum variant.

## 7. Static validity

A Schema is usable for configuration parsing only if its syntax and static semantics are valid. Type references must resolve to structs, enums, or valid aliases. Aliases cannot be cyclic, and recursive user-declared types must have at least one finite value. Struct fields, enum variants, and declarations in the same namespace cannot be duplicated. The entry must be a struct.

Every default must match its declared type and constraints. Annotations must apply to supported target types and have valid arguments and bounds. Assertions and collection expressions must pass static type checking. If any condition fails, the Schema is invalid.

## 8. Canonical formatting

`SchemaFormatter` formats syntax-valid source without resolving imports or checking static semantics. It preserves every non-whitespace token and comment byte in source order, including numeric and string spelling. The canonical layout uses four-space indentation, expands non-empty declaration bodies and compound defaults, puts list elements on separate lines with trailing commas, and leaves declarations one blank line apart. Empty braces stay inline; types and expressions stay compact. The output ends with a newline and uses CRLF when the input contains any CRLF, otherwise LF. Formatting does not reorder declarations or comments.
