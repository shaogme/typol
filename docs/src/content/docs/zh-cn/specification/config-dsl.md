---
title: "Config DSL 规范"
description: "定义 .tconf 配置文件的语法、值解析、默认值和环境变量表达式。"
sidebar:
  order: 1
---

本文规定 `.tconf` 配置文件的词法、语法及其相对于 Schema 的值语义。类型、字段、枚举变体、默认值和约束由对应的 [Schema DSL](../schema-dsl/) 声明。

## 1. 文件与词法

配置文件是 UTF-8 文本。解析时必须提供对应的根 Schema；配置文件本身不声明 Schema 路径或入口类型。根 Schema 的 `entry struct` 是配置值的根类型，因此配置文件必须恰好包含一个结构体对象，且对象之后只能有空白或注释。

行结束符为 LF（`U+000A`）或 CRLF（`U+000D U+000A`）。单独出现的 CR 非法，包括出现在注释中时。token 之间的空白仅包括空格、水平制表符、LF 和 CRLF；其他 Unicode 空白字符不是分隔符，会导致词法错误。

`//` 开始行注释，延续到行结束；`/*` 和 `*/` 包围块注释，块注释不嵌套且必须闭合。

标识符以 ASCII 字母或 `_` 开始，后续由 ASCII 字母、数字或 `_` 组成，区分大小写。以下名称保留，不能用作标识符：`extern`、`mod`、`type`、`struct`、`entry`、`enum`、`option`、`map`、内建类型名（`i8` 至 `i128`、`u8` 至 `u128`、`f32`、`f64`、`string`、`bool`）、`true`、`false` 和 `none`。`env` 不是保留字；仅在 Schema 允许的字段值位置，`env(...)` 才作为环境变量表达式识别。

字符串使用双引号。支持的转义只有 `\\`、`\"`、`\n`、`\r` 和 `\t`。其他反斜杠转义以及未转义控制字符均非法；非 ASCII 字符按 UTF-8 原样表示。

整数使用十进制或小写 `0x` 前缀的十六进制；无前缀的前导零数字仍按十进制解释。浮点数必须在小数点两侧各有至少一位十进制数字。数值可带前导 `-`，不支持前导 `+`、指数记法或数字分隔下划线。布尔字面值为 `true` 和 `false`。

## 2. 语法

下列语法中，`identifier`、字符串、整数和浮点数遵循第 1 节的词法规则。具体值是否适用于字段，由目标 Schema 类型决定。

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
digit           ::= "0" | "1" | ... | "9"
hex_digit       ::= digit | "a" | ... | "f" | "A" | ... | "F"
```

结构体字段名不加引号，map 键必须是字符串。数组、结构体对象和 map 均可写尾逗号。字段名重复、map 解码后键重复、未知字段、未知枚举变体以及不符合目标类型的值均无效。

## 3. 结构体与集合

结构体对象中的字段对应 Schema 中同名字段。字段可按任意顺序书写；缺省字段在对象结束时按以下规则确定：

1. 有 Schema 默认值时，采用该默认值。
2. 类型为 `option<T>` 且没有默认值时，取 `none`。
3. 其他字段为必填字段，缺省时配置无效。

这些规则同样适用于嵌套结构体，以及枚举的匿名结构体载荷。显式给出的字段值优先于默认值。显式 `none` 可用于覆盖可选字段的默认值；非可选字段不能取 `none`。

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

数组使用方括号，元素按 Schema 声明的元素类型解释。`[T]` 可包含任意数量的元素；`[T; N]` 必须恰好包含 `N` 个元素。数组保持元素次序。

`map<string, T>` 使用对象语法，键为双引号字符串，值按 `T` 解释，空 map 写作 `{}`。解码后的键以 Unicode 字符串精确比较，不进行 Unicode 归一化；重复键无效。map 不定义键值对的语义顺序。

```tconf
{
    labels: {
        "tier": "production",
        "owner": "platform",
    },
}
```

对象语法的含义由目标类型决定：结构体目标接受未加引号的字段名，map 目标接受字符串键。解析不会在两种含义之间尝试回退。类型不匹配、未知字段或错误的键形式均使配置无效。

## 4. 枚举与可选类型

枚举变体使用前导点号，名称区分大小写。Schema 声明的载荷形态必须与配置一致：

- 无载荷变体写作 `.Variant`。
- 单载荷变体写作 `.Variant(value)`，括号内只能有一个值。
- 匿名结构体变体写作 `.Variant { fields... }`，字段遵循结构体字段规则。

未声明的变体、给无载荷变体附加载荷、单载荷缺失或载荷形式/类型不匹配均无效。匿名结构体变体只接受该变体声明的字段。变体字段的默认值、可选值和约束按其 Schema 声明处理。

对于 `option<T>`，可直接提供 `T` 的值；该值表示有值状态。`none` 表示空值。若可选类型带有默认值，省略字段采用默认值；显式 `none` 仍表示空值并覆盖默认值。除可选值的这种包装外，不进行隐式类型转换。

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

## 5. 标量值与约束

标量字面值必须符合目标字段类型：字符串、布尔值、有符号/无符号整数及浮点值之间不进行转换。整数必须处于目标位宽范围内。浮点值转换为目标 `f32` 或 `f64` 后必须是有限值；转换采用 IEEE 754 最近值、平分取偶数舍入。Schema 不支持 `f128`。

Schema 类型别名不改变配置中的底层值写法。值仍按别名展开后的目标类型表示，并须满足适用于该字段、别名及其类型的约束。`@length`、`@regex`、集合约束及其他约束的定义见 [Schema DSL 的注解规范](../schema-dsl/#5-注解)；字符串长度按 Unicode 标量值计算，不进行 Unicode 归一化。

字段值和容器元素在完整配置值形成时接受对应 Schema 约束检查。数组/map 的大小、唯一性、全项及求和约束按 Schema DSL 定义检查。结构体和当前实际枚举变体的 `@assert` 在完整配置值形成后检查；不活动的枚举变体断言不适用于该配置值。因字段缺失或字段值无效而无法计算的断言不产生额外的断言失败。

## 6. 环境变量表达式

环境变量通过 `env("NAME")` 显式引用。表达式仅适用于 Schema 以 `@env("NAME")` 授权的标量字段；目标也可以是 `option<T>`，其中 `T` 为标量。配置中的名称必须与该字段授权名称完全一致。环境变量名称必须匹配 `[A-Za-z_][A-Za-z0-9_]*`。

`env` 可用于嵌套结构体字段和枚举匿名结构体载荷字段。数组元素、map 项和枚举单载荷本身不能单独声明环境授权。Schema 上的 `@env` 只授权名称，不会因字段省略而自动读取环境变量；只有出现 `env(...)` 才会读取。

环境值按字段目标标量类型解释：字符串原样使用，布尔值只接受 `true` 或 `false`，整数和浮点数遵循第 1 节数字格式。环境文本不裁去首尾空白，且必须是有效 UTF-8。空字符串是已设置的值，其是否有效由目标类型及约束决定。已设置值解析失败或违反约束时配置无效，不改用回退值。

可在表达式中提供一个标量字面值作为回退：`env("NAME", fallback)`。回退字面值必须符合目标标量类型及该字段的约束；即使环境变量已设置，提供的回退值也必须有效。表达式中的回退值优先于 Schema 默认值；环境变量已设置时使用环境值，未设置时使用显式回退值。若环境变量未设置且没有回退值，则按字段缺省规则采用 Schema 默认值、可选值 `none` 或必填字段错误。对 `option<T>` 字段，成功取得的环境值或回退值成为有值状态。
