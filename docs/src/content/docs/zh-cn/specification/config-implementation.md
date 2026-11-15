---
title: "Config 实现说明"
description: "说明 Typol 如何解析、构造、校验 .tconf 值并安全报告错误。"
sidebar:
  order: 3
---

## 本页要点

:::tip[适用场景]
需要追踪配置读取经过的解析器、环境访问、校验流程或资源限制时，请查阅本页。
:::

| 环节 | 当前行为 |
| --- | --- |
| 解析 | 值读取增量消费 token，不保留 AST；文档读取还会保留有界源码和解析器生成的值跨度。 |
| 环境变量 | 只有遇到已授权的 `env(...)` 表达式时，才调用调用方提供器。 |
| 格式化 | `ConfigFormatter` 使用显式分隔符栈扫描已校验文档，并保留源码 token 与注释。 |
| 限制与错误 | 解析、校验、诊断和执行使用显式资源额度，并通过结构化错误报告失败。 |

本文说明 Typol 当前如何实现 `.tconf`：解析入口、流式输入、Schema 指导下的值构造、环境
读取、校验、错误恢复和安全诊断。本文补充[Config DSL 规范](../config-dsl/)及
[Schema 实现说明](../schema-implementation/)，不改变 DSL 语义。

本文依据当前源码整理。Config DSL 与 Schema DSL 规范定义语言语义；资源默认值以源码为准。

## 1. 实现范围与边界

配置处理由四层协作完成：

1. `typol-syntax` 按需从 `Read` 读取 UTF-8 字节，并产生带源跨度的 token。它不会为配置
   构造完整 token 数组。
2. `typol-config` 根据 `CompiledSchema` 的只读视图读取 token，并构造拥有型 `ConfigValue`。
   文档入口还保留原文及同一次解析产生的位置索引，不会重新扫描源码来定位值。
3. `typol-semantic` 提供类型、字段、变体、约束、断言、环境授权及已编译正则的只读视图，
   并负责表达式执行。
4. `typol` 将解析与 Rust 转换、Serde、加载会话及可选 Config IR 导出组合起来；
   `typol-std` 提供文件输入和进程环境适配器。

核心实现使用 `no_std + alloc`。它不打开文件、不读取进程环境，也不向终端输出。
环境访问只通过调用方传入的 `Environment` 实现完成；成功值拥有自己的字符串、键和值，
不借用输入或提供器缓冲区。

`no_std + alloc` 应用集成方式见 [no_std 指南](../../guides/no_std/)。

| 职责 | 主要源码位置 |
| --- | --- |
| 公共入口、请求、额度和会话 | `typol-config/src/api.rs`、`options.rs`、`session.rs` |
| Schema 指导的解析状态机 | `parser.rs` 和 `parser/` |
| 错误恢复和 map 重复键索引 | `parser/recovery.rs`、`parser/map_index.rs` |
| 环境访问、缓存和标量转换 | `environment.rs`、`scalar.rs` |
| 默认值、外来值规范化和实例路径 | `value_normalization.rs`、`instance.rs` |
| 标量、集合、断言和完整树校验 | `validation.rs`、`validation/`、`value_validation.rs` |
| 诊断、路径裁剪和秘密值隐藏 | `diagnostics.rs`、`secret.rs` |
| 源格式化和注释扫描 | `document.rs`、`document/formatter.rs`、`document/formatter/format_scan.rs` |
| token 流和 UTF-8 输入 | `typol-syntax/src/input.rs`、`lexer.rs`、`cursor.rs` |
| 值来源和执行额度 | `typol-core/src/value.rs`、`budget.rs` |
| 宿主适配器和 facade | `typol-std` 文件/环境模块；`typol` 的 loading、`binding_impl` 和 codec 模块 |

## 2. 调用入口与会话

### 2.1 配置文本解析

通用调用方使用 `ConfigParser::new(schema).parse(request, environment)`。成功时返回
`ValidatedConfigValue`，表示值约束和完整树断言均已通过。调用方可借用其中的
`ConfigValue`；需要不带验证标记的拥有值时，再调用 `into_config_value()`。

`ConfigParseRequest` 包含可读 `Read`、显示名称和 `ConfigParseOptions`。选项配置
`ExcerptLimits`、`RegexOptions`、`ConfigLimits`、通用执行额度、词法流 `StreamLimits`
及 `Translator`。外来值校验选项也携带翻译器；默认语言为英文，应在解析或校验前选择语言。

Schema 和 Config 共用块字符串 lexer 规则。`Read` 不保证支持回放，因此 lexer 会将当前
块 token 正文暂存到计费工作区，再计算缩进并解码到第二个计费缓冲区。扫描计入共享执行额度。

源 token 额度包括分隔符、闭合缩进及原始 CRLF 字节；字符串额度只计算最终解码后的
UTF-8 字节。显示名称是诊断元数据，不属于配置输入。

`ConfigParser::parse_document` 和 `LoadedSchema::read_document` 保留 parser 的 UTF-8 源码
缓冲区及语义位置索引。文档直接接管该缓冲区，避免再复制一份完整源码；首次编辑前，源文本
本身也作为 `DocumentSnapshot` 写入期望值。

首次编辑成功时，旧源码缓冲区会转为计费快照，不再分配第二份文本。成功写入后，期望值更新为
当前源码，并释放独立快照。

`ConfigDocument::write_to` 原样写出保留文本。`apply_edits` 通过解析器生成的位置索引处理带类型
的替换、插入和删除，并按父容器分组索引，只为每个受影响容器准备一次边界、条目位置、逗号和布局。

编辑器拒绝重叠补丁，组装一个候选文档，再重新解析并校验后提交。重解析只记录位置，不保留第二份
候选源码；校验成功后，候选缓冲区成为文档的新源码。

替换会编码目标子树；插入会推断容器换行、缩进和键间空格，并直接将值编码到最终补丁缓冲区。
map 项检查使用已解析的父节点，不会克隆完整路径。

删除和复合替换会保留受影响范围内的注释字节，未触及范围则逐字节不变。编辑数量、排序、索引遍历
和有界源码扫描均计入共享执行额度。

相关修改应通过一次 `apply_edits` 或 `replace_values` 批量提交，使候选文档只解析和校验一次。
分开的 `set_value` 调用会分别承担这项成本；编辑器将两种字符串都视为不透明范围，避免块字符串
中的分隔符和注释标记干扰扫描。

`set_value` 使用 `ConfigReplacement` 封装路径和值，`insert_field` 使用 `ConfigFieldInsertion`。
默认布局为 `Compact`；`with_layout(TextLayout::Pretty)` 只改变新值片段的空白，`remove` 使用默认选项。

Pretty 布局保持 `ConfigValue` 条目顺序，不规范化外围文档。非空数组、map 和结构体每项单独一行，
逐层缩进四个空格并使用尾逗号；空容器保持内联。单值枚举载荷使用多行圆括号且不加尾逗号；
unit variant 和 `env(...)` 保持紧凑。

若源码任意位置含 CRLF，新片段使用 CRLF，否则使用 LF；片段末尾不附加文件级换行。替换缩进以目标行
的前导空格或 Tab 为基线，并为该行此前仍打开的编码容器各增加四个空格。

新插入行使用容器布局选定的缩进；Compact 编码及外围插入标点保持不变。Facade 导出
`ConfigEditOptions`；`TextLayout` 可从 `typol_config`、`typol`、`typol::core` 和
`typol::config` 使用。

候选文本同时受 `max_source_bytes` 和 `max_constructed_text_bytes` 限制。分配前，编辑器按候选
全文长度计费一次；补丁字节不重复计费，校验期间 parser 新建的文本仍按常规路径计费。

候选解析若报告 allocation 或 resource/capacity 诊断，编辑错误会保留对应终止类别及完整的
`ConfigParseErrors` 集合。

`ConfigFormatter::format(&document, options, out)` 只格式化成功解析的文档。它用显式分隔符栈扫描
保留源码，不构造配置 AST，也不重新编码 `ConfigValue`。`FormattedConfig` 独立拥有完整结果，
并提供只读视图。

`ConfigFormatter::is_formatted` 使用相同 formatter，逐段比较输出和原文，不分配完整输出缓冲区。
CLI 的 `config fmt --check` 使用此路径；格式化不会修改文档或源码位置。

格式化按顺序保留所有非空白 token 和注释，包括字符串、数值写法及 `env(...)` 表达式。块字符串作为
不透明 token，其字节、缩进和 LF/CRLF 写法保持不变；格式化具有幂等性。

非空容器按每行一个条目展开，缩进四个空格并添加尾逗号。空容器保持紧凑，含注释时例外。
单值枚举载荷使用多行圆括号且不加尾逗号；`env(...)` 调用保持紧凑。

`ConfigFormatOptions` 设置输出字节、formatter 工作区和嵌套深度额度。格式化还受文档源码及构造文本
额度约束；栈空间和扫描工作计入共享执行额度，不提供多种规范格式的样式开关。

`format` 会构造并计量拥有型输出缓冲区；`is_formatted` 使用相同输出上限逐段比较。连续注释串的
尾逗号前瞻只执行一次，因此含 `k` 条注释的串只增加线性扫描工作。

只有扫描、边界检查和输出计量全部成功后，formatter 才返回拥有型结果。结构错误、资源超限、分配失败
和 sink 错误类别各自独立；不完整缓冲区不会作为成功结果返回。

`LoadedSchema::read_file` 返回 `ConfigDocument`；只需要已校验值时使用 `read_file_value`。
启用 `std` 后，`LoadedSchema::write_file` 使用文档快照期望值，并同时限制源码和文件输入字节数。

本地文件系统不能原子比较文件字节并替换路径，因此 adapter 对 `ExactSnapshot` 返回
`AtomicReplaceUnsupported`。新目标使用 `MustBeAbsent`；有意覆盖时使用 `ReplaceAny`。Unix 通过
同目录硬链接创建新目标，Windows 使用同卷且不覆盖的写穿透移动。

两种新建操作都不会覆盖并发创建的目标。`ReplaceAny` 使用原子替换；Unix 会在提交后同步父目录，
无法确认持久性时返回专门错误；Windows 使用同卷写穿透移动。`no_std` 调用方需为 `SourceWriter`
实现所选期望值要求的语义。

`ConfigValueProcessor::validate` 和 `normalize` 接收 `ConfigValidationRequest`。其选项配置源码片段、
Regex、Config 和通用执行额度，不含 token 流。校验与规范化不会读取配置输入或环境提供器。

`TypolOptions` 和 `SchemaLoader::new(TypolOptions)` 在 `no_std` 与 `std` 构建中共用。
`LoadedSchema::read_value` 和 `BoundSchema` 读取方法接收 `ReadRequest<R>`；reader 遵循 lexer
使用的 `typol_core::Read` 契约。

内存文本可使用 `ReadRequest::from_text`；`no_std` 调用方也可传入 `SliceSource` 或自定义 reader。
所有读取都显式接收 `Environment`。启用 `std` 后，`read_file` 使用 `FileConfigInput`，
`read_io` 通过 `ReadSource` 适配 `std::io::Read`，`SchemaLoader::compile_file` 则适配文件 Schema。

需要读取进程环境时，应传入配置了 `ProcessEnvironmentLimits` 的 `ProcessEnvironment`。额度限制变量名
和值，但值长度是在 `var_os` 返回拥有型 `OsString` 后才检查，无法限制该查询的临时分配。

启用 `std` 后，`TypolOptions` 也携带宿主额度。配置文件输入使用 `with_file_input_limits`；文件
Schema、来源名称和名称编辑分别使用 `with_files`、`with_names`、`with_name_edits`。
`with_config_limits` 和 `with_binding_limits` 分别调整配置解析和绑定额度。

配置策略可从 `typol::config` 获取，宿主适配器位于 `typol::host`。文件输入额度限制配置文件和路径
字节；Schema 导入使用 `FileSchemaLimits`，执行额度仍由 Schema/Config 选项管理。

`ConfigLimits` 提供有限默认值，以及具名、可链式调用的 `with_max_*` 更新方法。字段保持私有，没有
位置参数构造器或可变 setter，外部可通过只读 getter 查询。配置可传给 `ConfigParseOptions::with_config`；
编解码选项也使用具名更新方法。

`ConfigParser::parse` 会为调用创建通用 `ExecutionBudget`。要与外层加载、绑定或编解码共用非 Regex
额度，可调用 `parse_with_budget`。传入额度必须与请求中的通用执行额度完全一致，否则返回内部配置错误。

解析、环境处理、校验和诊断路径处理共用通用额度。Regex 检查则共用由
`ConfigParseOptions::regex()` 创建的独立 `RegexExecutionBudget`；Regex 编译发生在 Schema 编译期。

### 2.2 外来值校验

`ConfigValueProcessor::validate` 接收拥有型 `ConfigValue`，按 Schema 复制并规范化后，执行与文本路径
相同的约束和断言。它不接收 `Environment`，因此不会读取进程环境。

需要共用通用执行额度时使用 `validate_with_budget`；Regex 检查仍使用
`ConfigValidationOptions::regex()` 创建的独立额度。仅规范化操作不会创建或消耗 Regex 额度。

规范化会检查值形态、字段名、枚举载荷和标量目标类型；按 Schema 字段顺序重建结构体，补齐默认值和
无默认值的可选字段，并创建新的拥有树。

缺少必填字段、未知或重复字段、无效 map 键/值及错误载荷形态都会失败。map 没有 DSL 顺序语义；
外来值路径会按键排序，以稳定地重建结果。

仅规范化会拒绝带环境来源的值，因为它不执行完整授权和 fallback 校验。外来值需要保留环境来源时，
应使用 `validate`。

### 2.3 根据 Schema 默认值生成配置

`LoadedSchema::default_config_to_string` 和 `write_default_config` 根据编译后的入口 Schema 生成
`.tconf`，无需绑定 Rust 类型或接收 `Environment`。Facade 创建空结构体根值，并通过
`ConfigValueProcessor` 校验一次。

此校验会补齐默认值，并检查字段规则、集合规则和完整树断言。生成错误通过 `TypolError` 返回，
阶段为 `Encoding`；值诊断可从 `codec_errors().config_value_errors()` 查询。

对每个缺失字段，共用补齐规则依次选择显式 Schema 默认值、无默认值 `option<T>` 的 `none`，
或带字段路径和 Schema 定义位置的必填缺失诊断。只有入口结构体会被合成为根值。

嵌套结构体、数组、map 或枚举不会仅因内容看似可构造就自动生成，字段本身必须有显式默认值。
选定嵌套对象或枚举载荷后，再递归补齐其字段。`@env` 只授权输入表达式，生成过程不会执行它。

`DefaultConfigOptions` 默认使用 `Compact`、1 MiB UTF-8 字节上限，并允许输出真实 `@secret` 值。
`with_layout(TextLayout::Pretty)` 使用 `config fmt` 的四空格规范布局，复合值分多行并以 LF 结尾。

已移除 `with_pretty(bool)` 和 `pretty()`，属于源码兼容性破坏。调用方应改用
`with_layout(TextLayout)` 和 `layout()`。选项还可拒绝带秘密标记的值；拒绝检查在 sink 写入首字节前
完成，返回不含值文本的编码诊断。

允许输出时，秘密仍是明文，必须按敏感配置保护。字符串 API 只在校验和编码完整成功后返回；若编码、I/O、
执行额度或输出上限失败，sink API 可能已写入前缀。

要原子创建或替换文件，应先生成完整字符串，再将字节交给
`FileSourceWriter::replace_atomically`，并显式选择 `SourceExpectation`。
`LoadedSchema::write_file` 操作的是 `ConfigDocument` 源快照，不是默认配置生成 API。

## 3. 流式解析与值构造

### 3.1 输入和根值

解析器为配置源分配 `SourceId`，从 `schema.entry()` 取得根类型，并确认其为结构体。Lexer 通过
`Read` 的固定大小窗口读取；token cursor 缓存当前事件和至多一个前瞻事件。流式输入维护行列位置，
token 跨度使用 UTF-8 字节偏移。

根值按结构体对象解析。对象闭合后，解析器继续读取到 EOF；任何多余 token 都会使输入无效。
一次调用只接受一个根对象，后面只能跟 DSL 认可的空白和注释。Token 直接消费为值，不保留配置 AST。

### 3.2 按 Schema 类型解释 token

`ValueParser` 使用受额度约束的显式帧栈处理嵌套值，不把输入深度映射为宿主递归调用。
每个帧保存当前 Schema 类型及容器或载荷的续接状态。

- 标量消费精确匹配内建 Schema 类型的字面值。字符串使用 lexer 解码结果；整数检查目标位宽，
  浮点数转换后必须有限。
- `option<T>` 接受 `none`，或解析一个 `T` 后包装；除此之外不做隐式转换。
- 数组按输入顺序构造元素，并在闭合时检查定长数组长度。
- 结构体和匿名结构体载荷通过 Schema 字段索引匹配未加引号的字段名；输入字段可任意排序。
- map 只接受字符串键。每次调用使用 AVL 索引检测解码后的重复键，并按输入槽位生成条目；
  map 语义不保证顺序。
- 枚举按名称查找已声明变体，并要求单元、单载荷或匿名结构体载荷形态完全匹配。

解析递归 Schema 引用时仍使用名义类型 ID；有限 Config 实例按普通值树遍历。空数组、空 map、`none`
和终止枚举变体都能结束递归值。每个实际子值仍消耗深度、节点、执行和工作区额度；解析失败时
不会返回部分 `ConfigValue`。

结构体帧为每个 Schema 字段记录未见、解析中、已完成、失败或延后状态。对象闭合后按字段声明顺序
生成 `FieldValue`：优先使用显式值，否则校验 Schema 默认值，再使用无默认值可选字段的 `none`，
最后报告必填字段缺失。显式 `none` 会覆盖可选字段默认值。

### 3.3 值来源

每个 `ConfigValue` 都记录来源类别：`ConfigLiteral`、`SchemaDefault`、`EnvironmentVariable`、
`ExplicitFallback` 或 `RustValue`，供诊断和后续转换识别值的来源。

成功解析环境表达式后，还会保存结构化引用名称及已校验、带类型的可选标量 fallback。
提供器返回的实际值不会存入这份元数据；option 节点与其有值标量子节点共享引用记录。

复制和 rebase 会保留这种共享，并将引用文本计入值额度。外来值规范化无法校验环境引用时会报错，
不会悄悄将其来源改为 `RustValue`。

## 4. 环境变量解析

解析器只在值位置遇到 `env` 调用时读取环境。该调用必须位于受 `@env` 授权的标量字段或标量可选字段，
且授权名称必须与表达式名称完全一致。

调用提供器前会先检查名称语法和授权。因此未授权名称、错误位置或无效 fallback 都不会触发宿主读取。

Schema 编译期为授权名称分配规范化槽位，同名授权共享槽位。每次配置调用中，`EnvironmentCache`
只为实际读取的名称建立稀疏条目；条目按 Schema 槽位排序，并通过二分查找。

缓存条目保存未设置状态、复制后的原始字节或提供器错误。缺失值也占一个条目；环境项额度统计实际
读取的不同授权名称，环境字节额度统计缓存的原始字节。

首次读取前，缓存会检查名称长度与授权槽位，再检查条目额度、预留容量并计量插入工作。额度或容量
不足时不会调用提供器。

提供器返回后，缓存检查字符串长度和累计环境字节额度，再将字节复制到计入工作区的存储中。
只有完整复制成功后才提交条目；查找和移动条目计入步数，条目及字节容量计入共享工作区。

缓存不保存字段转换结果。使用同一名称的每个字段仍按自身目标类型转换，并单独执行约束。

提供器返回 `None` 表示未设置，`Some(&[])` 表示已设置为空值。字节必须通过 UTF-8 校验；字符串保留
原文，布尔值只接受 `true` 或 `false`，数值遵循 Config 数值语法和目标范围。

提供器报错、UTF-8 无效，或已设置值转换/校验失败都会产生错误，不会再使用 fallback。

即使环境变量已设置，显式 fallback 也会先按字段标量类型解析并校验。变量缺失时采用该 fallback，
值来源为 `ExplicitFallback`，结果不保留环境引用。

没有显式 fallback 时，缺失变量会延后处理，随后按对象闭合规则应用默认值、可选值或必填字段规则。
提供器值成功转换并通过校验后，才会保留表达式名称和 fallback 以供 typed 往返。

Schema-aware writer 将保留引用写成 `env("NAME"[, fallback])`，并将已选用的 fallback 写为标量字面值。
写出首字节前，它会检查引用名、标量/option 形态、字段精确授权和 typed fallback。

`ConfigDocument` 编辑共用 writer 的标量和 token 规则；可选 Pretty 布局只使用编辑片段的空白规则。
候选文本重新解析和校验后才提交，因此无效编辑不会改变保留文档；formatter 会保留未编辑的 token 与注释。

`typol_config::Environment` 不依赖进程环境。可选的 `typol_std::ProcessEnvironment` 按名称查询，
不会枚举或修改环境。Unix 原始字节由 Config 检查 UTF-8；Windows 转为 UTF-8 并拒绝孤立代理项。

## 5. 校验流水线

### 5.1 字段和局部值

值构造完成后，校验器先确认运行期形态符合 Schema 类型，再从编译 Schema 视图收集使用点规则和
可选包装继承的规则。

`ScalarValidator` 使用 `SchemaRules` 和预编译正则程序检查标量。运行期正则使用本次调用的
`RegexExecutor` scratch 与执行额度，不会重新编译模式。

可选值的规则作用于有值的标量或容器内容；`none` 没有可供标量比较的叶值。`@variants` 会把有值枚举
解析为已编译的 `VariantId` 并检查 allow/deny 列表；`none` 满足合法的 option 限制。

嵌套值在各自目标 Schema 类型上逐层校验，因此数组元素、map value 和枚举载荷的规则只作用于对应节点。
默认值和 fallback 进入最终树前也会通过相同的形态与约束检查。

已校验 fallback 携带本次调用内的验证凭证。写入实例时会登记定位节点，但不重复运行规则。

### 5.2 集合约束

数组和 map 在容器完成时进入 `CollectionValidator`。大小规则检查元素或条目数量；数组还可检查求和、
唯一性、按字段路径唯一及 `@all` 表达式。

`@all` 按源顺序只遍历容器一次，并直接将当前数组项或 map 键值绑定到表达式。错误定位到对应元素或
map 项；运行时直接使用 Schema 编译期生成的表达式 IR 和正则程序。

### 5.3 完整树断言

只有根对象解析成功且后续输入到达 EOF 后，解析器才遍历完整实例树并执行结构体及当前枚举变体断言。
`InstanceWalk` 使用显式帧；表达式中的 `self` 是当前实例，`root` 是根对象。

匿名结构体变体载荷的 `self` 绑定到载荷字段，未选中的变体断言不会执行。

结构化解析、局部校验或 EOF 检查失败时，不执行根断言，也不返回部分 `ConfigValue`。
`ConfigValueProcessor::validate` 规范化后使用相同的完整树校验入口，因此外来值和 Rust 值不能绕过
Schema 规则。

### 5.4 外来值的结果类型

`normalize` 和 `normalize_with_budget` 返回 `NormalizedConfigValue`，表示值形态符合 Schema 且默认值
已补齐，但不代表约束或完整树断言已通过。

`ConfigValueProcessor::validate`、`validate_with_budget`、`ConfigParser::parse` 和
`parse_with_budget` 返回 `ValidatedConfigValue`，表示规范化、约束校验和完整树断言均已完成。

两种包装类型都私有持有 `ConfigValue`，并提供只读的 `as_config_value()` 和消费型
`into_config_value()`。`ValidatedConfigValue` 还保存用于校验的精确编译 Schema 的不透明身份；
`belongs_to_schema()` 可检查这一关联。

## 6. 错误恢复与诊断

结构体字段、数组元素或 map 项解析失败时，解析器可在当前深度寻找下一个逗号或匹配的容器结束符，
然后继续收集独立的同级错误。恢复过程会跟踪圆括号、方括号和花括号深度。

遇到不匹配结束符、EOF、分隔符状态不可靠、致命词法事件或额度终止时，恢复会停止。失败字段仍占据
原槽位，避免重复输入被当作新字段，也避免同一字段再次报告缺失。

`ConfigParseErrors` 是阶段为 `Input` 或 `Parsing` 的非空拥有型集合。它不保存源码或环境原始字节；
普通诊断保留结构化错误码、源跨度、可选 Schema 标签、值来源和安全处理后的实例路径。

输入丢弃后，`span_location()` 仍可查询错误自有的位置快照；`excerpts()` 可查询有界安全源码片段。
扫描时记录配置源位置；Schema 标签位置和安全规则片段则从 `CompiledSchema` 复制。

紧急终止错误可能没有行列或源码片段。错误集合冻结前，普通诊断按主跨度的源 ID、起始字节和结束字节
排序；跨度相同或缺失时保留产生顺序。终止诊断（包括上限槽）始终排在最后。

排序会消耗共享执行额度；额度不足按资源错误处理。

`ConfigParseErrors` 和 `ConfigValueErrors` 的 `Display` 写出诊断报告，包括符号化错误码、原因、位置、
标签及可用片段。收集被截断时，报告会说明后续错误未收集。两种类型都提供 `is_truncated()`；
`Debug` 显示阶段、数量和截断状态。

写入 `fmt::Formatter` 不会分配报告缓冲区；`to_string()` 仍会为返回的 `String` 分配内存。

每条诊断还公开稳定的 `kind().id()`、诊断阶段、语义化 `related_locations()`，以及主跨度、实例路径和
值来源的状态。Facade 的 `TypolError` 在 `std` 与 `no_std` 构建中都提供索引查询
`diagnostic_count()`、`diagnostic(index)` 和 `diagnostic_context(index)`。

状态用于区分不适用、不可用和脱敏字段。`Diagnostic::message()` 以可失败结果返回已翻译原因；翻译失败
会向上传递，不会替换成 fallback 文本。报告 writer 不能再切换原因语言。

诊断按调用方上限收集。上限为 0 时按容量 1 处理；容量为 1 时保留首条真实诊断并标记截断，
不会用 `DIAGNOSTIC_LIMIT_REACHED` 替换它。

容量至少为 2 时会预留末槽；后续错误到达后在该槽加入 `DIAGNOSTIC_LIMIT_REACHED`。应通过
`is_truncated()` 判断是否提前停止；容量为 1 时，列表末尾不一定是上限错误码。

资源超限、分配/容量失败、输入失败和内部不变量错误会终止解析；词法错误也会停止后续恢复。
公开类别包括词法、Config 语法/值/环境/校验、资源、分配、输入、诊断上限和内部错误。
稳定错误码定义于 `typol_core::DiagnosticCode`。

## 7. 资源额度和分配边界

一次 Config 调用分别配置源码片段、Regex、Config、词法流和通用执行额度。解析与校验选项分别公开
`ExcerptLimits` 和 `RegexOptions`；复用通用 `ExecutionBudget` 不会跳过 Regex 计量。

| 额度 | 控制范围 |
| --- | --- |
| `ExcerptLimits` | 源码片段总字节、单行字节、记录数量和上下文行数 |
| `RegexOptions::limits()` | 正则结构边界：模式字节/数量、嵌套、指令、字符区间和 scratch 状态 |
| `RegexOptions::execution()` | 正则步数、单次搜索 NFA 状态数和正则工作区；各默认值为 67,108,864 |
| `ConfigLimits::max_source_bytes` | 单个配置源允许读取的最大字节数 |
| `max_string_bytes` | 解码字符串、字段/变体名、map 键和环境文本 |
| `max_value_nodes` | 解析或外来值规范化期间的值节点数量 |
| `max_nesting_depth` | 值、恢复、规范化、实例遍历和表达式深度；根为 0，等于上限时允许 |
| `max_diagnostics` | 普通及终止 Config 诊断总数 |
| `max_environment_entries` | 本次调用缓存的不同授权环境名称数 |
| `max_environment_bytes` | 本次调用缓存的环境原始字节总数 |
| `max_constructed_text_bytes` | 本次调用构造或复制的文本总量 |
| Formatter 输出 | 输出字节数，也受源码和构造文本额度限制 |
| Formatter 工作区 | 帧额度；实际分配也计入共享工作区 |
| Formatter 嵌套 | 最大分隔符深度 |
| `max_display_name_bytes` | 诊断显示名称长度 |
| `StreamLimits::chunk_bytes` | 流式窗口大小；`try_new` 会拒绝零值 |
| `StreamLimits::max_token_bytes` | 单个 token 的源文本长度 |
| `ExecutionLimits` | 一次调用中非 Regex 工作的累计步数和工作区字节 |

当前 `ConfigLimits::default()` 包括：单源 1 MiB、100 条诊断、深度 128、字符串 256 KiB、值节点
100,000 个、环境项 1,024 个、环境字节 8 MiB、构造文本 16 MiB、显示名称 4,096 字节。
默认流限制为 8,192 字节窗口和 1 MiB token。

源码片段、正则结构、正则执行和通用执行的默认值分别来自 `ExcerptLimits`、`RegexLimits`、
`RegexExecutionLimits` 和 `ConfigParseOptions::default()`。这些是可覆盖的运行策略，不属于 DSL 语义。

`ConfigLimits` 的九项额度都有具名更新方法：`with_max_source_bytes`、
`with_max_diagnostics`、`with_max_nesting_depth`、`with_max_string_bytes`、
`with_max_value_nodes`、`with_max_environment_entries`、`with_max_environment_bytes`、
`with_max_constructed_text_bytes` 和 `with_max_display_name_bytes`。

未覆盖的额度保留默认值。额度为零时，仍按该项自身语义处理。

默认源码片段额度为总计 65,536 字节、单行 512 字节、128 条记录，以及每个跨度额外两行上下文。
可在解析或校验选项中设置；必要容量为零会关闭源码片段，但不会关闭诊断。

候选窗口、遮罩范围和最终片段各有字节上限，并计入通用会话工作区。额度耗尽或可选分配失败只会降低
源码片段的可用状态。

值深度按父子边计算：根为 0，有值的 option 载荷和单值枚举载荷各增加一层，等于上限的深度允许通过。
匿名结构体枚举载荷
在载荷与字段之间再计一层，因此枚举深度为 `d` 时，其字段从 `d + 2` 开始。

空容器在自身深度检查。恢复扫描会以根对象的分隔符基准换算栈限制，不会把绝对分隔符数量直接当作值深度。

直接使用 `typol-core` 并通过 `ConfigValueBuilder::append_view` 复制 `ValueView` 时，必须提供
`ValueCopyLimits`。根值深度为 0，等于上限的值允许复制；复制使用显式后序帧栈和可失败分配。

若 builder 带共享执行额度，帧和子项存储也计入该额度。单次复制深度额度独立于 `ExecutionLimits`；
后者仍限制累计步数和工作区字节。

动态工作区使用可失败分配并计入执行额度。额度耗尽、工作区不足和分配失败是不同的终止错误。

解析、规范化和值视图复制都使用显式帧，深度限制不依赖宿主调用栈。失败后临时值、缓存和工作区会释放，
不会发布部分结果。

## 8. 秘密值与敏感数据

`SecretPolicy` 根据编译 Schema、实例路径、字段、变体和别名查找最外层秘密边界。诊断进入缓冲区前，
敏感消息会被隐藏，路径会被裁剪；最终错误快照阶段还会再次应用该策略。

秘密子树下的数组下标、map 键和载荷后缀不会暴露。若动态路径无法安全映射回 Schema，会以不带路径的
内部诊断终止，避免泄漏未知 map 键。

同一策略也会在 `TypedSchema::serialize` 和 `TypedSchema::deserialize` 的 Rust 转换诊断进入
`CodecErrors` 前保护它们。无法映射的路径会清除；脱敏预算或分配失败时只返回不含原路径的安全诊断。

源码行进入错误对象前，已知秘密范围会逐字节遮罩；秘密 map 项的键和值都会隐藏。未完成的秘密值会一直
遮罩到输入边界。

若范围记录失败、跨度无效或片段额度耗尽，受影响的片段会被省略，不会回退显示原始行。环境原值不会进入
源码窗口；默认报告使用裁剪后的显示名称，不附加真实文件路径或 OS 错误文本。

通用 `ConfigValue` 的 `Debug` 只显示值种类、字符串/容器长度和来源，不显示字面值。请求、错误集合及外层
值封装也不会打印输入或错误正文。

秘密标记保护诊断和安全展示，但不会加密成功 `ConfigValue` 中的数据。序列化和应用代码仍可读取真实值。

环境原始字节只由本次调用的缓存暂存；构造字符串或标量后，不会附加到诊断。多次读取的一致性取决于宿主
提供器；进程环境适配器不保证不同名称来自同一快照。

## 9. 宿主集成

`SchemaLoader` 持有编译后的 Schema 和源名称。文件配置使用 `typol_std::FileConfigInput`，由 lexer
按需读取；标准 I/O reader 通过 `ReadSource` 将 `std::io::Read` 适配为 `typol_core::Read`。

这些 adapter 处理宿主 I/O 和有限错误分类；Config 语法及 Schema 校验由 `typol-config` 实现。

启用 `config-ir` feature 后，Rust 调用方可用 `ConfigIr::serialize` 导出 Config IR v1，详见
[Config IR 规范](../config-ir/)。原始 `ConfigValue` 在编码前会补齐默认值并执行完整 Schema 校验。

已持有 `ValidatedConfigValue` 的调用方可用 `ConfigIr::serialize_validated` 跳过第二次校验遍历，
但凭据必须属于相同的编译 Schema。Facade 负责类型和值遍历、秘密标记、确定性排序、资源额度和错误类别。

输出超限与排序工作区超限使用不同类别。最大嵌套深度为 128，可由 `ConfigIrOptions` 降低而不能提高。
`typol-cli` 将配置加载时获得的验证凭据传给快速入口，并把完整结果写入 stdout。

CLI 的 `config fmt` 则读取 `ConfigDocument`、格式化保留源码并原子替换同一文件。`--check` 只比较格式化
结果，不写文件；格式化不会将环境结果或补齐的默认值写回源码。

命令用法见 [CLI 使用指南](../../guides/cli/)。Config IR 的 `secret: true` 仍与真实值一同输出，
调用方必须按敏感数据处理。

`TypedSchema::deserialize` 复用配置解析和完整校验，再将拥有值转换为 Rust 类型。`typol` 编解码器及
上下文 Serde 会在编码或解码前，通过 `ConfigValueProcessor::validate_with_budget` 校验外来或序列化值。

绑定、解析和转换可共用外层通用执行额度；每次校验仍根据自身选项创建 Regex 额度。没有 Schema 上下文的
普通 Serde trait 调用不在此校验保证范围内。

## 10. 验收与维护

配置测试位于 `crates/typol-config/src/parser/tests/`、`src/value_validation/tests.rs` 和
`crates/typol-config/tests/`。维护解析器时应覆盖流式输入、词法/语法错误、嵌套恢复、默认补齐和枚举载荷。

还应覆盖环境缓存、fallback 与转换；标量和集合规则；完整树断言；秘密路径裁剪；位置快照；执行/工作区额度；
以及分配失败时的原子性。

Formatter 测试覆盖 token/注释保留、分隔符、`env` 和枚举表达式、LF/CRLF、尾随注释、幂等性、输出/工作区
额度及失败时不返回半成品。CLI 测试覆盖原子替换、重复运行幂等、只读检查、本地化帮助、用法错误和格式化失败。

`typol` 与 `typol-std` 的测试还覆盖文件、reader、进程环境、类型绑定和安全诊断集成。

若实现行为与 Config DSL 或 Schema DSL 冲突，应先修订相应语义规范或决策记录。解析器和宿主 adapter
不得自行确立新的 DSL 语义。

## 11. 有界诊断源码快照

`ExcerptLimits` 独立限制总存储字节、单行字节、记录数量和每个跨度的上下文行数。额度为零会关闭片段；
每个跨度从起始行选取有界窗口，跨行跨度超出窗口时标为截断。

上下文行数在跨度前后分配。候选窗口、脱敏缓冲和最终片段各有字节上限，并计入通用调用工作区；
可选分配失败不会改变业务诊断。

拥有型 `SourceExcerptSet` 的状态为 `Complete`、`Truncated` 或 `Unavailable`。缺失记录不会回退读取或展示
原文。列数按 Unicode 标量计数，Tab 占一列，CRLF 不计入行内容。

裁剪只落在 UTF-8 边界并添加省略标记。流式 Config 输入维护有界候选和近期行窗口，并将诊断引用的行固定到
调用结束；延迟诊断只能使用仍保留的候选。

解析完成时，只发布实际诊断及其上下文引用的行。已知秘密值和秘密 map 源范围会被遮罩；未完成的秘密值会
一直遮罩到输入末尾。恢复跳过或尚未绑定的内容会保守隐藏。

范围记录失败时，源码片段不可用。环境原值不会进入行窗口。只有源目录仍存活时才提取 Schema 片段；默认值
范围都会遮罩，包括尚未解析秘密别名的默认值。

若缺少完整 AST，就省略 Schema 源码片段。错误和编译产物不保留完整源码；新增类型的 `Debug` 只显示状态和数量。
