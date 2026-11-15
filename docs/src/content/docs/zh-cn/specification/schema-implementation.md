---
title: "Schema 实现说明"
description: "说明 Typol 如何加载、编译和消费 .tschem Schema。"
sidebar:
  order: 4
---

## 本页要点

:::tip[适用场景]
了解 Schema 加载、编译诊断、外部导入，或编译类型如何供配置解析和 Rust
绑定使用时，请查阅本页。
:::

| 阶段 | 当前行为 |
| --- | --- |
| 输入 | 流式 lexer 和 parser 将 Schema 解析为带源码跨度的 AST。 |
| 编译 | loader 解析导入、检查语义，并生成不可变的 Schema。 |
| 使用方 | 配置解析和 Rust 绑定读取编译视图；宿主访问由适配器提供。 |

本文说明 `.tschem` 的当前实现，以及编译结果如何用于配置解析和 Rust 绑定。
[Schema DSL 规范](../schema-dsl/)定义语言行为，遇到语义问题时以规范为准。

## 1. 实现范围与架构

Schema 处理分为四层：

1. `typol-syntax` 对 UTF-8 输入执行词法和语法分析，生成拥有型 AST。
2. `typol-schema` 加载根源与导入源，解析声明并执行静态编译。
3. `typol-semantic` 保存不可变的编译模型，并提供只读视图。
4. `typol-config`、`typol` 和 `typol-std` 分别解析配置、编排应用 API，
   并提供宿主适配器。

核心语法和语义 crate 使用 `no_std + alloc`。Schema 编译通过
`SchemaResolver` 接收导入源，不直接访问文件；配置解析直接构造值，不建立
独立的配置 AST。`typol-macros` 只从 Rust 类型生成类型描述，运行时由
`TypedSchema::bind` 检查它与编译后 Schema 的兼容性。

不依赖标准库的应用集成方式见 [no_std 指南](../../guides/no_std/)。

| 职责 | 主要源码位置 |
| --- | --- |
| Lexer、token 游标、Schema AST/parser | `crates/typol-syntax/src/` |
| 导入、模块、符号和类型解析 | `typol-schema`：`loader/`、`module_tree.rs`、`symbols.rs`、`types.rs` |
| 注解、表达式、默认值和记录 | `typol-schema`：`annotations/`、`expressions/`、`defaults/`、`compiled/` |
| 编译模型与语义视图 | `typol-semantic`：`model`、`views`、`constraints`、`expressions` |
| 配置解析和校验 | `typol-config`：`parser.rs`、`validation.rs`、`value_validation.rs` |
| 宿主加载与 Rust 绑定 | `typol-std`：`schema_compiler.rs`；`typol`：`loading/`、`binding_impl.rs` |

## 2. 源输入、词法和语法分析

### 2.1 流式输入

Schema 源必须是有效 UTF-8，词法规则见
[Schema DSL §1](../schema-dsl/#1-词法规则)。`Read` 按顺序提供字节：短读不代表
输入结束，读到零字节才是 `EOF`。解析不要求输入可重放，也不会保留完整源文本。

Lexer 按需扫描，不生成完整 token 数组。`TokenCursor` 最多缓存当前事件和一个
前瞻事件；每个 token 都带有源 ID 和 UTF-8 字节范围组成的 `SourceSpan`。
标识符和数字 token 可提供源文本，字符串 token 保存解码结果，`EOF` 使用空跨度。

Token 与 AST 文本使用受预算约束的拥有型存储。输入可能无法重放，因此块字符串
只暂存当前 token 的正文；暂存内容与解码结果同时存在时都会计入额度。Token
字节上限包括分隔符和源换行，字符串上限则作用于解码后的内容。

共享 lexer 通过可复用窗口读取输入，不会暴露稳定的源文本切片。Token 和 AST 的
调试输出不展示字面值，只包含种类、跨度、长度等元数据。

### 2.2 AST 与解析结果

`SchemaParser::parse` 接收字节源、`SchemaParseOptions` 和 `ExecutionBudget`。
`parse_str` 将字符串适配为 `SliceSource`；若后续需要查找源码位置，则使用
`parse_with_positions`。该入口返回拥有型 AST 和 `SourcePositionIndex`，索引保存
字节偏移、行号及 Unicode 标量列号，不保留源字节。

`SchemaParseOptions::with_translator` 设置语法诊断的语言。Schema 编译也可通过
`SchemaCompileOptions` 设置语言；工具若分开执行解析和编译，应保持两处设置一致。
默认语言为英文，后续报告 writer 不会再次翻译诊断。

解析器会一直读取到 `EOF`，并以确认后的字节长度确定 AST 跨度。成功时返回完整
AST；词法或语法错误返回非空 `SyntaxErrors`，不会返回部分 AST。在可靠的声明、
模块、字段和变体边界，解析器会恢复并继续收集独立错误。

恢复扫描和 token 访问共用执行预算。资源耗尽、分配失败或内部错误会终止解析。
AST 节点是由所属 AST 持有的只读视图，调用方不能伪造节点 ID 或存储表。

AST 保留源顺序和跨度、类型路径与别名目标、注解参数、枚举载荷形式及原始数值
文本。它也保留大括号字面量的键是标识符还是字符串，以及表达式结构、路径、
方法调用和闭包参数。

解析阶段只检查语法，不解析名称、不检查类型兼容性或注解适用范围，也不执行
表达式。空 `{}` 会暂时保留为未判型的大括号字面量，之后由编译器根据目标类型
判定它表示对象还是 map。

### 2.3 限制与仅解析文档

`SchemaLimits::syntax()` 配置解析深度、AST 节点数、解码字符串、单源字节数和
诊断数。`SchemaLimits::stream_limits()` 设置 lexer 窗口和单 token 字节上限；
`StreamLimits::try_new()` 会拒绝零长度窗口。Loader 还限制所有源的总字节数和
文件数，成功编译要求每个源都读到 EOF。

默认 token 窗口为 8192 字节，token 上限为 1 MiB。默认语法限制包括深度 128、
100 条诊断、256 KiB 解码字符串和每文件 100,000 个 AST 节点。节点数包含恢复时
构造后又丢弃的节点；调用方可读取 `SchemaAst::node_count()` 和
`structure_height()`。

需要保留原文的工具可调用 `SchemaParser::parse_document`。它返回借用原始字符串、
并拥有成功 AST 的 `SchemaDocument`。此入口不解析导入或检查静态语义；编译器的
流式路径仍不会保留完整源文本。

`SchemaFormatter::format` 接收文档和格式选项，返回完整的 `FormattedSchema`。
它通过 AST 跨度识别分隔符，同时保留 token 与注释；格式化会统一布局和尾逗号，
保留块字符串原始字节，并根据输入选择 CRLF 或 LF。

Formatter 的工作区、输出大小和嵌套深度分别受限。调用方可通过
`FormattedSchema::as_str()` 读取结果，也可调用 `write_to()` 将完整结果写入
`fmt::Write` sink。

## 3. 编译会话与导入

### 3.1 编译入口

使用 `SchemaCompiler::new(SchemaCompileOptions)` 创建一次性编译会话。有导入时调用
`compile(root, resolver, observer)`；不需要导入的内存源可调用
`compile_without_imports(root)`。带 observer 的无导入入口可登记源名称。

`SchemaSource::from_source` 接收一次性字节 reader，`from_str` 则适配内存字符串。
源身份按字节精确比较，不写入编译后的 Schema。显式绑定到其他编译上下文的源会
以 `SCHEMA_CONTEXT_MISMATCH` 失败；上下文已结束时会以
`SCHEMA_SESSION_FINISHED` 失败。

`SchemaResolver::resolve` 接收当前源身份、单段模块名、剩余源字节额度和编译上下文。
宿主负责决定模块名如何映射到文件或内存源。`SourceObserver` 可登记已加载源的
身份与 `SourceId`，供宿主关联可读名称。

`typol_std::FileSchemaCompiler` 提供文件加载和相对导入，并在 parser 按需读取时
保持文件句柄有效。应用 facade 提供 `SchemaLoader::compile_file`、
`compile_text` 和 `compile_with_resolver`；加载后的 `LoadedSchema` 可绑定一次，
再重复读取多份配置。

### 3.2 导入图

根源使用 `SourceId(0)`，后续源按加载顺序分配 ID。Loader 使用显式栈按深度优先
读取导入；每条导入边都记录状态和可选目标，因此失败边不会指向有效源。

仍在加载的身份再次出现时会报告导入环；已加载的身份再次出现则报告重复导入。
即使内容相同，不同身份仍表示不同源。Loader 在解析前登记源，但会先按身份去重，
再读取重复源的正文。

文件 resolver 在可用时使用稳定文件 ID：Unix 使用设备号和 inode，Windows 使用
卷序列号和文件索引，因此硬链接共享身份。无法取得稳定 ID 时改用规范路径；
Linux 上还可从 `/proc/self/fd` 确定导入基路径。

读取过程同时检查单文件和总字节数、文件数及导入深度。为区分恰好达到上限与
已经超限，loader 最多额外读取一个探测字节。Schema 导入使用文件级
`extern mod`；普通 `mod` 声明则在同一源内建立命名空间。

### 3.3 共享预算

一个 `ExecutionBudget` 覆盖源加载、解析、语义检查、记录生成、索引、位置映射和
冻结。独立的 `RegexExecutionBudget` 覆盖模式编译和 Schema 默认值中的正则检查；
两类预算分别报告步数、工作区用量和失败原因。

`SchemaCompileUsage` 的 `steps`、`workspace_bytes`、`peak_workspace_bytes` 和
`failure` 表示通用工作用量；`regex_*` 字段表示正则用量。资源耗尽与分配失败
属于不同错误，阶段标签则标识当前通用编译阶段。

默认资源策略由 `SchemaLimits`、`SyntaxLimits`、`StreamLimits`、`ValueLimits`、
`RegexLimits`、`RegexExecutionLimits` 和 `ExcerptLimits` 分别配置。例如，默认
Schema 限制为单源 1 MiB、总源 8 MiB、最多 64 个文件。调用方可以覆盖这些运行期
策略；它们不是 DSL 语义。

## 4. 静态编译

`SchemaCompilation::compile` 按顺序执行以下阶段。任何诊断都会阻止可用的
`CompiledSchema` 返回；只有所有检查都通过，编译才成功。

### 4.1 解析、模块与符号

Loader 解析根源和每个导入源。存在语法错误的源不会进入模块树，但其他可加载的源
仍可用于收集诊断，除非发生终止性预算错误。

`ModuleTree` 将声明放入对应源模块，并递归加入普通 `mod` 声明。只有根文件的
`entry struct` 是配置入口；导入文件中的 entry 标记不会替代根入口。

`SymbolTable` 为模块、类型、字段、变体及匿名结构体载荷字段建立索引。它检查
声明、字段和变体重名，以及入口数量是否有效。查找结果会明确区分缺失、唯一命中
和歧义，不会静默选择重名项。

### 4.2 类型解析

类型路径首段从当前模块开始，逐级向父模块查找。首段解析后，其余路径段只能在
模块命名空间中解析。把类型用作中间模块、末段不是类型或符号无效都会产生语义
错误。

类型图为内建类型、用户类型、容器、别名和匿名结构体枚举载荷分配 ID。别名保留
自身记录和底层形态；使用点也能保留适用的别名约束及 `@secret` 元数据。

别名环单独检查。随后编译器通过受预算约束的工作队列，计算哪些用户类型能构造
有限值：结构体要求全部字段可构造，枚举只需有一个可构造变体。正长度定长数组
要求元素可构造，零长度数组则不要求。

无法构造有限值的递归组件会产生语义诊断。其他原因已无效的声明会排除在此检查外，
避免产生级联递归错误。字段和载荷解析前会先分配结构体与枚举 ID，使合法递归引用
保持为有限的 ID 边。

### 4.3 注解与表达式

注解附着在其后紧邻的递归类型表达式节点上。编译器将声明根注解分配给所属项，
将嵌套注解分配给 `TypeUse` 槽，并检查目标类型、参数、重复项、边界顺序及约束
之间的兼容性。

`@regex` 字符串由 `typol-regex` 编译为受限 Thompson NFA。编译期检查模式大小、
正则数量、指令数和字符范围；运行期校验复用编译好的程序。`@env` 只允许出现在
符合条件的标量或可选标量字段根节点，并生成授权元数据，不读取环境提供器。

`@secret` 标记精确的类型节点，用于诊断、展示和 Rust 转换时的脱敏；别名根标记也
会作用于该别名的使用点。`@variants` 会沿透明别名解析名称，并保存 `VariantId`；
未知或重复名称、目标类型无效及有效集合为空都会使 Schema 编译失败。

表达式编译器会类型检查 `@assert` 和 `@all`，包括路径、方法、运算符、字面值类型，
以及可用的 `self`、`root`、闭包和 map 上下文。表达式结果必须是 `bool`。
编译后的程序存入 `typol-semantic`；断言稍后执行，`@all` 则附着于对应数组或
map 的类型使用点。

### 4.4 默认值与冻结

默认值编译器根据已解析的字段类型构造 `ConfigValue`，并检查标量形态、数值范围、
浮点有限性、容器、对象、枚举载荷和适用约束。整数类型只接受整数 token，浮点
类型只接受浮点 token；`f32` 默认值直接舍入到 binary32。

对象默认值可省略有字段默认值或为 `option` 的字段。缺少必填字段、未知字段或字段
重复都会使 Schema 无效。默认值依赖使用显式帧并检查循环，包括递归 Schema 类型
导致的循环；默认值不会读取环境变量。

默认值通过校验后会产生 `SchemaDefaultValidation`。完整编译将它传给
`Builder::freeze_with_validated_defaults`，避免重复校验；直接调用
`Builder::freeze` 时没有这份凭证，仍会校验输入。默认值构造受值限制和通用预算
约束，正则检查则使用本次编译的正则预算。

`RecordBuilder` 将模块、符号、类型、约束、默认值、表达式和位置数据冻结为
`CompiledSchema`。它建立排序查找索引，同时保持字段和变体的声明顺序。调用方通过
只读 `TypeView`、字段、变体和规则视图读取结果；`CompiledSchema::entry()` 返回
根入口视图。

源码跨度仍是 UTF-8 字节范围。编译模型根据解析期索引保存 1-based 行列位置，列按
Unicode 标量计数。源码片段独立受限；编译器不保留完整源文本，也不会为补齐缺失
片段而重新读取源文件。

## 5. 诊断

Schema 失败会返回非空 `SchemaCompileErrors`，或不需要分配的紧急错误。诊断包含
稳定错误码、主要跨度、可选标签和位置状态；重名诊断可指出两处声明。

`Diagnostic::message()` 返回 `Result`，因此翻译失败会保留给调用方。
`Display` 无需先生成报告字符串，即可写出错误码、消息、可用位置和安全片段；
`Debug` 只显示数量和状态。`internal_cause()` 可读取结构化内部原因。

| 类别 | 错误码 |
| --- | ---: |
| 分配失败 | 2 |
| 词法错误 | 3 |
| Schema 语法错误 | 4 |
| 诊断数量到限 | 5 |
| 正则语法错误 | 6 |
| Schema 静态语义错误 | 7 |
| resolver 或宿主导入错误 | 8 |
| 根编译上下文不匹配 | 19 |
| 编译会话已结束 | 20 |

普通诊断按源 ID 和字节跨度稳定排序。资源、分配和内部错误优先作为终止诊断，
并为诊断数量到限保留槽位。核心诊断不会复制完整源码行、环境原值、敏感默认值、
宿主任意错误文本或真实文件路径。

语法错误由 `typol-syntax` 产生；导入、名称、类型、注解和表达式错误由 Schema
编译阶段产生。编译失败时必须停止配置解析，因为部分记录不可用于加载。

## 6. 配置解析

`ConfigParser::parse` 接收有效的 `CompiledSchema`、配置源和选项，以及
`Environment` 实现。解析器从 `schema.entry()` 开始，按预期类型读取 token 并直接
构造值树。大括号表示结构体还是 map，由编译后的类型决定。

结构体或匿名结构体载荷完成时，解析器应用显式值、字段默认值和 option 规则，并
报告未知、重复或缺失的必填字段。数组、定长数组、map 和枚举载荷按编译后的形态
检查；出现错误就不会返回部分值。

局部约束在值完成时检查，集合约束在数组或 map 结束时检查，完整树断言最后运行。
断言只检查当前激活的枚举变体。环境仅在配置显式写出 `env(...)` 且字段根
`@env` 已授权时读取；一次调用中每个名称只读取一次，但各引用分别转换和校验。

Schema 默认值不会来自环境。`ValueSource` 区分配置文本、Schema 默认值、环境值、
回退值和 Rust 转换值。运行期通过只读视图读取已编译约束和正则程序。

## 7. Rust 绑定与编解码

`#[derive(Typol)]` 描述 Rust 类型时不会打开 Schema 文件。`LoadedSchema::bind` 将类型
描述与 Schema 入口比对，并返回可复用绑定。普通绑定要求 `BudgetedTypol`；递归
派生类型按 Rust `TypeId` 与 Schema 视图配对，转换值仍受已配置额度约束。

手写 `Typol` 实现通过 `bind_trusted` 接入，返回类型会明确标记为受信任绑定。库管理的
工作仍受预算约束，但回调可以在额度外分配、阻塞或执行任意计算；手写解码器的
返回值也不会重新校验。`BudgetedTypol` 是能力标记，不是沙箱。

`Sourced<T>` 保留标量字段的值来源，也可保留环境引用的名称和回退元数据。它与 `T`
具有相同的 Schema 形态；引用只适用于获授权的标量或可选标量字段。该类型不实现
`Debug`，避免意外打印秘密值或回退值。

不启用 `std` 时可使用 `OrderedMap<T>`；启用 `std` 后可用 `HashMap<String, T>`，也可通过
可选 feature 使用 `hashbrown` 或 `btree-map` 实现。`OrderedMap` 按键排序，哈希 map
会在解码前预留容量。BTreeMap 使用经过检查的逻辑额度估算；由于没有可失败的
节点预留接口，节点分配仍遵循全局分配器。

文本解析会执行完整校验。外部 `ConfigValue` 规范化只检查形态并补齐默认值；
之后还需调用 `ConfigValueProcessor::validate` 检查约束和断言。Rust 序列化与上下文
Serde 也会校验完整值，因此 Rust 值不能跳过 Schema 规则。文本 writer 在写入前
校验，按 Schema 字段顺序输出，并提供紧凑和易读两种布局。

| Serde 数据模型操作 | `serialize_serde` | `deserialize_serde` |
| --- | --- | --- |
| `bool`、整数、有限浮点数、字符串 | 支持 | 支持 |
| `Option<T>` | 支持；拒绝嵌套 option | 支持；拒绝嵌套 option |
| `sequence` | 按 Schema 数组处理 | 按 Schema 数组处理 |
| `tuple` | 长度相同的 Schema 定长数组 | 长度相同的 Schema 定长数组 |
| `map` | 仅支持字符串键 | 仅支持字符串键 |
| 命名结构体 | 类型名和字段名必须匹配 | 类型名和字段名必须匹配 |
| 枚举变体 | `unit`、`newtype`、命名字段载荷 | `unit`、`newtype`、命名字段载荷 |
| 其他：`char`、`bytes`、`unit`、`tuple struct/variant`、`collect_str` | 拒绝 | 不适用 |
| `deserialize_any`/`ignored-any`、根级 `identifier`、`char`/`bytes`/`unit`、`tuple struct/variant` | 不适用 | 拒绝 |

桥接会通过稳定的 `DiagnosticContext::failure_point()` 标识报告不支持的操作、
用户回调拒绝和 Schema 形态不匹配。自定义 Serde 错误文本不会保存或展示。回调
仍属于用户代码信任边界，桥接只能计量自己控制的工作。

`typol-std` 提供文件、reader、进程环境和诊断适配器。`std` 与 `no_std + alloc`
构建共用相同的加载、绑定和错误类型；调用方显式传入 `Environment`。

## 8. 安全与源码片段

`@secret` 及从别名传播的标记会在诊断和安全展示中隐藏敏感消息，并将路径截断到
秘密边界。Rust 转换错误遵循相同策略。若路径无法安全映射，或脱敏时资源不足，
系统不会暴露原始路径。

源码快照只保存有界的安全行。发布片段前会遮罩已知默认值范围和秘密配置值；秘密
map 项同时隐藏键和值。若无法证明遮罩完整，系统会省略片段，不会输出原始内容。

片段限制覆盖总字节数、单行字节数、记录数和上下文行数。列按 Unicode 标量计数，
制表符占一列，CRLF 不计入行文本。截断会保持 UTF-8 边界；缺少片段时不会回读源
文件来补齐。

成功值和序列化输出仍包含秘密数据。安全展示和诊断脱敏不等于加密；应用仍须保护
自己保存或写出的实际值。

## 9. 维护

维护时应覆盖 lexer 与 AST、导入图、名称和类型解析、递归类型、注解、默认值、
表达式类型、正则限制、诊断、资源失败、运行期校验及 Rust 绑定。断言结果时应
比较稳定错误码、位置和结构化数据，不依赖调试字符串。

若实现与 Schema DSL 冲突，应先修订规范或决策记录，再修改 parser 或编译器语义。
实现文档描述当前源码，不独立确立语言规则。
