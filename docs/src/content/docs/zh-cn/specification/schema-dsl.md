---
title: "Schema DSL 规范"
description: "定义 .tschem Schema 文件的语法、类型、注解、表达式和静态有效性规则。"
sidebar:
  order: 2
---

## 本页要点

:::tip[适用场景]
在 `.tschem` 中声明配置类型、默认值、注解或跨字段校验规则时，请查阅本页。
:::

| 主题 | 关键规则 |
| --- | --- |
| 入口与导入 | 根 Schema 恰有一个 `entry struct`；通过 `extern mod` 导入其他声明。 |
| 类型 | 支持标量、option、数组、字符串键 map、结构体和枚举；有有限值的递归类型可用，嵌套 option 不支持。 |
| 校验 | 默认值和注解在 Schema 编译时检查；断言在配置值构造完成后执行。 |

本文规定 `.tschem` Schema 文件的词法、语法和静态语义。Schema 描述配置数据的类型、默认值和校验规则。配置实例的写法见[配置 DSL 规范](../config-dsl/)。

## 1. 词法规则

Schema 源文本为 UTF-8。token 之间允许的空白只有空格（`U+0020`）、水平制表符（`U+0009`）、LF（`U+000A`）和 CRLF（`U+000D U+000A`）。单独的 CR 非法；其他 Unicode 空白、垂直制表符和换页符也非法。LF 与 CRLF 均表示换行。

支持 `//` 行注释和 `/* ... */` 块注释。块注释不嵌套，必须闭合。字符串使用双引号，支持 `\"`、`\\`、`\n`、`\r`、`\t` 转义；其他反斜杠转义及未转义控制字符非法。字符串中的非 ASCII 字符按其 Unicode 内容保留。

标识符以 ASCII 字母或 `_` 开始，后续由 ASCII 字母、数字或 `_` 组成；标识符区分大小写。下列关键字不可用作声明名、字段名或路径段：`extern`、`mod`、`type`、`struct`、`entry`、`enum`、`option`、`map`、所有内建类型名、`true`、`false`、`none`。

整数是十进制数字串或小写 `0x` 前缀的十六进制数字串。未带前缀的数字按十进制解释，包括带前导零的数字。浮点数由小数点分隔的两段非空十进制数字组成。数字 token 不含正负号；前导 `+`、指数记法和数字分隔下划线均不支持。负号在允许负数的值位置及表达式中作为一元运算符解析。

## 2. 文件与声明

Schema 文件由顶层声明组成。根 Schema 必须包含且只能包含一个 `entry struct`；被导入的文件可以没有入口，也可以声明一个入口，该入口在导入处仅作为普通类型声明。`extern mod` 只允许出现在文件根作用域。

```text
schema_file       ::= top_level_item*
top_level_item    ::= extern_mod_decl | declaration | entry_decl
extern_mod_decl   ::= "extern" "mod" identifier ";"
entry_decl        ::= "entry" struct_decl
declaration       ::= module_decl | type_decl | struct_decl | enum_decl
module_decl       ::= "mod" identifier "{" declaration* "}"
type_decl         ::= "type" identifier "=" type annotation* ";"
```

`extern mod name;` 引入名为 `name` 的模块；模块内容来自对应的外部 Schema 文件。模块可以嵌套。模块和类型声明共享所在作用域的名称空间，同一作用域不得出现重名声明。声明顺序不影响类型引用的解析。

类型路径以标识符开头，可由 `::` 连接多个路径段。路径没有独立的绝对路径写法：解析器从当前模块开始，逐级向父模块查找首段，再从找到的模块解析剩余路径。因此，同一段路径在嵌套模块中可能解析到最近的同名声明；从根模块解析时，`common::network::Endpoint` 引用根模块下 `common` 模块中的类型。

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

类型别名使用 `type Name = Type;` 声明，可附加适用于目标类型的注解。别名透明，不引入新的运行时表示；引用别名时同时应用别名及使用位置的约束。别名不得形成循环。`@env` 仅适用于字段，不可附加于类型别名。

## 3. 类型、结构体与枚举

### 3.1 类型

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

整数类型为对应位宽的有符号或无符号整数。`f32` 和 `f64` 分别为 IEEE 754 binary32 与 binary64。`option<T>` 表示可选的 `T`；`[T]` 表示任意长度数组；`[T; N]` 表示长度恰为 `N` 的数组；`map<string, T>` 表示字符串键到 `T` 的映射。`N` 使用非负十进制整数，范围为 `0` 至 `u32::MAX`。映射键唯一。

容器元素类型可继续使用容器或用户声明类型。完全展开别名后不支持嵌套 `option<option<T>>`。用户声明类型可以通过字段、枚举载荷、容器及非循环别名直接或间接递归引用自身或其他用户声明类型，但类型图必须存在有限值。`option`、动态数组、map 和零长定长数组可以终止递归构造；递归枚举至少需要一个可构造变体。正长度定长数组要求元素类型可构造。没有有限分支的必需环（例如 `struct Loop { next: Loop }`）属于静态错误。类型别名仍不得形成循环。

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

### 3.2 结构体和字段

```text
struct_decl   ::= "struct" identifier "{" field_list? "}" struct_assert*
field_list    ::= field ("," field)* ","?
field         ::= identifier ":" type annotation* ("=" default_value)?
struct_assert ::= "@assert" "(" expression "," string_literal ")"
```

结构体可以为空。字段名在同一结构体中必须唯一。字段类型之后可有零个或多个注解，默认值（若有）必须位于注解之后。逗号分隔字段，末尾逗号可省略或保留。

### 3.3 枚举

```text
enum_decl       ::= "enum" identifier "{" variant_list ","? "}"
variant_list    ::= variant ("," variant)*
variant         ::= identifier variant_assert*
                  | identifier "(" type annotation* ")" variant_assert*
                  | identifier "{" field_list? "}" variant_assert*
variant_assert  ::= "@assert" "(" expression "," string_literal ")"
```

枚举至少包含一个变体；同一枚举中的变体名必须唯一。变体可以不带载荷、携带一个有类型的载荷，或携带一组匿名结构体字段。匿名结构体字段遵循普通字段的类型、注解和默认值规则。

变体后的断言附着于紧邻的变体，仅在该变体被选中时检查。结构体后的断言附着于该结构体。断言中的 `self` 表示当前结构体实例或变体载荷，`root` 表示入口结构体实例；入口结构体断言中的二者指向同一实例。单元变体没有载荷，不能在其断言中使用 `self`。断言在配置值构造完成后检查。

## 4. 默认值

字段可以使用 `= value` 声明默认值。默认值必须符合字段类型，并满足适用的约束。默认值不读取环境变量。

标量默认值遵循与配置字面值相同的严格类型规则：整数类型只接受整数 token，`f32` 和 `f64` 只接受浮点 token，字符串和布尔类型分别只接受对应字面值。整数 token 不会隐式转换为浮点默认值。浮点默认值直接舍入到字段的目标精度，采用 IEEE 754 最近值、平分取偶数规则；转换结果必须有限。

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

默认值中的负号是独立的一元前缀，不属于整数或浮点 token；负值只在默认值语法允许的位置解析。

对象默认值可省略有默认值或可选类型的字段；省略必填字段会使 Schema 无效。对象中不得出现未声明字段。数组、映射、对象及枚举载荷中的值均须符合各自目标类型。枚举默认值的载荷形式必须与该变体声明一致。`none` 仅可用于可选类型；可选类型字段也可直接写入其内部类型的默认值，此时表示存在该值。

默认值本身及其嵌套值在 Schema 校验时检查类型和约束；不合法的默认值使 Schema 无效。

## 5. 注解

注解可用于类型别名、字段、匿名结构体变体字段及单载荷变体的载荷类型，具体适用范围如下表。同一声明上不得重复同一种注解。别名上的约束与使用位置的约束共同生效。

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

| 注解 | 适用目标 | 语义 |
| --- | --- | --- |
| `@min(n)`、`@max(n)`、`@range(min, max)` | 数值类型，可选数值类型 | 对有值数值进行包含边界的比较；`@range` 的下界不得大于上界。整数目标使用整数边界。 |
| `@length(min, max)` | `string`，可选字符串 | 字符串 Unicode 标量值数量须位于闭区间；不进行 Unicode 归一化。 |
| `@regex("pattern")` | `string`，可选字符串 | 模式须合法；字符串中至少有一个匹配片段。需要整串匹配时使用 `^` 和 `$`。 |
| `@host` | `string`，可选字符串 | 校验 IP 字面地址或 DNS 主机名；不接受 scheme、端口或空白。 |
| `@email` | `string`，可选字符串 | 校验实用的 ASCII 邮箱形式：非空本地部分、一个 `@` 和合法 DNS 域名；不保证完整 RFC 5322 覆盖。 |
| `@cidr(v4_only = bool)` | `string`，可选字符串 | 校验 IP 地址和前缀长度；`true` 仅允许 IPv4，`false` 允许 IPv4 或 IPv6。 |
| `@env("NAME")` | 字符串、布尔或数值字段及其可选形式 | 声明字段可读取对应环境变量；只允许用于字段，不允许用于类型别名。 |
| `@size(...)` | 数组、映射 | 限制元素数量或键值对数量。位置参数为最小值和最大值；命名参数可指定 `min`、`max` 或两者。 |
| `@unique` | 数组 | 要求数组元素唯一。 |
| `@unique(by = "field.path")` | 结构体元素数组 | 要求指定字段路径的值唯一。 |
| `@all(expression, "message")` | 数组、映射 | 要求每个数组元素或每个映射值满足表达式。 |
| `@sum(...)` | 数值数组 | 按数组顺序求和，并检查总和是否满足指定的包含边界；可指定 `min`、`max` 或两者。 |
| `@secret` | 类型别名、字段或单载荷类型 | 将值标记为敏感值，在诊断和安全展示中隐藏其实际内容；别名上的标记传播到其使用位置。 |

`@min`、`@max`、`@range`、`@size` 和 `@sum` 的边界均包含端点。数值注解边界是上下文数值：整数边界可用于浮点目标，并直接按 IEEE 754 最近值、平分取偶数规则舍入到目标浮点精度；浮点边界不能用于整数目标。所有边界值及组合必须与目标类型相容；不适用的注解、重复注解、无效参数或不兼容组合均使 Schema 无效。

字符串相等以及 `@unique` 比较元素中的字符串值时，均按原始 Unicode 文本精确比较，不执行 Unicode 归一化、大小写折叠、DNS 根点移除或 IP 地址解析。`@host` 仅验证字符串格式，不改变相等或唯一性语义。DSL 表达式不提供主机规范化比较；需要这种比较时，宿主代码可显式调用 `ValueComparison::host_equal`：该方法对可解析的 IP 地址按地址比较，否则移除一个末尾点并按 ASCII 大小写不敏感方式比较文本。

### 5.1 正则语法与匹配

正则模式是经字符串转义解码后的文本，采用搜索匹配语义。

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
literal ::= 非转义的非元字符 Unicode 标量值
class_char ::= 除 `]`、反斜杠和 `-` 外的单个 Unicode 标量值
regex_metachar ::= `.` | `^` | `$` | `|` | `(` | `)` | `[` | `]`
                 | `{` | `}` | `*` | `+` | `?` | 反斜杠
```

`{m}` 表示恰好重复 `m` 次，`{m,}` 表示至少重复 `m` 次，`{m,n}` 表示闭区间重复；边界不得大于 255，且下界不得大于上界。整个模式和分组可以为空并匹配空字符串，但 `|` 分隔的任一选择分支不得为空。量词只能作用于一个消费原子；同一原子不能连续使用多个量词，锚点不能使用量词。字符类不能为空；范围端点必须是单个 Unicode 标量值，且起点不得大于终点。范围两端必须都是字面字符。未转义 `-` 仅可位于字符类首尾或作为范围符号；`^` 仅在字符类开头表示取补集。

`\d`、`\w`、`\s` 分别匹配 ASCII 数字、ASCII 字母/数字/下划线、ASCII 空白（空格及 `U+0009` 至 `U+000D`）；大写形式匹配 Unicode 标量值集合中的补集。普通字面字符和字符范围按 Unicode 标量值匹配，不执行大小写折叠、归一化或区域设置转换。`.` 匹配除 LF（`U+000A`）以外的任意一个 Unicode 标量值。锚点 `^`、`$` 分别表示整个输入的开头和结尾，不表示行边界。

反向引用、环视、命名捕获、内联标志、Unicode 属性转义、单词边界转义及未列出的转义不支持。分组只改变优先级，不捕获文本。

## 6. 断言与表达式

`@assert(expression, "message")` 可附着于结构体声明或枚举变体；`@all(expression, "message")` 使用相同表达式语言。表达式在 Schema 校验时做静态类型检查，结果必须为布尔值。未知字段、不可用方法、类型不匹配或非布尔结果都会使 Schema 无效。

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

运算优先级从高到低为：一元运算、乘除、加减、比较、`&&`、`||`。比较运算不得链式连续书写。字段路径通过点号访问字段；除 `@all` 绑定名和闭包参数外，路径必须以 `self` 或 `root` 开始。`@all` 中数组元素名为 `item`；映射项可使用 `key` 和 `value`。这些名称只在对应表达式内可用。

可用只读方法如下：数组和映射支持 `len() -> u64`、`is_empty() -> bool`；可选类型支持 `is_some() -> bool`、`is_none() -> bool`；字符串支持 `starts_with(string) -> bool`；枚举支持 `is_variant(string) -> bool`；数组支持 `any(|name| expression) -> bool` 和 `all(|name| expression) -> bool`；数值数组支持 `sum() -> element_type`。闭包参数仅在闭包表达式中可见，且不得使用 `self`、`root`、`item`、`key`、`value` 作为参数名。

布尔运算只接受布尔值。比较两侧必须为同一类型；数值字面量可根据另一侧的数值类型推断，否则整数按 `i128`、浮点按 `f64` 推断。浮点字面值按推断出的目标精度直接舍入，采用 IEEE 754 最近值、平分取偶数规则；整数 token 推断为浮点时，先精确解析整数，再直接转换到目标精度。表达式中的上下文推断不改变默认值的严格 token 类型规则。不执行隐式类型提升。相等比较支持同类型标量、枚举及可比较的可选值；结构体、数组和映射不支持相等比较。字符串相等按原始 Unicode 文本精确比较。有序比较只支持同类型数值和字符串，字符串按 Unicode 标量值词典序比较。算术只支持同类型数值；一元负号只支持有符号整数和浮点数。整数算术溢出或除零、浮点运算产生非有限值均为求值错误。`none` 只能与同类型可选值比较，不能解引用。

逻辑 `&&`、`||` 短路求值；`any` 和 `all` 在结果确定时停止访问后续元素，空数组上的 `any` 为假、`all` 为真。断言只能读取值，不修改配置或读取环境变量。运行时断言仅针对构造完成的实例和实际选中的枚举变体进行检查。

## 7. 静态有效性

Schema 只有在语法及静态语义均有效时才可用于配置解析。类型引用必须解析为结构体、枚举或有效别名；类型别名不得循环；递归的用户声明类型必须至少存在一个有限值。结构体字段、枚举变体以及同一命名空间中的声明不得重复。入口必须是结构体。

所有默认值必须符合声明类型及其约束。注解必须应用于支持的目标类型，参数和边界必须有效。断言和集合表达式必须通过静态类型检查。任一上述条件不满足时，Schema 无效。

## 8. 规范格式化

`SchemaFormatter` 只格式化语法有效的源文本，不解析导入，也不执行静态语义检查。除空白外，所有 token 与注释均按原字节和原顺序保留，包括数值与字符串写法。规范布局使用四空格缩进，展开非空声明体和复合默认值，将列表元素分行并添加尾逗号，声明之间保留一个空行。空大括号保持内联；类型和表达式保持紧凑。输出以换行符结尾；输入包含任意 CRLF 时统一使用 CRLF，否则使用 LF。Formatter 不重排声明或注释。
