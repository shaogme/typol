---
title: "Schema 实现说明"
description: "说明 Typol 如何加载、编译和消费 .tschem Schema。"
sidebar:
  order: 4
---

## 本页要点

:::tip[适用场景]
了解 Schema 加载、编译诊断、外部导入，或编译类型如何供配置解析和 Rust 绑定使用时，请查阅本页。
:::

| 阶段 | 当前行为 |
| --- | --- |
| 输入 | 共享流式 lexer 和 parser 生成带源码跨度的拥有型 Schema AST。 |
| 编译 | loader 解析导入、检查静态语义，并构造不可变的编译后 Schema。 |
| 使用方 | 配置解析和 Rust 绑定读取编译后的 Schema 视图；文件访问由宿主适配器提供。 |

本文说明 Typol 当前如何实现 `.tschem` Schema：各 crate 的职责、从源文本到只读编译产物的处理步骤、静态语义边界，以及配置解析和 Rust 类型绑定如何消费编译结果。它补充[Schema DSL 规范](../schema-dsl/)，不改变 DSL 的词法、语法和语义。

本文依据当前源码整理。语义以 Schema DSL 规范为准。

## 1. 实现范围与边界

Schema 实现分为四层：

1. `typol-syntax` 对 UTF-8 源执行共享词法分析，产生带字节跨度的 token，并将 Schema 解析为拥有型 AST。
2. `typol-schema` 通过宿主提供的 resolver 加载根文件及 `extern mod` 文件，建立模块/符号/类型模型，执行完整静态编译。
3. `typol-semantic` 保存编译后的类型、字段、变体、约束、表达式程序、正则程序、源位置和索引，并为调用方提供只读视图。
4. `typol-config`、`typol` 和 `typol-std` 分别在 Schema 指导下构造/验证配置值、提供应用级编排、接入文件和进程环境。

核心语法与语义 crate 使用 `no_std + alloc`。Schema 编译不自行访问文件系统；`typol-schema::SchemaResolver` 是导入读取边界。配置解析不建立配置 AST，而是消费 token 并直接构造值。`typol-macros` 在编译宿主上生成 Rust 类型描述，不读取 Schema 文件；实际兼容性由 `TypedSchema::bind` 对传入的 `CompiledSchema` 检查。

在 `no_std + alloc` 环境中通过 facade 集成应用的步骤见 [no_std 指南](../../guides/no_std/)。

主要源码位置：

| 职责 | 实现位置 |
| --- | --- |
| 词法、token 游标、Schema AST/parser | `crates/typol-syntax/src/lexer.rs`、`token.rs`、`cursor.rs`、`parser.rs`、`parser/`、`ast.rs`、`storage.rs` |
| 导入加载、编译会话、模块和符号 | `crates/typol-schema/src/resolver.rs`、`loader.rs`、`session.rs`、`module_tree.rs`、`symbols.rs` |
| 类型解析、别名与递归检测 | `crates/typol-schema/src/types.rs`、`types/` |
| 注解、正则和表达式静态编译 | `crates/typol-schema/src/annotations.rs`、`regex.rs`、`expressions.rs`、`expressions/` |
| 默认值编译与最终 Schema 记录 | `crates/typol-schema/src/defaults.rs`、`defaults/`、`compiled.rs`、`compiled/records.rs` |
| 编译结果和语义视图 | `crates/typol-semantic/src/model.rs`、`views.rs`、`constraints.rs`、`expressions.rs` |
| 配置解析和运行期校验 | `crates/typol-config/src/parser.rs`、`validation.rs`、`value_validation.rs`、`environment.rs` |
| 文件加载和应用 facade | `crates/typol-std/src/schema_compiler.rs`、`crates/typol/src/loading.rs`、`binding.rs`、`descriptor.rs` |

## 2. 源输入和词法/语法阶段

### 2.1 字节流与 token

Schema 输入必须是有效 UTF-8。词法约定以[Schema DSL §1](../schema-dsl/#1-词法规则)为准。`ByteSource` 按顺序提供字节，短读不代表结束，零字节表示 EOF；parser 不要求输入可重放，也不保存完整源文本。Lexer 按需扫描，不生成完整 token 数组；`TokenCursor` 最多缓存当前事件和一个前瞻事件。token 保存 `SourceSpan`（源 ID 与 UTF-8 字节范围），标识符/数字可读取原文，字符串提供解码内容。EOF 的跨度是源末尾的空范围。

标识符和数字 token 使用受预算约束的拥有缓冲；Schema parser 借用 token 文本并复制到 AST 文本池。字符串始终使用受预算约束的拥有型解码缓冲，无论原文是否含转义；Schema parser 将解码缓冲直接移交 AST。共享 lexer 通过可复用窗口读取 `ByteSource`，不会向 token 暴露稳定的源文本切片。token、AST 的调试输出避免打印字面值，只暴露种类、跨度和长度等信息。

### 2.2 Schema AST

`SchemaParser::parse(source_id, byte_source, options, budget)` 接收 `ByteSource<Diagnostic>` 并使用统一的 `SchemaParseOptions` 配置 `SyntaxLimits` 与 `StreamLimits`；`parse_str` 只将字符串包装为 `SliceSource`。需要后续源位置查询时使用 `parse_with_positions`，成功结果同时拥有 `SchemaAst` 和预算化 `SourcePositionIndex`。位置索引记录词法 token 起止端点的字节偏移、行号和 Unicode 标量列号，不保留源字节；EOF token 提供实际源长度及 EOF 位置。parser 不预读或缓存整份输入；来源上限在读取期间约束输入，EOF token 确认后的实际字节偏移用于 `SchemaAst::span()`，AST 存储也在成功完成时将跨度边界收紧到该长度。诊断初始化和 EOF 未知时的通用锚点使用当前已知位置，EOF 语法错误使用 EOF token 的精确跨度。默认调用可使用 `SchemaParseOptions::default()`。成功时返回 AST；词法或语法错误返回非空 `SyntaxErrors`，不会返回可被误用的部分 AST。parser 在顶层声明、模块成员、字段和变体边界的可靠同步点继续收集后续独立错误；恢复循环与 parser 的 token 访问共用 `ExecutionBudget`，资源耗尽、分配失败和内部错误等终止错误会停止解析。

`SchemaAst` 由分表节点存储和拥有型文本缓冲持有，不借用输入源。公开节点是只能通过所属 AST 读取的只读视图；节点、ID 和存储表不可由调用方伪造。AST 保留：

- 顶层项、模块声明、字段、变体的源顺序及各自 span；
- 别名目标、容器/数组长度、类型路径分段；
- 单元/单载荷/匿名结构体三类枚举载荷；
- 注解的结构化参数和参数源顺序；
- 默认值原始数值文本、`none`、数组、枚举，以及大括号键是标识符还是字符串；
- 断言/集合表达式的运算结构、路径、方法调用和闭包参数。

解析阶段仅确认语法，不判定名称是否存在、类型是否兼容、注解是否适用，也不执行表达式。空 `{}` 保留为未判型的大括号字面值，由后续编译阶段根据目标类型判为对象或 map。

### 2.3 Parser 深度和 AST 预算

Schema parser 从 `SchemaLimits::syntax()` 读取 `SyntaxLimits`：解析深度、AST 节点、字符串解码、单源字节数和诊断数量均由该类型控制。`SchemaLimits::stream_limits()` 单独提供 token 扫描窗口和单 token 字节上限，并通过 `set_stream_limits()` 配置。流限制通过 `StreamLimits::try_new()` 构造，零窗口会立即返回 `StreamLimitsError`；token 上限为零仍可用于拒绝任何非空 token。Schema loader 将实际流读取限制在单源和全局剩余额度加一个探测字节内，并把字节 tee 到受限源码候选捕获器；成功解析必须到达 EOF。默认 token 窗口为 8192 字节，token 上限为 1 MiB。AST 节点统计包括实际构造的节点（也包括恢复过程中后来被丢弃的节点），每个文件单独计数。AST 还提供 `node_count()` 和 `structure_height()` 供只读检查。默认值与工作区构造均通过可失败分配路径处理。

当前 parser 使用显式续接帧处理嵌套声明、类型、默认值和表达式；恢复过程也受工作预算约束。实现应维持本节列出的解析深度、节点数、输入字节和诊断数量边界。

## 3. Schema 编译会话

### 3.1 编译入口

通用/no_std 调用方使用 `SchemaCompiler::new(SchemaCompileOptions)` 建立一次性会话。需要导入时调用 `compile(root, resolver, observer)`；不含 `extern mod` 的内存源可直接调用 `compile_without_imports(root)`，该入口遇到导入时会给出明确诊断。需要登记源名称时使用 `compile_without_imports_with_observer`。`SchemaSource::from_source` 接收一次性 `ByteSource<SourceReadError>`，`from_str` 为内存字符串提供便捷适配；源身份按字节精确比较，但不写入编译产物。源对象不能复制，loader 消费后输入不可复用。可选 `SchemaCompileContext` 将根读取、resolver 和编译各阶段绑定到同一调用余额。

`SchemaResolver::resolve` 接收当前源身份、单段模块名、剩余源字节额度和编译上下文，返回下一份 Schema 源或有限的 `ResolverError`。导入者负责如何映射模块名到文件/内存源。可选 `SourceObserver` 按源登记身份到 `SourceId` 的关联，供宿主保存可读源名称；身份重复时不会重复登记。

显式绑定到其他调用的根源会以 `SCHEMA_CONTEXT_MISMATCH` 失败；执行上下文已结束时会以 `SCHEMA_SESSION_FINISHED` 失败。`SchemaCompileErrors::context_failure()` 可读取对应的 `SchemaCompileContextFailure`，诊断保留根源跨度，错误展示包含具体原因。两类调用契约/生命周期错误不会调用 resolver；resolver 自身的宿主失败仍使用 `SCHEMA_RESOLVER_ERROR`。

标准文件调用方可使用 `typol_std::FileSchemaCompiler::compile`。该适配器打开并检查文件句柄，由 parser 按需读取 Schema 正文，不构造 `FileText` 全文缓冲；它也负责相对导入路径、宿主文件身份和源名称表。Schema 的词法、语法、名称解析和静态语义仍由核心实现。应用级 `SchemaLoader::compile_file` 编译文件，`compile_text` 通过 `SchemaSource::from_str` 提供无需 resolver 的内存入口，`compile_with_resolver` 用于自定义导入。加载完成后，调用 `LoadedSchema::bind::<T>()` 创建可复用绑定，再通过 `BoundSchema::read` 读取多份配置，避免每次读取都重新匹配 Rust 与 Schema 类型。

### 3.2 导入图加载

根文件被分配 `SourceId(0)`；后续成功登记的源按加载顺序获得后续 ID。loader 按根 AST 中的 `extern mod` 声明请求源，并通过显式栈深度优先加载。每条导入边保留状态和可选目标，失败边没有有效目标。

源身份按字节精确比较：遇到仍处于 `Loading` 的身份即诊断导入环；遇到已经加载过的身份则诊断重复导入。不能因两个不同身份返回了相同字节而认定是同一源。loader 在解析前登记 `SourceId` 并调用 observer；导入身份去重后才读取正文。源读取错误由有限 `SourceReadError` 类别转为结构化诊断，不向诊断写入宿主任意错误文本。

`FileSchemaResolver` 将已打开文件的稳定 ID 映射为上述源身份，并在会话期间保留句柄，防止文件 ID 被复用。Unix 使用设备号和 inode，Windows 使用卷序列号和文件索引；硬链接因此共用源身份。无法取得稳定 ID 的平台按规范路径去重。Linux 有 `/proc/self/fd` 时，导入基路径也从实际读取句柄解析；不可用时按请求路径规范化退化。文件内容仍是按需读取，不形成快照。

单文件字节数、所有源总字节数、Schema 文件数和导入嵌套深度在读取时检查。读取边界按实际字节计量，并最多额外请求一个字节以区分恰好达到额度与超限；底层错误可报告已消费字节数。Schema 导入声明只存在于 AST 文件根项；语言中的普通 `mod` 是同一文件内的命名空间层级。

### 3.3 一次调用共享预算

编译会话对源加载、解析、语义 pass、记录生成、索引、位置映射和冻结共用 `ExecutionBudget`。除语言资源上限外还有执行步骤与工作区字节额度；超限和分配失败是不同错误。阶段标签记录当前正在进行的工作阶段，以支持宿主观测。

冻结索引按声明槽位保存字段和变体的排序名称查找表，索引分配与比较均消耗同一预算。索引只保存槽位，记录中的字段和变体仍保持 Schema 声明顺序；只读变体查询可使用预算化二分查找。

默认策略由不同职责的类型分别提供：`SchemaLimits` 限制单源 1 MiB、Schema 总源 8 MiB 和 64 个源；其 `SyntaxLimits` 限制嵌套深度 128、100 条诊断、解码字符串 256 KiB 和每文件 100,000 个 AST 节点；其 `StreamLimits` 默认使用 8192 字节窗口和 1 MiB token 上限；Schema 默认值验证使用 100,000 个值节点。正则的模式字节、数量、嵌套、指令、字符范围、搜索步数和 scratch 状态额度由 `RegexLimits` 提供。源码诊断快照的默认值为 65,536 字节、单行 512 字节、128 条记录及每跨度额外两行上下文。数值是运行策略，可以由调用方覆盖，不属于 DSL 语义；默认值分别定义在 `typol-schema::SchemaLimits`、`typol-syntax::SyntaxLimits`、`typol-syntax::StreamLimits`、`typol-policy::ValueLimits`、`RegexLimits` 和 `ExcerptLimits`。

## 4. 静态编译流水线

所有步骤均在 `typol-schema::SchemaCompilation::compile` 中按顺序编排。遇到诊断后不生成可用的 `CompiledSchema`；只有全阶段无错误才返回最终产物。

### 4.1 解析各源

loader 对根源和每个新导入源调用 `SchemaParser`。解析错误保留对应 `SourceId` 和源跨度。语法失败的源不会进入后续模块树；其他可独立加载的源仍可能用于收集诊断，除非发生终止预算错误。

### 4.2 模块树与命名空间

`ModuleTree` 将每个源的根声明放入相应模块，并递归加入普通 `mod` 声明。模块树和类型声明按稳定 ID 存放，模块成员保持插入顺序。根文件的 `entry struct` 被标为唯一配置入口；导入文件的 entry 标记不会替代根入口，它在声明树中仍表现为可引用类型。

`SymbolTable` 为每个模块建立共享的模块/类型命名空间索引，并为结构体字段、枚举变体和结构体载荷字段建立索引。它检查同层声明重名、入口数量、字段重名和变体重名；重复项会关联先前声明的位置。查找索引返回缺失、唯一命中或歧义状态，不静默选择重复声明。

### 4.3 类型路径和已解析类型图

类型引用从当前模块开始，向父模块逐层查找路径首段；首段命中后，剩余路径段只能在模块命名空间继续解析。中间段若命中类型而不是模块、最终段若不是类型、目标模块/符号无效，分别产生语义错误。

类型图为内建类型、用户类型、容器类型、别名链接和匿名结构体变体载荷分配语义 ID。别名是透明的类型形态：编译结果会保留命名别名记录以及目标底层形态，使用点仍可保留别名约束和 `@secret` 元数据。别名环保持独立检测；之后编译器以受预算约束的工作队列计算可产生有限值的用户声明最小不动点。内建类型、option、动态数组、map 和零长度定长数组可构造；结构体要求全部字段可构造，枚举至少要求一个可构造变体。正长度定长数组要求其元素类型可构造。固定点中仍无有限值的递归组件使用 `SCHEMA_SEMANTIC_ERROR` 并报告“递归类型不存在有限值”。未知引用和别名环等前序无效声明不会再引出递归级联诊断。未知路径、嵌套 option 及超范围定长数组继续使用原诊断。

字段及载荷解析前会为命名结构体和枚举预分配 ID，因此递归引用始终是有限 ID 边，不会展开成无限声明树。冻结校验会检查索引范围、字段和载荷引用、owner 以及嵌套 option 规则，并接受合法回边。

类型记录包括内建类型、`option`、数组（可选固定长度）、map、结构体和枚举。结构体与枚举按声明顺序保存字段/变体引用；枚举变体显式保存 unit、单值或匿名结构体载荷类型。

### 4.4 注解和正则

注解编译器将 AST 注解按别名、字段和单载荷变体分层存储。每个槽包括编译约束、秘密位和可选环境授权记录。它检查注解的目标类型、参数形式、重复注解、边界顺序及叠加约束兼容性。使用别名的字段通过有效规则视图同时应用别名层和局部层约束。

`@regex` 的字符串在 Schema 编译期交给 `typol-regex` 编译为受限 Thompson NFA；实现细节见[正则实现说明](../regex-implementation/)。非法模式映射到正则语法诊断；模式字节、正则数量、单模式/累计指令数和字符范围数量累计受限。编译后约束只保存 regex 程序引用，不在配置每次校验时重新编译。

`@env` 被编译为字段环境授权记录，不使 Schema 编译器读取环境。别名不能持有环境授权。`@secret` 元数据会传播到使用点，并供配置诊断、安全展示和 Rust 值转换共享。

### 4.5 表达式静态分析

表达式编译器解析 AST 中的 `@assert` 和 `@all`，分类 `self`、`root`、闭包参数及 map 的 `key`/`value` 上下文，对路径、方法、字面值推断、运算符和比较逐节点检查类型，并要求程序结果为布尔值。无效字段、方法、上下文、操作数或非布尔结果使整个 Schema 编译失败。

成功表达式编译为 `typol-semantic` 中的类型化表达式节点和程序 ID。`@all` 的约束槽稍后绑定到表达式程序。Schema 编译期间不执行实例断言；静态默认值的集合约束由默认值构造校验阶段执行。

### 4.6 默认值静态构造

默认值编译以字段已解析类型为上下文，将语法节点构造成 `ConfigValue`：检查目标标量形态、数值范围和有限浮点，递归构造数组/map/对象/枚举载荷。整数类型默认值只接受整数 token，浮点类型默认值只接受浮点 token，与配置字面值一致；`f32` 浮点 token 直接解析到 binary32，避免先转 `f64` 再舍入。`{...}` 根据目标是结构体还是 map 判型；对象字段未知/重复或必填字段缺失均使 Schema 无效。

对象默认值可以省略由字段默认值或 option 规则可补足的字段。字段默认值依赖以显式帧解析，并检测依赖环；即使 Schema 类型本身合法递归，也不会放过省略字段造成的默认值无限展开。编译后的嵌套默认值会复制到适用字段。Schema 默认值不会访问环境。默认值构造阶段递归检查目标形状以及局部、别名继承、字段和活动枚举载荷约束，并保留错误恢复、元素位置和标签。校验成功时会产生 `SchemaDefaultValidation` 结果；完整 Schema 编译将该结果随同同批默认值传入 `Builder::freeze_with_validated_defaults`，从而省去冻结时的重复遍历。普通 `Builder::freeze` 不接受校验结果，仍对任意直接构造的输入完整校验。值节点、深度、字符串、正则执行和规则工作均沿用 Schema 编译配置的 `ValueLimits`、`RegexLimits` 与共享 `ExecutionBudget`。违反约束会阻止产生校验结果，不会冻结出无效默认值。跨字段 `@assert` 依赖完整配置实例和 `root` 上下文，仍在配置值构造后执行。

### 4.7 记录冻结与返回

`RecordBuilder` 将临时模块/符号/类型图、约束槽、默认值、表达式程序和源位置信息冻结为 `typol-semantic::CompiledSchema`。冻结先构建字段/变体槽位索引并校验记录结构；普通 `Builder::freeze` 还会完整校验每个默认值，完整 Schema 编译则使用与默认值绑定的校验结果避免重复遍历。只有全部检查通过才返回编译模型。最终模型通过只读 `TypeView`/字段/变体/有效规则视图读取；外部调用方不能改变内部记录、索引和编译状态。结构字段查询依据排序槽位索引；表达式读取字段时先确认声明槽位与值项名称一致，再直接读取该项。变体排序索引只映射回保持声明顺序的记录。`CompiledSchema::entry()` 返回根入口类型视图。

最终记录还包含源 span 到 1-based 行列 `SpanLocation` 的索引。位置由解析期 `SourcePositionIndex` 按端点查询；冻结时 `SourceCatalog` 通过源长度和索引中的 UTF-8 边界验证跨度，不读取源文本。行号/列号按 Unicode 标量计数；span 本身仍是 UTF-8 字节偏移。成功记录缺少端点时编译失败。错误返回若诊断位置无法由诊断或索引提供，则保留原始诊断并通过 `LocationStatus` 报告不可用，不回读源文件。

`LoadedFile` 仅保留身份、AST、实际字节长度、位置索引及有界候选捕获器，不保留完整源文本。每个源的候选字节和记录额度按最大源数切分，以保证本次编译的候选捕获总量不超过 `SchemaLimits::excerpts()`；所有存储仍计入共享工作区。AST 完整后，所有默认值跨度先被遮罩，再固定诊断或编译记录引用的候选行。成功产物通过有界 `SourceExcerptSet` 保存安全行；编译失败则在源目录释放前提取诊断所需片段并交给 `SchemaCompileErrors` 拥有。快照缺少候选时标记截断或不可用，不回读源文件，也不会改变主要诊断。

## 5. 静态错误与诊断契约

Schema 错误由 `SchemaCompileErrors` 以非空集合或不需分配的内联紧急错误返回。公开读取接口为 `diagnostics()`、`span_location()` 和 `location_status()`。诊断带稳定错误码、主要跨度和可选标签；重复名称诊断可同时指出后续和先前声明。

`SchemaCompileErrors` 实现 `Display` 和标准错误接口。`Display` 写出错误码名称与数值、消息、可用位置和安全源码片段，不先申请拥有型报告字符串；`Debug` 只显示诊断数量与位置状态。`internal_cause()` 可查询结构化的 `SchemaInternalCause`，其中 AST 访问失败保留原始错误变体、完整 `usize` 索引及范围端点；内部原因也会出现在展示文本中。源码快照可通过 `excerpts()` 读取。额度为零或收集失败时仍保留诊断和可用位置，源码片段状态为 `Unavailable` 或 `Truncated`。

当前相关错误码定义在 `typol-core::DiagnosticCode`：

| 诊断类别 | 错误码 |
| --- | ---: |
| 资源超限 | 1 |
| 分配失败 | 2 |
| 词法错误 | 3 |
| Schema 语法错误 | 4 |
| 诊断数量到限 | 5 |
| 正则语法错误 | 6 |
| Schema 静态语义错误 | 7 |
| resolver/宿主导入错误 | 8 |
| 根源编译上下文不匹配 | 19 |
| Schema 编译会话已结束 | 20 |

资源、分配和内部错误优先作为终止诊断；普通诊断按源 ID 和字节跨度稳定排序，达到诊断上限时保留终止槽。诊断不应复制完整源码行、环境原值或敏感默认值。宿主错误使用有限类别，不将任意 OS 错误文本纳入核心诊断。

词法错误与 Schema 语法错误分别由 syntax 层产生；缺失导入、名称/类型不匹配、注解错误和表达式类型错误由 Schema 编译产生。Schema 无效时调用方必须停止配置解析，不能将部分记录用于加载。

## 6. 编译产物在配置期的使用

`typol-config::ConfigParser::parse` 要求有效 `CompiledSchema`、配置源/选项和 `Environment` 实现。解析器从 `schema.entry()` 开始按预期类型读取配置 token，直接创建值树。结构体和 map 的 `{}` 解释由编译类型决定；解析不会尝试另一种解释来挽救类型错误。

完成结构体或匿名结构体载荷时，解析器对每个字段应用显式值、字段默认值、option 的 `none`/可选提升规则，并诊断未知、重复或缺失必填字段。数组、定长数组、map 和枚举载荷按 Schema 记录校验形态。所有值完整后才执行结构体与当前活动枚举变体断言。任何构造或校验错误都不返回部分实例。

局部标量/字段约束在值完成时检查；数组/map 集合约束在容器完成时检查；完整树断言最后执行。环境读取仅在配置显式使用 `env(...)` 时发生，并受 Schema 字段的 `@env` 授权；同一调用中的同名原始环境值仅读一次，但每个字段引用独立转换/验证。Schema 默认值不来自环境。各值来源（配置 literal、Schema 默认、环境、fallback、Rust 转换）在 `typol-core::ValueSource` 区分。

编译产物中的别名约束、字段/载荷约束、表达式程序、正则程序、秘密标记及源位置，在配置期按只读视图访问。运行时正则搜索使用调用方额度和有界 scratch；预算耗尽是资源错误，不是匹配失败。

## 7. Rust 类型绑定和宿主入口

`#[derive(Typol)]` 只从 Rust 类型本身生成类型描述和转换实现。它不在宏展开时读取 Schema。`LoadedSchema::bind` 和 `TypedSchema::bind(compiled_schema, options)` 要求类型具备隐藏的 `BudgetedTypol` 标记；derive 为所有嵌套字段和枚举载荷添加标记约束。内建标量与容器按其子类型条件实现该标记。derive 支持 `Vec<Self>` 等递归形态；绑定器按 Rust `TypeId` 与 Schema `TypeView` 配对追踪活动分支，遇到相同配对时结束该分支，并继续独立检查同一 Rust 类型在其他 Schema 使用点的映射。绑定、转换、节点、执行和工作区额度仍约束有限递归值。若派生类型包含手写 `Typol` 子类型，应添加 `#[typol(trusted)]` 以省略该类型的常规额度标记，并通过 `bind_trusted` 使用。

当应用逻辑需要字段来源时，使用 `Sourced<T>`。它与 `T` 具有相同的 Schema 形态，`source()` 返回解码值节点的来源。对于可选字段，`Option<Sourced<T>>` 在值存在时记录内层值的来源。`Sourced<T>` 不实现 `Debug`，避免意外打印包装的秘密。`BoundSchema::read_with_value` 返回 `BoundRead<T>`，其中包含类型化结果和同一次解析、校验会话产生的 `ValidatedConfigValue`；`read` 仍是只返回类型化结果的便捷方法。

手写实现通过 `LoadedSchema::bind_trusted` 或 `TypedSchema::bind_trusted` 接入，返回的 `TrustedBoundSchema` 或 `TrustedTypedSchema` 会保留这一路径标识。受信任回调使用 `DescribeContext`、`ConversionContext` 和子类型分发时，对应的库管理工作仍受现有额度约束；库无法限制回调直接分配、阻塞或执行任意计算，也不重新校验手写解码器返回的 Rust 值。`BudgetedTypol` 只是常规 API 的能力标记，不是沙箱或防止恶意伪造的安全边界。需要约束不可信实现时，必须另行使用进程或 Wasm 隔离。

普通记录和枚举映射应优先使用 derive；只有存在特殊映射且应用已审查并信任其回调时才手写实现。迁移时检查直接 `impl Typol`，以及 derive 类型字段/载荷的传递嵌套；含手写类型的整个父类型都改为 `bind_trusted`，或把映射重构成普通字段和枚举后使用 derive。配置读取应显式执行 Schema 编译、类型绑定和读取；手写类型通过加载后调用 `bind_trusted`。

内建 map 绑定支持无需 `std` 的 `OrderedMap<T>`、启用 `std` 后的 `std::collections::HashMap<String, T>`，以及启用可选 `hashbrown` feature 后的 `hashbrown::HashMap<String, T>`。`OrderedMap` 按 Rust 字符串序保存键；其可失败插入会替换重复键并返回旧值，而转换解码会拒绝重复输入键。解码先对输入键引用排序，再预留最终向量容量，避免构造 map 时发生二次方级元素移动。哈希 map 会在解码条目前预留容量；两种路径都按实际元素容量记入目标额度，并单独统计拥有型键文本。该目标额度估算不包含哈希表控制字节和 allocator 元数据。hashbrown 当前默认的 foldhash 构建器通常不防御 HashDoS，键可由攻击者控制的应用应评估这一取舍。标准库 `HashMap` 仍可使用，并保留其不同的默认哈希构建器。`BTreeMap` 没有可失败的节点预留接口，因此不提供自动 `Typol` 实现。

同时启用 `serde` 和 `hashbrown` 时，Typol 会转发 hashbrown 的 Serde 支持。Serde 自身的 `Deserialize` 回调可能独立分配内存，不经过上述可失败的预留路径。

文本解析、外来 `ConfigValue` 规范化和 Rust 序列化使用共享的完整树 Schema 校验入口。Rust 值不能跳过字段/集合约束或完整树断言；序列化输出按 Schema 字段顺序和 map 解码键序构造。上下文 serde 方法在绑定后使用同一验证路径；无 Schema 上下文的普通 serde trait 调用不承诺验证实际 `.tschem`。

`BoundSchema::write(value, options, out)` 和受 feature 控制的 `write_serde` 使用 `WriteOptions`；`WriteOptions::default()` 选择紧凑格式，并在调用方未显式覆盖时继承加载 Schema 的转换额度。`with_pretty(true)` 会将非空结构体、map 和数组展开为两空格缩进的多行文本，每行一个条目并带尾逗号；空容器仍保持内联。`CodecOptions::with_pretty` 为 `TypedSchema::serialize` 和 `serialize_serde` 提供相同选择。格式空白会计入输出和执行额度；编码器仍使用迭代路径，失败时不会返回部分文本。输出末尾没有换行，且仍包含 `@secret` 的实际值。

### Serde 桥接支持矩阵

`TypedSchema::serialize_serde` 和 `TypedSchema::deserialize_serde` 是 Typol 提供的上下文桥接：它们在成功绑定的 Schema 下消费/产生 Serde 数据模型事件。应用直接把普通 Serde trait 交给自选的 serializer/deserializer 时，不经过该桥接，也没有实际 `.tschem` 的校验保证。无论绑定是否带有 `BudgetedTypol`，调用方提供的 `Serialize`/`Deserialize` 实现都属于用户代码信任边界；桥接只计量其控制的操作，不能限制这些回调的直接分配、阻塞或任意计算。完整矩阵也列于 `typol` crate 的 Rust 文档。

| Serde 数据模型操作 | `serialize_serde` | `deserialize_serde` |
| --- | --- | --- |
| bool、整数、有限浮点数、字符串 | 支持 | 支持 |
| `Option<T>` | 支持；拒绝嵌套 option | 支持；拒绝嵌套 option |
| 序列 | 支持，按 Schema 数组编码 | 支持，按 Schema 数组解码 |
| tuple | 支持，但必须映射到长度相同的 Schema 定长数组 | 支持，但必须映射到长度相同的 Schema 定长数组 |
| map | 支持，键必须是字符串 | 支持，键必须是字符串 |
| 命名结构体 | 支持，类型名和字段名须匹配 Schema | 支持，类型名和字段名须匹配 Schema |
| 枚举变体 | 支持 unit、newtype 和命名字段载荷 | 支持 unit、newtype 和命名字段载荷 |
| `serialize_char`、bytes、独立 unit、unit/newtype/tuple struct、tuple enum variant、`collect_str` | 拒绝 | 不适用 |
| `deserialize_any`、`deserialize_ignored_any`、根级 `deserialize_identifier`、`char`、bytes、独立 unit、unit/newtype/tuple struct、tuple enum variant | 不适用 | 拒绝 |

Serde 桥接会拒绝使用三个稳定的 `DiagnosticContext::failure_point()` 标识，且它们共用 `RUST_CONVERSION_ERROR` 诊断码：

- `serde-data-model-operation-unsupported`：请求了矩阵未支持的 serde 数据模型操作。
- `serde-user-custom-rejection`：Serde 实现通过 `serde::de::Error::custom` 或 `serde::ser::Error::custom` 拒绝操作。
- `serde-schema-shape-mismatch`：已支持的操作与绑定 Schema 的类型、名称、字段、枚举载荷或定长数组形状不匹配。

Serde 在读取结构体字段名和枚举变体名时会内部使用 `deserialize_identifier` 读取字符串标识符；矩阵中的“根级”限定表示直接把目标类型作为 identifier 解码不受支持。

调用方可从 `CodecErrors::diagnostic_context(index)` 获取相应静态上下文。自定义错误文本不会保存在错误载体中，也不会进入 `Display`、`Debug` 或诊断，避免泄漏敏感输入。完整 Schema 值校验在 serde 序列化事件转换后执行；资源、分配和 Schema 校验错误仍使用各自的诊断类别。

`typol-std` 提供 `FileSchemaResolver`/`FileSchemaCompiler` 与 `ProcessEnvironment` 等标准库适配。`SchemaLoader` 和 `LoadedSchema` 组织 Schema 加载、源名称、类型绑定和配置读取；核心 parser/compiler 不调用文件 API、`std::env` 或终端输出。

加载 facade 在 `no_std + alloc` 和 `std` 构建中使用同一组 `SchemaLoader`、`LoadedSchema`、`BoundSchema`、`ConfigInput`、`TypolError` 与 `TypolStage` 类型。共有读取入口显式接收 `Environment`；文件和 reader 输入及进程环境快捷入口仅由宿主适配器提供。

## 8. 安全和数据处理要求

- 源位置内部始终使用字节偏移；格式化时才换算为行列。
- 环境原始字节缓存仅存在于单次配置调用，不把原始值拷入普通诊断。
- `@secret` 以及别名传播的秘密位驱动安全诊断/展示；敏感容器隐藏子项路径和 map 键。
- `TypedSchema::serialize` 和 `TypedSchema::deserialize` 产生的 Rust 值转换诊断，在进入 `CodecErrors` 前按同一秘密边界隐藏消息并裁剪路径；无法解析的路径会被清除，脱敏资源失败不会回退到原始路径。
- 默认值源范围及秘密配置子树在源码快照发布前遮罩；秘密 map 项同时遮罩键和值。若遮罩范围无法完整确认，则省略该行，不输出原始源码。
- 错误和编译产物只保留有界安全源码行，不保留完整输入；环境原值、真实文件路径和原生 OS 错误文本不进入默认报告。
- 值的实际编码必须保留秘密数据以支持还原；安全展示与序列化是不同用途。
- `ConfigValue`、Schema token 和编译错误的通用 `Debug` 不得递归暴露可能敏感的字面值。
- 资源或分配失败不得 panic、无限递归或返回部分 AST、部分 Schema、部分配置值。
- Unicode 字符长度、正则字符、源列和 map 键等价规则分别遵循 DSL 定义；不可用 UTF-8 字节数、区域设置或隐式归一化替代。

## 9. 验收与维护

维护实现时至少按层覆盖：词法与 AST 结构；导入缺失/环/重复身份；名称空间和根入口；路径解析、别名环及类型递归；注解适用性和边界冲突；默认值递归完整性及约束；表达式类型/上下文；正则子集与累计额度；源位置、诊断上限和分配故障；配置期对编译记录的实际消费；Rust 类型绑定与上下文编解码。

测试应比较稳定诊断码、位置和结构化结果，不依赖调试字符串。规范示例的公共端到端验收与资源策略复核应纳入回归测试。若实现与 Schema DSL 规范冲突，先登记并修订规范/决策记录，再调整实现，不能由 parser 或编译器私自确立新语义。

### 9.1 有界诊断源码快照

`SchemaLimits::excerpts()` 提供独立的 `ExcerptLimits`：总存储字节、单行字节、记录数量和每个跨度的上下文行数。零额度关闭片段。每个跨度从起始行选取有限窗口，超出窗口的跨行范围标记为截断；上下文行总数在起始行前后各分配一半。候选窗口、脱敏范围缓冲和最终快照各自受字节上限约束，全部计入调用工作区；可选申请失败不改变业务诊断。片段通过 `SourceExcerptSet` 拥有，状态为 `Complete`、`Truncated` 或 `Unavailable`；缺失记录不得回退读取或显示原文。列使用 Unicode 标量计数，制表符占一列，CRLF 不进入行内容；裁剪只能落在 UTF-8 边界，裁剪行带省略标记。

配置流式输入维护受限候选和近期行窗口，诊断引用的窗口行固定至调用结束；延迟诊断只能使用仍保留的候选。解析结束只发布实际诊断及其上下文引用的安全行。已知秘密值和秘密 map 的源范围进入遮罩，未完成秘密值遮罩延伸到输入末尾。恢复跳过或尚未绑定的内容保守隐藏；范围记录失败则片段不可用。环境原值不进入行窗口。Schema 候选仅在完整 AST 上应用默认值遮罩，随后提取诊断和编译记录引用的行；语法失败、边界无法证明或捕获失败时丢弃原始候选。错误及编译产物不保存完整源，所有新增类型的 `Debug` 仅显示状态及数量。
