---
title: "Config IR v1 规范"
description: "定义 Typol Config IR v1 JSON 格式、确定性输出和兼容规则。"
sidebar:
  order: 5
---

## 本页要点

:::tip[适用场景]
消费或生成 Config IR v1 JSON，并需要确认值形状、排序或校验行为时，请查阅本页。
:::

| 主题 | 关键规则 |
| --- | --- |
| 顶层对象 | 成员依次为 `format`、`version`、`root_type`、`value`；成功输出以一个 LF 结束。 |
| 导出模式 | 默认 `Validated` 会校验约束；`WithDefaults` 补默认值但不校验约束；`AsProvided` 两者都跳过。 |
| 敏感值 | `secret: true` 仅作标记，不会遮蔽或加密数据；消费者必须保护整个标记子树。 |

本文定义 `typol-config-ir` JSON 格式第 1 版（Config IR v1）的输出结构、排序、敏感值标记和资源边界。Config IR 是编译后 Schema 与完整 `ConfigValue` 的导出格式，不是 `.tconf` 配置语法。配置值的语法和语义见[配置 DSL 规范](../config-dsl/)，Schema 类型和约束见[Schema DSL 规范](../schema-dsl/)。

## 1. 格式标识与导出边界

Config IR 是 UTF-8 JSON 文本。顶层对象恰有以下成员，并按所列顺序输出：

| 成员 | JSON 类型 | 含义 |
| --- | --- | --- |
| `format` | 字符串 | 固定为 `typol-config-ir`。 |
| `version` | 整数 | 固定为 `1`。 |
| `root_type` | 字符串 | 入口结构体的 Schema 限定名。 |
| `value` | 对象 | 按入口结构体类型编码的根值。 |

每个成功输出末尾恰有一个 LF（`U+000A`），包括紧凑和美化输出。输出不含 BOM。美化输出只改变 JSON 空白，不改变数据模型。

v1 输出完整配置值，不包含源位置、值来源、环境引用名称或 fallback、默认值来源、约束定义或校验结果。省略引用元数据并不代表解析后的值已脱敏：环境变量实际值以及 `@secret` 下的值仍会作为数据导出，必须按敏感信息保护。类型别名透明：输出描述别名展开后的类型形态，结构体和枚举保留 Schema 限定名。`map<string, T>` 的键类型固定为字符串，不在 map 类型描述中单独编码。

`ConfigIr::serialize` 不读取环境变量。调用方可通过 `ConfigIrOptions` 选择序列化前的配置值处理模式：

| 模式 | 默认值补全 | Schema 约束与断言 | 用途 |
| --- | --- | --- | --- |
| `Validated`（默认） | 执行 | 执行完整校验 | 补齐默认值后检查字段、类型、标量和集合约束，以及结构体和活动枚举变体断言。 |
| `AsProvided` | 不执行 | 不执行 | 显式跳过规范化和约束校验，直接编码已完整构造的值，并检查本规范列出的值形态。 |
| `WithDefaults` | 执行 | 不执行 | 按 Schema 补齐缺省字段、可选值、嵌套结构体和活动枚举结构载荷；显式值（包括显式 `none`）优先保留。 |

`WithDefaults` 只规范化值，不会因用户显式值违反 Schema 约束而失败；但后续序列化仍会执行值类型、整数范围、定长数组长度及载荷形态检查。`Validated` 使用相同的配置资源与执行额度设置，且约束检查总是在补齐默认值之后进行。所有模式都不改变 IR v1 的输出结构。

## 2. 类型描述

每个值节点的 `type` 成员包含一个类型描述对象。类型描述只使用与类型形态对应的成员：

| Schema 类型 | 类型描述 |
| --- | --- |
| `i8`、`i16`、`i32`、`i64`、`i128`、`u8`、`u16`、`u32`、`u64`、`u128`、`f32`、`f64`、`string`、`bool` | `{"kind":"<内建类型名>"}` |
| `option<T>` | `{"kind":"option","inner":T 的类型描述}` |
| `[T]` | `{"kind":"array","element":T 的类型描述,"length":null}` |
| `[T; N]` | `{"kind":"array","element":T 的类型描述,"length":N}` |
| `map<string, T>` | `{"kind":"map","value":T 的类型描述}` |
| 结构体 | `{"kind":"struct","name":"<限定名>"}` |
| 枚举 | `{"kind":"enum","name":"<限定名>"}` |

限定名使用编译后 Schema 提供的名称，并以 `::` 分隔模块路径；根模块中的名称不带前缀。匿名结构体枚举载荷没有独立类型描述，其字段直接编码在载荷中。

递归 Schema 类型的描述仍是有限的：结构体和枚举的描述在限定名义类型名称处停止。递归 Config 实例仍作为普通嵌套值节点输出，现有 `max_nesting_depth` 继续限制每条值边。该行为不改变 Config IR v1 的格式或版本。

## 3. 值节点

每个值节点都是 JSON 对象，先输出 `type`，再输出与类型对应的数据成员；当该节点为敏感值时，最后输出 `secret`。`secret` 为假时省略该成员，不输出 `false`。

### 3.1 标量和可选值

| 类型 | 值节点成员 | 编码规则 |
| --- | --- | --- |
| 整数 | `value` | 十进制 JSON 字符串，不带分组符或进制前缀；保留完整有符号/无符号整数精度。 |
| `f32`、`f64` | `value` | 有限浮点值按 Rust `Display` 格式化后的 JSON 字符串；负零统一写为字符串 `"-0"`。非有限值无法编码。 |
| `string` | `value` | JSON 字符串，内容为配置值中的 UTF-8 文本。 |
| `bool` | `value` | JSON 布尔值 `true` 或 `false`。 |
| `option<T>` | `value` | 有值时为按 `T` 编码的完整值节点；空值时为 JSON `null`。 |

数字使用字符串是 v1 的固定规则，消费者不得将整数或浮点载荷假定为 JSON 数字。整数的字符串形式保留 `i128` 和 `u128` 的完整范围。浮点载荷必须有限；`NaN` 和正负无穷均导致导出失败。

### 3.2 数组和 map

数组节点使用 `items` 数组，每项是按元素类型编码的值节点。元素顺序与 `ConfigValue` 中的顺序一致。定长数组的项目数必须等于 Schema 声明的长度。

map 节点使用 `entries` 数组，每项为 `{"key":"<字符串键>","value":<值节点>}`。条目按键的字符串升序排列，以保证相同输入得到确定性输出；排序不改变 map 的语义。键按 Unicode 内容精确比较，不进行 Unicode 归一化。map 值按其 Schema 值类型编码。

### 3.3 结构体字段

结构体节点使用 `fields` 数组。每项为 `{"name":"<字段名>","value":<值节点>}`。字段按 Schema 中的声明顺序输出，与 `ConfigValue` 中字段的存储或输入顺序无关。每个 Schema 字段必须恰有一个对应值；缺少字段、额外字段或类型不匹配均不能导出。

### 3.4 枚举

枚举值节点依次包含 `type`、`variant` 和 `payload`。`variant` 是所选变体的名称。`payload` 按变体形态编码：

| 变体载荷 | `payload` 对象 |
| --- | --- |
| 无载荷 | `{"kind":"unit"}` |
| 单值载荷 | `{"kind":"single","value":<值节点>}` |
| 匿名结构体载荷 | `{"kind":"struct","fields":[字段项...]}` |

匿名结构体载荷的字段项与普通结构体字段项相同，并按其 Schema 声明顺序输出。未知变体、载荷种类不符或载荷值形态不符均不能导出。

## 4. 敏感值标记

当 Schema 对相应类型或字段标记了 `@secret` 时，序列化器在对应值节点上输出 `"secret":true`。类型别名传播的敏感标记按编译后 Schema 的有效类型标记处理。单载荷枚举变体的载荷类型标记为敏感时，标记位输出在所选枚举值节点上。字段标记作用于该字段的值节点。

敏感标记不删除、不遮盖或加密值。即使输出了 `secret: true`，同一节点的 `value`、`items`、`entries`、`fields` 或 `payload` 仍保留真实内容。消费方必须把带有 `secret: true` 的节点及其整个子树按敏感数据处理；子节点根据各自的 Schema 标记仍可能单独带有 `secret: true`。

## 5. 示例

以下示例展示整数字符串、空可选值、按键排序的 map 和无载荷枚举：

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

## 6. 确定性与 JSON 排列

紧凑输出不包含无意义空白；美化输出使用 JSON pretty 格式。两种模式都按以下顺序写对象成员，以保证实现输出稳定：

- 顶层：`format`、`version`、`root_type`、`value`。
- 类型描述：先 `kind`，再按类型写 `inner`、`element`/`length`、`value` 或 `name`。
- 标量/可选/数组/map/结构体值节点：`type` 后依次为 `value`、`items`、`entries` 或 `fields`，如有则 `secret` 最后。
- 枚举值节点：`type`、`variant`、`payload`，如有则 `secret` 最后。
- 字段项：`name`、`value`；map 项：`key`、`value`。
- 枚举载荷：`kind` 后按形态输出 `value` 或 `fields`。

数组保留原有顺序；结构体字段采用 Schema 声明顺序；map 按键升序。对象成员次序规定序列化器的确定性输出；消费者解析 JSON 时应按成员名称识别数据，不依赖对象成员的先后顺序。

## 7. 资源额度与失败

`ConfigIr::serialize_to` 由 `typol` crate 的 `config-ir` feature 提供，会将 `ConfigValue` 写入调用方提供的 sink；`serialize` 是基于该 sink 接口的 `Vec<u8>` 便捷封装。前置规范化或校验成功后才开始 JSON 编码；sink 或输出额度错误可能留下已写入的前缀。`ConfigIrOptions` 的默认值如下：

| 选项 | 默认值 | 作用 |
| --- | ---: | --- |
| `pretty` | `false` | 选择紧凑或美化 JSON。 |
| `max_output_bytes` | 16,777,216 字节 | 限制最终 JSON 字节数，包含末尾 LF。 |
| `max_workspace_bytes` | 16,777,216 字节 | 限制 map 和结构体字段排序所用的临时引用工作区。 |
| `max_nesting_depth` | 128 | 分别限制值节点和嵌套容器类型描述的递归深度。 |
| `value_mode` | `Validated` | 选择默认值规范化和 Schema 校验策略。跳过校验必须显式选择 `AsProvided`。 |
| `validation_options` | `ConfigValidationOptions::default()` | 仅在 `WithDefaults` 和 `Validated` 中使用，控制配置资源额度与执行预算。 |

调用方可覆盖输出和工作区额度。嵌套深度可以调低，但最大固定为 128；设置为 0 时，根值或根类型描述无法进入，导出失败。工作区额度按排序所需引用项的逻辑字节数计量，不代表进程的总内存上限。

JSON 输出缓冲区从 8 字节开始按倍数增长，每次预留的目标容量不超过 `max_output_bytes`，并在追加前检查最终字节数。缓冲区不会为 serde 的每个小写入片段单独预留空间；输出上限仍包括末尾 LF。该上限约束输出长度和缓冲区请求容量，不构成整个进程的峰值内存上限；扩容期间分配器可能同时持有旧、新内存块，排序工作区也由独立额度控制。

导出错误分类如下：

| 错误 | 含义 |
| --- | --- |
| `SchemaAccess` | 无法读取编译 Schema 的必要信息或限定名。 |
| `ValueShapeMismatch` | 值与 Schema 形态不匹配，包括标量类型/整数范围、定长数组长度、结构体字段集合或枚举载荷不匹配。 |
| `UnsupportedFloat` | 浮点值为 NaN 或正负无穷。 |
| `OutputLimitExceeded` | 最终 JSON 输出超过 `max_output_bytes`。 |
| `WorkspaceLimitExceeded` | map 或结构体字段排序所需的临时引用工作区超过 `max_workspace_bytes`。 |
| `NestingLimitExceeded` | 值节点或类型描述超过嵌套深度额度。 |
| `InvalidNestingDepthLimit` | 请求的嵌套深度超过固定安全上限。 |
| `AllocationFailed` | 创建输出缓冲区或临时排序向量时分配失败。 |
| `SerializationFailed` | JSON 序列化发生其他失败。 |
| `NormalizationFailed` | `WithDefaults` 规范化失败；错误携带经敏感信息策略处理的 `ConfigValueErrors` 诊断。 |
| `ValidationFailed` | `Validated` 的规范化或 Schema 校验失败；错误携带经敏感信息策略处理的 `ConfigValueErrors` 诊断。 |

`ConfigIrError::kind()` 返回 `ConfigIrErrorKind`，可用于稳定地比较错误类别；它不表示完整错误值相等。`ConfigIrError` 本身不实现 `PartialEq` 或 `Eq`，以免忽略规范化/校验错误携带的诊断差异。规范化和校验错误仍分别携带 `ConfigValueErrors`，调用方可通过错误变体或 `Error::source()` 读取安全诊断及可用的 Schema 位置；诊断不会输出敏感值。序列化错误不包含被序列化的配置值。实现可以调整内部存储方式，但成功输出的 v1 数据形状和上述额度/失败语义保持不变。

## 8. 版本兼容

消费者应同时检查 `format` 和 `version`，不得把未知版本按 v1 解释。v1 的字段含义、值节点形状、数字字符串编码、排序和敏感标记语义均为格式契约；与这些契约不兼容的修改必须发布新的 IR 版本。
