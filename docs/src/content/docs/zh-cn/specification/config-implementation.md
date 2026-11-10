---
title: "Config 实现说明"
description: "说明 Typol 如何流式解析配置、应用校验并生成安全诊断。"
sidebar:
  order: 3
---

## 本页要点

:::tip[适用场景]
追踪配置读取经过的解析器、环境提供器、校验流程或资源限制时，请查阅本页。
:::

| 环节 | 当前行为 |
| --- | --- |
| 解析 | 值读取增量消费 token，不保留 AST；文档读取还会在额度内保留源文本和解析器生成的值跨度。 |
| 环境变量 | 只有遇到已授权的 `env(...)` 表达式时，才调用调用方传入的提供器读取环境变量。 |
| 格式化 | `ConfigFormatter` 使用显式分隔符栈扫描已校验文档，并保留源 token 与注释。 |
| 限制与错误 | 解析、校验、诊断和执行都有显式资源额度，并通过结构化错误报告失败。 |

本文说明 Typol 当前如何实现 `.tconf` 配置：解析入口、流式词法输入、Schema 指导下的值构造、环境变量读取、约束校验、错误恢复和安全诊断。本文补充[Config DSL 规范](../config-dsl/)及[Schema 实现说明](../schema-implementation/)，不改变 DSL 的词法、语法和语义。

本文依据当前源码整理。配置语义以 Config DSL 和 Schema DSL 规范为准；资源默认值以对应源码为准。

## 1. 实现范围与边界

配置处理由以下组件协作完成：

1. `typol-syntax` 从 `Read` 按需读取 UTF-8 字节，产生带源跨度的 token。它不为配置创建完整 token 数组。
2. `typol-config` 持有 `CompiledSchema` 的只读视图，按目标类型读取 token，并直接构造拥有型 `ConfigValue`。文档入口还会保留原始文本和同一次解析产生的 `LocationIndex` 跨度，不会为了定位值而重新词法扫描原文。
3. `typol-semantic` 提供类型、字段、变体、约束、断言表达式、环境授权和正则程序的只读视图及表达式执行器。
4. `typol` 将配置解析与 Rust 类型转换、serde、加载会话和可选 Config IR 导出组合；`typol-std` 提供文件输入和进程环境适配器。

核心配置实现使用 `no_std + alloc`。核心解析器不打开文件、不读取进程环境，也不向终端输出。环境访问只通过调用方传入的 `Environment` 完成。成功结果拥有自己的字符串、键和值，不借用配置输入或环境提供者缓冲区。

在 `no_std + alloc` 环境中通过 facade 集成应用的步骤见 [no_std 指南](../../guides/no_std/)。

主要源码位置：

| 职责 | 实现位置 |
| --- | --- |
| 公共入口、请求、配置额度和会话 | `crates/typol-config/src/lib.rs`、`crates/typol-config/src/options.rs`、`crates/typol-config/src/session.rs` |
| Schema 指导的解析状态机 | `crates/typol-config/src/parser.rs`、`crates/typol-config/src/parser/frames.rs`、`crates/typol-config/src/parser/containers.rs`、`crates/typol-config/src/parser/enums.rs`、`crates/typol-config/src/parser/literals.rs` |
| token 错误恢复和 map 重复键索引 | `crates/typol-config/src/parser/recovery.rs`、`crates/typol-config/src/parser/map_index.rs` |
| 环境授权、缓存和标量转换 | `crates/typol-config/src/environment.rs`、`crates/typol-config/src/scalar.rs` |
| 默认值复制、外来值规范化和实例定位 | `crates/typol-config/src/value_normalization.rs`、`crates/typol-config/src/instance.rs` |
| 标量、集合、断言和完整树校验 | `crates/typol-config/src/validation.rs`、`crates/typol-config/src/validation/`、`crates/typol-config/src/value_validation.rs` |
| 诊断、路径裁剪和秘密值隐藏 | `crates/typol-config/src/diagnostics.rs`、`crates/typol-config/src/secret.rs` |
| 源格式化与词法注释扫描 | `crates/typol-config/src/document.rs`、`document/formatter.rs`、`document/formatter/format_scan.rs` |
| token 流和 UTF-8 输入 | `crates/typol-syntax/src/input.rs`、`crates/typol-syntax/src/lexer.rs`、`crates/typol-syntax/src/cursor.rs` |
| 值来源和共享执行额度 | `crates/typol-core/src/value.rs`、`crates/typol-core/src/budget.rs` |
| 共享资源策略和源码快照额度 | `crates/typol-policy/src/lib.rs`、`crates/typol-core/src/excerpt_limits.rs` |
| 文件/进程环境适配及加载 facade | `crates/typol-std/src/filesystem/input.rs`、`crates/typol-std/src/environment.rs`、`crates/typol/src/loading.rs`、`crates/typol/src/binding.rs`、`crates/typol/src/codec.rs` |
| Config IR v1 序列化及额度 | `crates/typol/src/config_ir.rs`（由 `config-ir` feature 启用） |
| 命令行装配、错误展示和标准输出 | `crates/typol-cli/src/main.rs`、`crates/typol-cli/src/commands.rs` |

## 2. 调用入口与会话

### 2.1 配置文本解析

通用调用方使用：

```rust
ConfigParser::new(schema).parse(request, environment)
```

解析成功返回 `ValidatedConfigValue`，明确表示值约束和完整树断言均已通过。调用方可只读借用其中的 `ConfigValue`；只有需要不带验证标记的拥有值时才调用 `into_config_value()`。

`ConfigParseRequest` 包含可读 `Read`、显示名称和 `ConfigParseOptions`。选项分别持有 `ExcerptLimits` 和 `RegexOptions`，并配置 `ConfigLimits`、通用执行/工作区 `ExecutionLimits` 和词法流 `StreamLimits`。Schema 与 Config 共享相同的块字符串 lexer 规则。由于 `Read` 不保证支持回放，lexer 会将当前块 token 正文暂存到计费工作区，再去缩进并解码到第二个计费缓冲区。扫描消耗共享执行额度；源 token 上限包括分隔符、闭合缩进及原始 CRLF 字节，字符串上限只计算最终解码后的 UTF-8 字节。显示名称仅用于诊断元数据，不是配置文本的一部分。

`ConfigParser::parse_document` 和 `LoadedSchema::read_document` 会保留 parser 的 UTF-8 源缓冲区及语义定位索引。文档直接接管该缓冲区作为源文本，不再复制第二份完整内容。首次编辑前，源文本本身也用于 `DocumentSnapshot` 写入期望值；首次编辑成功时，旧源缓冲区会直接转为计费快照，不再分配第二份文本。写入成功后，期望值更新为当前源文本，并释放独立快照。

`ConfigDocument::write_to` 原样输出保留的文本。`apply_edits` 通过 parser 索引解析带类型的替换、字段/map 项/数组元素插入和删除操作。事务按父容器分组子节点索引，并为每个受影响容器只准备一次边界、条目位置、逗号和布局。它拒绝重叠源范围，组装一个候选文档，再重解析并校验后提交内存文档。重解析只记录位置，不会再保留一份候选源码；校验成功后，候选缓冲区会直接成为新文档保留的源码。替换会编码目标子树；插入会推断容器换行、缩进和键分隔空格。插入值直接编码到最终补丁缓冲区；map 项存在性检查使用已解析的父节点，不克隆完整路径。删除和复合值替换会提取受影响范围内的注释，并把注释字节保留在替换区域。未触及的源范围逐字节不变。编辑数量、排序、索引遍历和有界源扫描均计入共享执行额度。相关修改应通过一次 `apply_edits` 或 `replace_values` 批量提交，这样候选文档只会解析和校验一次；分开的 `set_value` 调用会各自付出一次成本。编辑器扫描受限的源切片定位分隔符和注释，并把两种字符串形式都当作不透明范围，因此块字符串中的括号、逗号和注释标记不会干扰容器配对；它不另建 token/trivia CST。

`set_value` 使用 `ConfigReplacement` 封装路径和值，`insert_field` 使用 `ConfigFieldInsertion` 封装路径和值；两者都通过构造函数创建，再把 options 传给文档方法，以满足仓库的方法参数上限。默认布局为 `Compact`；`with_layout(TextLayout::Pretty)` 只格式化新写入的值片段。`remove` 保留原签名，并在内部使用默认选项。一批编辑中的所有值共用一份布局。Pretty 保持 `ConfigValue` 的 entry 顺序，不规范化外围文档：非空 array、map 和 structure 每项单独一行，逐层增加四个空格并使用尾逗号；空容器保持内联。枚举单值载荷使用多行圆括号且不加尾逗号；unit variant 和 `env(...)` 保持紧凑。新行沿用 formatter 的整文档规则（源码任意位置含 CRLF 时选 CRLF，否则选 LF），值片段本身不附加文件级末尾换行。替换缩进以目标行的前导空格或 Tab 为基线，再为该行此前仍打开的实际编码容器增加四个空格；新插入行使用容器布局选定的缩进。Compact 编码和现有外围插入标点保持不变。`ConfigEditOptions` 从 `typol_config` 根导出；`TextLayout` 可从 `typol_config`、`typol`、`typol::core` 和 `typol::config` 使用。

候选源码同时受 `max_source_bytes` 与 `max_constructed_text_bytes` 限制。编辑器在分配候选缓冲区前，以候选完整长度向 constructed-text 额度计费一次；补丁片段不重复计费，校验期间 parser 新建的文本仍按现有路径计费。候选解析若报告 allocation 或 resource/capacity 诊断，编辑错误类型会映射到对应类别，并继续保留完整 `ConfigParseErrors`。

`ConfigFormatter::format(&document, options, out)` 只格式化成功解析得到的 `ConfigDocument`。它使用显式分隔符栈直接扫描保留的源文，不构造配置 AST，也不重新编码 `ConfigValue`。`FormattedConfig` 独立拥有完整结果，提供只读字符串视图，并可通过 `typol_core::Write` sink 消费写出。`ConfigFormatter::is_formatted` 使用相同格式化流程，将每段输出直接与原文比较，不分配完整输出缓冲区；CLI 的 `config fmt --check` 使用此路径。原文档及其源位置保持不变。格式化按顺序保留所有非空白源 token 和注释，包括字符串/数值写法和 `env(...)` 表达式。每个块字符串都是不透明 token，内部字节、缩进和 LF/CRLF 写法均保持原样；`format` 与 `is_formatted` 使用相同的边界扫描器。非空容器按每行一个条目展开，使用 4 空格缩进并添加尾逗号；空容器保持紧凑，除非其中含有注释。枚举单值载荷使用多行圆括号分组且不加尾逗号，`env(...)` 调用仍保持紧凑。输入采用 LF 或 CRLF 时保留对应风格，输出以相应换行符结束，并满足幂等性。

`ConfigFormatOptions` 只提供输出字节、格式化工作区和嵌套深度额度，不提供会产生多种规范格式的样式选项。格式化还受文档的源码和构造文本额度约束；栈存储和扫描工作计入文档共享执行预算。`format` 会构造拥有型计费输出缓冲区，并计量构造文本；`is_formatted` 使用相同的输出额度逐段比较，不构造或保留该缓冲区。尾部注释的逗号前瞻每串连续注释只执行一次，因此含 `k` 条注释的串只增加线性扫描工作。拥有型结果只有在扫描、范围检查和输出计量全部成功后才返回。结构错误、资源超限、分配失败和后续输出 sink 错误保持不同类别；格式化中断时不会把部分缓冲区作为成功结果返回。

`LoadedSchema::read_file` 返回 `ConfigDocument`；只需要原有已校验值结果时使用 `read_file_value`。启用 `std` 后，`LoadedSchema::write_file` 使用文档快照期望值，并同时限制源字节和文件输入字节。普通文件系统不能把按字节比较与路径替换作为一个原子操作，因此本地 adapter 对 `ExactSnapshot` 返回 `AtomicReplaceUnsupported`。新目标使用 `write_file_with_expectation` 和 `MustBeAbsent`；有意覆盖时显式选择 `ReplaceAny`。Unix 通过同目录硬链接原子创建新目标；Windows 使用同卷且不允许覆盖的写穿透移动，两者都不会覆盖并发创建的目标。`ReplaceAny` 使用原子替换；Unix 在提交后同步父目录，若无法确认持久性则返回专门错误。Windows 使用同卷写穿透移动。no_std 调用方为自己的目标实现 `SourceWriter`，并负责提供所选期望值要求的原子语义。

`ConfigValueProcessor::validate` 与 `ConfigValueProcessor::normalize` 接收 `ConfigValidationRequest`，其 `ConfigValidationOptions` 分别配置 `ExcerptLimits`、`RegexOptions`、`ConfigLimits` 和通用 `ExecutionLimits`，不包含词法流选项。校验与规范化不会读取配置输入或环境提供器；两者的额度与文本解析分开配置。

no_std 与 std 构建共用 `TypolOptions` 和 `SchemaLoader::new(TypolOptions)`。`LoadedSchema::read_value` 和 `BoundSchema` 的读取方法接收泛型 `ReadRequest<R>`；reader 实现 lexer 使用的同一 `typol_core::Read` 契约。内存文本可用 `ReadRequest::from_text`，no_std 调用方也可传入 `SliceSource` 或自定义 reader。所有读取都显式接收 `Environment` 提供器。启用 `std` 后，`read_file` 通过 `FileConfigInput` 打开配置文件，`read_io` 通过 `ReadSource` 适配标准 I/O reader；`SchemaLoader::compile_file` 会将文件 Schema 接入同一编译流程。需要访问进程环境时，请创建带所需 `ProcessEnvironmentLimits` 的 `ProcessEnvironment`，再传给读取方法。

启用 `std` 后，同一个 `TypolOptions` 也携带宿主限制。配置文件输入额度通过 `with_file_input_limits` 设置；文件 Schema、来源名称及名称编辑额度分别使用 `with_files`、`with_names` 和 `with_name_edits`。配置解析与绑定额度分别通过 `with_config_limits` 和 `with_binding_limits` 调整。配置策略类型可从 `typol::config` 获取，宿主适配类型可从 `typol::host` 获取。`FileConfigInputLimits` 只控制配置文件与路径字节数，`ProcessEnvironmentLimits` 由 `ProcessEnvironment` 持有，用于限制环境名称和值字节数；Schema 导入额度由 `FileSchemaLimits` 管理，执行额度仍由 Schema/Config 选项管理。`ConfigLimits` 以 `ConfigLimits::default()` 提供有限默认额度，并为每项额度提供唯一的具名链式 `with_max_*` 更新方法；它不公开位置参数构造器或可变 setter，字段保持私有，外部通过只读 getter 查询。配置可传给 `ConfigParseOptions::with_config`。编解码选项也提供具名更新方法。

`ConfigParser::parse` 为本次调用建立通用 `ExecutionBudget`。需要和外层加载、绑定或编解码流程共用非正则执行余额时，调用 `ConfigParser::parse_with_budget`；传入余额的 limits 必须与请求选项中的通用执行 limits 完全相同，否则返回内部配置错误。一次调用内的解析、环境处理、值校验和诊断路径处理共用该通用余额；所有正则检查则共用从 `ConfigParseOptions::regex()` 创建的独立 `RegexExecutionBudget`。

### 2.2 外来值校验

`ConfigValueProcessor::validate` 接收拥有型 `ConfigValue`，复制并按 Schema 规范化后执行与文本路径相同的完整约束和断言校验。它不接收 `Environment`，因此不读取环境变量。需要沿用通用调用余额时使用 `ConfigValueProcessor::validate_with_budget`；正则检查仍使用 `ConfigValidationOptions::regex()` 创建的独立预算。仅规范化的操作不会创建或消耗正则预算。

值规范化会检查每层值形态、字段名、枚举载荷和目标标量；按 Schema 字段顺序重建结构体，补齐 Schema 默认值和无默认值的可选字段，并为输出创建新的拥有树。缺少必填字段、未知或重复字段、无效 map 键/值及错误载荷形态均失败。map 不具有 DSL 层面的顺序语义；外来值路径为了稳定重建会按键排序。

仅规范化的操作会拒绝携带环境来源的值，因为它不会运行这些引用所需的完整授权和 fallback 校验。外来值需要保留环境来源时，请使用 `ConfigValueProcessor::validate`。

### 2.3 根据 Schema 默认值生成配置

`LoadedSchema::default_config_to_string` 和 `LoadedSchema::write_default_config` 会根据编译后的入口 Schema 生成 `.tconf`，无需绑定 Rust 类型或接收 `Environment`。Facade 创建空结构体根值，并只调用一次 `ConfigValueProcessor::validate_with_budget`，因此默认补齐、字段和集合约束、完整树断言都沿用其他外来值的验证流水线。生成错误通过 `TypolError` 返回，阶段为 `Encoding`；值诊断仍可通过 `codec_errors().config_value_errors()` 检查。

对每个缺失字段，共用的补齐规则依次选择显式 Schema 默认值、无默认值 `option<T>` 的 `none`，或带字段路径和 Schema 定义位置的必填缺失诊断。只有入口结构体会被合成为根值。不会仅因嵌套结构、数组、map 或枚举的内容看似可构造就自动生成它们；字段自身需要显式默认值。选定嵌套对象或枚举载荷后，再递归补齐其字段。`@env` 只授权输入文本中的环境表达式；生成过程不会执行 `env(...)` 或查询环境提供器。

`DefaultConfigOptions` 默认选择 `TextLayout::Compact`、1 MiB UTF-8 字节输出上限，并允许输出真实 `@secret` 值。`with_layout(TextLayout::Pretty)` 使用与 `config fmt` 相同的四空格规范布局：复合容器和枚举单值载荷分组为多行，输出以 LF 结尾。`with_pretty(bool)` 和 `pretty()` 已移除，这是公共 API 破坏性更改；调用方应迁移到 `with_layout(TextLayout)` 和 `layout()`。也可拒绝包含秘密标记值的结果；拒绝检查会在 sink 写入首字节前完成，并返回不含值文本的稳定编码诊断。允许输出时，秘密仍以明文形式存在，必须按敏感配置保护。字符串 API 只有在完整校验和编码成功后才返回；sink API 在编码、sink I/O、执行预算或输出限额失败时可能已写入前缀。

若要原子创建或替换文件，应先生成完整字符串，再将字节交给 `FileSourceWriter::replace_atomically`，并显式选择 `SourceExpectation`。`LoadedSchema::write_file` 操作的是 `ConfigDocument` 的源快照，并非默认配置生成的文件 API。

## 3. 流式解析与值构造

### 3.1 输入和根值

解析器为配置源申请新的 `SourceId`，从 `schema.entry()` 取得根类型，并在初始化时确认其为结构体。随后 lexer 通过固定大小窗口从 `Read` 拉取字节；token cursor 仅缓存当前事件和至多一个前瞻事件。行列位置由流式输入维护，token 的源跨度使用 UTF-8 字节偏移。

根值按结构体对象规则解析。对象完整后，解析器继续读取到 EOF；多余 token 会使配置无效。因此单个调用只接受一个根对象，根对象之后允许的内容限于 DSL 认可的空白和注释。解析器在构造值时消费 token，不保留可供调用方读取的配置 AST。

### 3.2 按 Schema 类型解释 token

`ValueParser` 使用受预算约束的显式帧栈处理嵌套值，避免把配置嵌套直接映射为宿主递归调用。每个帧持有当前 Schema 类型及容器/载荷的续接状态：

- 标量根据 Schema 的精确内建类型消费一个字面值；字符串使用 lexer 已解码的内容，整数检查目标位宽，浮点转换后必须有限。
- `option<T>` 接受 `none`，或解析一个 `T` 后包装为有值状态；除这一层包装外不做隐式类型转换。
- 数组按输入顺序构造元素，并在闭合时检查定长数组长度。
- 结构体和匿名结构体载荷通过 Schema 字段索引匹配未加引号的字段名，输入字段可以任意排序。
- map 只接受字符串键。调用期 AVL 索引用解码后的键检测重复项，索引按输入槽位输出 map 项；map 语义不承诺项顺序。
- 枚举按变体名查找 Schema 声明，并要求单元、单载荷或匿名结构体载荷形态完全匹配。

解析递归 Schema 引用时仍使用名义类型 ID；每个有限 Config 实例都按普通值树遍历。空数组、空 map、`none` 和枚举终止变体都可以结束递归值。每个实际子值仍计入现有深度、节点、执行和工作区额度；发生错误时不会返回部分 `ConfigValue`。

结构体帧为 Schema 中的字段建立槽位状态，分别记录未见、正在解析、已完成、失败和延后处理的字段。对象闭合后按 Schema 字段声明顺序生成 `FieldValue`：显式字段值优先；否则复制并校验 Schema 默认值；再否则无默认值的 `option<T>` 取 `none`；其余字段报必填缺失。显式 `none` 是普通显式值，会覆盖可选字段默认值。

### 3.3 值来源

每个 `ConfigValue` 记录来源类别，供诊断和后续转换区分：`ConfigLiteral`、`SchemaDefault`、`EnvironmentVariable`、`ExplicitFallback` 和 `RustValue`。成功的环境解析还会保存结构化引用名称，以及已按目标类型校验的可选标量 fallback；provider 返回的实际值不会进入这份元数据。option 节点与其存在的标量子节点共享同一引用记录；跨值复制和 rebase 会保留这种共享，并将引用文本计入值额度。外来值规范化遇到无法验证的环境引用时会报错，不会静默降级为 `RustValue`。

## 4. 环境变量解析

解析器只在值位置遇到 `env` 调用时尝试环境解析。调用必须位于带 `@env` 授权的标量字段或标量可选字段中；字段授权名称与表达式名称逐字匹配。名称格式和授权校验发生在调用 提供器 之前，未授权名称、错误位置或无效回退不会触发宿主读取。

Schema 编译期为授权名称建立规范化槽位；同名授权共享一个槽位。`EnvironmentCache` 在本次配置调用内按实际读取的名称建立稀疏条目，不按 Schema 授权总数预分配。条目按 Schema 槽位排序并以二分查找；未出现的槽位表示尚未读取，已插入条目保存缺失状态、复制后的原始字节或宿主读取失败。缺失值也占一个缓存项；累计环境项额度按本次实际读取的不同授权名称计，累计环境字节额度按实际缓存的原始字节计。

每次读取先检查名称长度、授权槽位和缓存命中。首次读取会先验证环境项额度、预留一个缓存条目容量并计量插入工作，然后才调用 提供器，因此额度或条目容量不足时不会触发宿主读取。提供器 返回字节后，再检查字符串长度和累计环境字节额度，并复制到共享工作区计费的存储；只有完整复制后才提交缓存条目。缓存查找和条目搬移计入执行步数，缓存条目与字节容量计入共享工作区预算。缓存不存字段转换结果：每个使用字段仍按自己的目标类型转换并单独执行其约束。

提供器 返回 `None` 表示未设置，`Some(&[])` 表示已设置为空值。返回字节需先通过 UTF-8 验证；字符串保留原文，布尔值只接受 `true`/`false`，数值遵循配置数值格式及目标范围。提供器 错误、无效 UTF-8 或已设置值转换/校验失败均产生错误，不会转而使用回退值。

若表达式包含回退字面值，解析器先按字段标量类型解析并校验回退，即使环境最终已设置也必须有效。环境缺失时优先采用显式回退；结果来源是 `ExplicitFallback`，不会携带环境引用。无显式回退时，解析器将字段标记为延后处理，再沿用对象闭合时的默认值、可选值或必填字段规则。只有 provider 值成功转换且通过校验后，才会保留表达式名称和 fallback 供 typed 往返。

Schema-aware writer 会将保留的引用写成 `env("NAME"[, fallback])`，而已选用的 fallback 写成标量字面值。完整值和 Schema 校验会在 writer 写入首字节前检查引用名、标量/option 形状、字段精确授权和 typed fallback。`ConfigDocument` 的替换与插入共享 writer 的 scalar/token 规则；可选 Pretty 布局使用编辑片段的空白规则，不采用完整输出布局。候选文本会重新解析并校验后才提交；候选无效时保留原文档。formatter 仍保留未编辑范围中的原始 token 和注释。

`typol_config::Environment` 不依赖进程环境。`typol_std::ProcessEnvironment` 是可选宿主适配器：按名称查询进程环境，不枚举或修改环境；Unix 字节在配置层校验 UTF-8，Windows 转为 UTF-8 并拒绝孤立代理项。

## 5. 校验流水线

### 5.1 字段和局部值规则

每个值完成构造时，校验器先确认其运行期形态与 Schema 类型相符，再通过编译 Schema 的有效约束视图收集使用点规则和可选包装继承的规则。标量校验由 `ScalarValidator` 执行，使用 `SchemaRules` 和预编译的正则程序；运行期正则使用本次调用的 `RegexExecutor` scratch 与执行额度，不重新编译模式。

可选值的规则作用于其有值的标量/容器内容；`none` 本身没有叶值可供标量比较。嵌套值逐层按目标 Schema 类型检查。默认值和 fallback 在写入最终树前也需通过相同的形态与约束检查；已验证的 fallback 会携带本次调用内的验证凭证，写入实例时登记定位节点但不重复运行规则。

### 5.2 集合约束

数组和 map 在各自容器完成时进入 `CollectionValidator`。大小约束检查元素/项数量；数组还可检查求和、唯一性、按字段路径唯一以及 `@all` 表达式。`@all` 按源顺序只遍历容器一次，并直接将当前数组项或 map 键值绑定到表达式；错误定位到对应元素或 map 项。Schema 静态编译已创建的表达式 IR 和 regex 程序由运行时直接消费。

### 5.3 完整树断言

只有根对象解析成功且后续输入为 EOF 后，解析器才遍历已完成的实例定位树并执行结构体及当前枚举变体断言。`InstanceWalk` 使用显式帧沿父子关系遍历；运行时表达式使用当前实例的 `self`、根对象的 `root`，匿名结构体变体载荷绑定到相应载荷字段。未选中的变体断言不执行。

结构化解析、局部校验或 EOF 检查出现错误时，不运行根断言，也不返回部分 `ConfigValue`。`ConfigValueProcessor::validate` 在规范化完成后使用同一完整树校验入口，因此 Rust/外来值不会绕过 Schema 约束或断言。

### 5.4 外来值处理的结果类型

`ConfigValueProcessor::normalize` 和 `ConfigValueProcessor::normalize_with_budget` 返回 `NormalizedConfigValue`，表示输入树的形态符合 Schema 且默认值已补齐；该类型不表示约束或完整树断言通过。`ConfigValueProcessor::validate`、`ConfigValueProcessor::validate_with_budget`、`ConfigParser::parse` 和 `ConfigParser::parse_with_budget` 返回 `ValidatedConfigValue`，表示输入树已完成规范化、约束校验和完整树断言。两个包装类型都私有持有 `ConfigValue`，调用方可通过 `as_config_value()` 只读访问，或显式调用 `into_config_value()` 消费状态标记。

## 6. 错误恢复与诊断

结构体字段、数组元素和 map 项解析失败时，解析器可在当前容器深度寻找逗号或该容器的匹配结束符，然后继续报告独立的同级错误。恢复游标跟踪括号/方括号/花括号深度；遇到不匹配结束符、EOF、失去可靠分隔符状态、词法 fatal 事件或预算终止时停止恢复。恢复过程中失败字段仍占据相应槽位，避免重复输入被误认为新字段，也避免错误后再报告同一字段的缺失错误。

`ConfigParseErrors` 是非空拥有型错误集合，阶段为 `Input` 或 `Parsing`。错误对象不保存配置源文本或环境原始字节；普通诊断保留结构化错误码、源跨度、可选 Schema 标签、值来源和经安全处理的实例路径。`span_location()` 查询本次返回错误所覆盖的位置快照，输入丢弃后仍可用；`excerpts()` 查询错误拥有的有界安全源码片段。配置源位置在扫描时记录；Schema 标签位置及其安全规则片段通过 `CompiledSchema` 查询后复制为快照。紧急终止错误可能没有行列或源码片段。

普通诊断在错误集合冻结前按主跨度的源 ID、起始字节、结束字节升序排列；主跨度相同或缺失时保留产生顺序。终止诊断（包括诊断上限槽）始终位于列表末尾。排序消耗本次调用的共享执行预算，预算不足按资源错误处理。

`ConfigParseErrors` 和 `ConfigValueErrors` 的 `Display` 写出诊断报告，含符号化错误码、原因、位置、标签和可用片段；收集达到上限时附带“诊断收集已截断，后续错误未收集”说明。两种错误都提供 `is_truncated()` 查询；`Debug` 保留阶段、诊断数量和截断状态。格式化写入 `fmt::Formatter` 时不建立报告缓冲区。`to_string()` 仍会为其返回的 `String` 分配内存。

诊断缓冲区按调用方上限收集错误。上限为 0 时按有效容量 1 处理；容量为 1 时保存首条真实诊断、标记截断并停止继续收集，不以 `DIAGNOSTIC_LIMIT_REACHED` 替换该错误；容量至少为 2 时保留末槽，在后续错误到达时追加 `DIAGNOSTIC_LIMIT_REACHED`。调用方应通过 `is_truncated()` 判断收集是否因上限停止；在容量为 1 时，不应再依赖诊断列表末尾必定包含上限错误码。资源超限、分配/容量失败、输入失败和内部不变量错误会终止解析；词法错误也会停止后续恢复。公开错误码类别包括词法、配置语法、配置值、环境、配置校验、资源、分配、输入、诊断超限和内部错误；具体稳定码定义在 `typol-core::DiagnosticCode`。

## 7. 资源额度和分配边界

配置调用分别使用源码片段、正则、配置、词法流和通用执行策略。`ConfigParseOptions` 与 `ConfigValidationOptions` 独立公开 `ExcerptLimits` 和 `RegexOptions`；复用通用 `ExecutionBudget` 不会跳过正则计量。

| 额度 | 控制范围 |
| --- | --- |
| `ExcerptLimits` | 源码快照总字节、单行字节、记录数量和上下文行数 |
| `RegexOptions::limits()` | 正则结构边界：模式字节/数量、嵌套、指令、字符区间和 scratch 状态 |
| `RegexOptions::execution()` | 正则步数、单次搜索 NFA 状态数和正则自有工作区；默认值均为 67,108,864 |
| `ConfigLimits::max_source_bytes` | 单个配置源允许读取的最大字节数 |
| `ConfigLimits::max_string_bytes` | 解码字符串、字段/变体名、map 键和环境文本的字节上限 |
| `ConfigLimits::max_value_nodes` | 解析值和外来值规范化期间的值节点上限 |
| `ConfigLimits::max_nesting_depth` | 值、恢复扫描、规范化、定位遍历和表达式求值的深度边界；根值深度为 0，深度等于上限仍允许 |
| `ConfigLimits::max_diagnostics` | 普通及终止配置诊断的总上限 |
| `ConfigLimits::max_environment_entries` | 本次调用缓存的不同授权环境名称数量 |
| `ConfigLimits::max_environment_bytes` | 本次调用缓存的环境原始字节总量 |
| `ConfigLimits::max_constructed_text_bytes` | 本次调用新构造/复制文本的累计额度 |
| `ConfigFormatOptions::max_output_bytes` | 格式化输出字节数，并受文档源码与构造文本额度进一步限制 |
| `ConfigFormatOptions::max_workspace_bytes` | formatter 帧存储的逻辑额度；实际分配也计入共享执行工作区 |
| `ConfigFormatOptions::max_nesting_depth` | formatter 遍历的最大分隔符深度 |
| `ConfigLimits::max_display_name_bytes` | 诊断显示名称的长度上限 |
| `StreamLimits::chunk_bytes` | 流式输入窗口大小；`StreamLimits::try_new` 在限制构造时拒绝零值 |
| `StreamLimits::max_token_bytes` | 单个 token 的源文本长度上限 |
| `ExecutionLimits` | 本次调用非正则工作的累计步数和工作区字节 |

当前 `ConfigLimits::default()` 为单源 1 MiB、100 条诊断、嵌套深度 128、字符串 256 KiB、值节点 100,000 个、1024 个环境项、8 MiB 环境字节、16 MiB 构造文本和 4096 字节显示名称；默认 `StreamLimits` 为 8192 字节窗口及 1 MiB token。源码片段默认值来自 `ExcerptLimits`，正则结构默认值来自 `RegexLimits`，正则执行默认值来自 `RegexExecutionLimits`，通用执行默认值由 `ConfigParseOptions::default()` 提供。以上均是可覆盖的运行策略，不属于 DSL 语义。

`ConfigLimits` 的九项额度均可通过对应的具名更新方法单独覆盖：

| 额度字段 | 具名更新方法 |
| --- | --- |
| `max_source_bytes` | `with_max_source_bytes` |
| `max_diagnostics` | `with_max_diagnostics` |
| `max_nesting_depth` | `with_max_nesting_depth` |
| `max_string_bytes` | `with_max_string_bytes` |
| `max_value_nodes` | `with_max_value_nodes` |
| `max_environment_entries` | `with_max_environment_entries` |
| `max_environment_bytes` | `with_max_environment_bytes` |
| `max_constructed_text_bytes` | `with_max_constructed_text_bytes` |
| `max_display_name_bytes` | `with_max_display_name_bytes` |

例如，可从默认值开始按名称设置多项额度：

```rust
let limits = ConfigLimits::default()
    .with_max_environment_entries(3)
    .with_max_environment_bytes(192)
    .with_max_constructed_text_bytes(268)
    .with_max_display_name_bytes(6);
```

未显式覆盖的额度保留 `ConfigLimits::default()` 中的值；额度为零时仍按各自原有语义处理。

默认 `ExcerptLimits` 为 65,536 字节、单行 512 字节、128 条记录及每个跨度额外两行上下文。可在解析或校验选项中直接设置；任一必要容量设为零会关闭源码片段，不会关闭错误诊断。解析期间使用的候选窗口、遮罩范围和最终快照受独立字节额度及通用会话工作区额度约束；超限或可选分配失败只会降级片段状态。

深度按值树层级计算：根值深度为 0，每条父子值边增加一层；可选值的有值载荷和枚举单载荷各增加一层。枚举匿名结构体载荷在载荷结构与其字段之间也计一层，即枚举值深度为 `d` 时，载荷字段从 `d + 2` 开始。空容器按其自身深度检查，不要求存在子项才触发上限检查。恢复扫描按根对象分隔符对应的基准槽位换算分隔符栈上限，避免把绝对分隔符数量直接当作值树深度。

直接使用 `typol-core` 的调用方通过 `ConfigValueBuilder::append_view` 复制 `ValueView` 时，必须提供 `ValueCopyLimits`；根值深度为 0，等于上限的值允许复制。复制通过显式后序帧栈完成；帧和子项暂存区使用可失败分配，并在 builder 带有共享执行预算时计入该预算。单次复制的深度额度独立于 `ExecutionLimits`；后者仍限制累计步数和工作区字节。

动态工作区使用可失败分配接口并计入执行预算。超限、工作区不足和分配失败是不同的终止错误。解析、规范化和值视图复制都使用显式帧；深度限制不依赖宿主调用栈。失败时临时值、缓存和工作区随调用释放，不发布部分结果。

## 8. 秘密值与敏感数据

`SecretPolicy` 按编译 Schema、实例路径、字段/变体及类型别名信息寻找最外层秘密边界。诊断进入缓冲区前会先隐藏敏感消息并裁剪路径；最终错误快照阶段再次执行安全处理。敏感子树的数组下标、map 键和载荷后缀不会暴露。无法将动态路径安全映射回 Schema 时，以不含路径的内部诊断终止，避免意外泄漏未知 map 键。

同一策略也用于 `TypedSchema::serialize` 和 `TypedSchema::deserialize` 的 Rust 值转换诊断，并在 `CodecErrors` 保存诊断前执行。路径无法映射回 Schema 时会清除路径；脱敏预算或分配失败时只返回不含原路径的安全诊断。没有进入转换诊断的路径（例如普通拒绝错误）本身不暴露值或字段信息。

源码行在进入错误对象前按已知秘密范围逐字节遮罩；秘密 map 项同时隐藏键和值。未完成秘密值的遮罩延伸到其输入边界。范围记录失败、跨度无效或快照额度耗尽时省略受影响片段，不回退到原始行。环境原值不进入源码窗口；默认报告只使用已裁剪的显示名称，不附加真实文件路径或 OS 错误文本。

通用 `ConfigValue` 的 `Debug` 只输出值种类、字符串/容器长度和来源，不输出字面值；请求、错误集合及外层值封装的 `Debug` 也不打印输入和错误正文。秘密标记保护错误和安全展示，不会加密成功 `ConfigValue` 中的数据；序列化和应用代码仍可取得真实值。

环境原始字节只由本次调用的缓存短期持有，并在构造环境字符串/标量后不附加到诊断。不同环境读取的稳定性由宿主 提供器 决定；进程环境适配器不承诺对不同名称提供同一环境快照。

## 9. 宿主集成

`SchemaLoader` 持有编译 Schema 和源名称。文件配置通过 `typol_std::FileConfigInput` 打开并由词法器按需读取；标准 I/O reader 配置通过 `ReadSource` 将 `std::io::Read` 适配为 `typol_core::Read`。这些适配器负责宿主 I/O 和有限错误分类，配置语法及 Schema 校验仍由 `typol-config` 完成。

启用 `typol` 的 `config-ir` feature 后，Rust 调用方可通过 `ConfigIr::serialize` 导出 Config IR v1，格式定义见 [Config IR v1 规范](../config-ir/)。默认模式先补齐默认值并执行完整 Schema 校验；需要跳过约束校验时，必须显式选择 `AsProvided`。类型和值遍历、秘密标记、确定性排序、输出/排序工作区/嵌套深度额度及序列化错误分类由 facade 实现；输出超限与排序工作区超限使用独立错误类别。默认嵌套深度为 128，可由 `ConfigIrOptions` 降低，序列化器不会允许提高该上限。`typol-cli` 在配置读取阶段已完成校验，因此以 `AsProvided` 编码已校验值，避免重复校验；它只选择环境模式、加载值、调用该 API 并将完整结果写入 stdout。其 `config fmt` 命令则读取 `ConfigDocument`、格式化保留源文并原子替换同一文件；`--check` 只比较格式化结果，不写文件。它不会把环境解析结果或补齐的默认值序列化回源文。命令用法见 [CLI 使用指南](../../guides/cli/)。IR 中 `secret: true` 仍保留对应真实值，调用方须按敏感数据处理。

`TypedSchema::deserialize` 先复用同一个配置解析和完整校验，再将拥有型值转换为 Rust 类型。`typol` 编解码器和上下文 serde 对外来/序列化值调用 `ConfigValueProcessor::validate_with_budget`，再编码或解码；绑定、配置解析和转换可共享外层通用执行余额，每次校验仍根据自身选项创建正则预算。没有 Schema 上下文的普通 serde trait 调用不属于此配置校验保证。

## 10. 验收与维护

配置实现的测试按职责分布在 `crates/typol-config/src/parser/tests/`、`src/value_validation/tests.rs` 及 `crates/typol-config/tests/`。维护解析器时应覆盖流式分块、词法/语法错误、嵌套恢复、字段默认与可选补齐、枚举载荷、环境缓存/回退/转换、标量和集合规则、完整树断言、秘密路径裁剪、输入位置快照、执行/工作区额度以及分配失败原子性。formatter 测试覆盖 token/注释保留、各类分隔符、env 与枚举表达式、LF/CRLF、尾随注释、幂等性、输出/工作区额度以及失败时不返回半成品。CLI 测试覆盖原子原地替换、重复运行幂等、只读检查模式、本地化帮助和用法错误，以及格式化失败。外围 `typol` 与 `typol-std` 测试覆盖文件、reader、进程环境、类型绑定和安全诊断集成。

若实现行为与 Config DSL 或 Schema DSL 规范冲突，应先修订对应语义规范或决策记录，再调整实现；不得由配置 parser 或宿主适配器私自改变 DSL 语义。

## 11. 有界诊断源码快照

`ExcerptLimits` 独立限制总存储字节、单行字节、记录数量和每个跨度的上下文行数。零额度关闭片段。每个跨度从起始行选取有限窗口，超出窗口的跨行范围标记为截断；上下文行总数在起始行前后各分配一半。候选窗口、脱敏范围缓冲和最终快照各自受字节上限约束，全部计入通用调用工作区；可选申请失败不改变业务诊断。片段通过 `SourceExcerptSet` 拥有，状态为 `Complete`、`Truncated` 或 `Unavailable`；缺失记录不得回退读取或显示原文。列使用 Unicode 标量计数，制表符占一列，CRLF 不进入行内容；裁剪只能落在 UTF-8 边界，裁剪行带省略标记。

配置流式输入维护受限候选和近期行窗口，诊断引用的窗口行固定至调用结束；延迟诊断只能使用仍保留的候选。解析结束只发布实际诊断及其上下文引用的安全行。已知秘密值和秘密 map 的源范围进入遮罩，未完成秘密值遮罩延伸到输入末尾。恢复跳过或尚未绑定的内容保守隐藏；范围记录失败则片段不可用。环境原值不进入行窗口。Schema 只在源目录存活时提取诊断及编译记录引用行，默认值范围统一遮罩（包括尚未解析秘密别名的默认值）；缺少完整 AST 时省略该源片段。错误及编译产物不保存完整源，所有新增类型的 `Debug` 仅显示状态及数量。
