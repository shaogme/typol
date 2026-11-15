---
title: "正则实现说明"
description: "说明正则编译、搜索语义、资源限制和运行期工作区。"
sidebar:
  order: 6
---

## 本页要点

:::tip[适用场景]
排查 `@regex` 编译、搜索行为或正则专用资源错误时，请查阅本页。
:::

- **匹配：** Typol 默认搜索任意位置；整串匹配需使用 `^` 和 `$`。
- **执行：** 模式编译为 Thompson NFA，不回溯，也不支持宿主库扩展语法。
- **限制：** 编译和匹配均有明确额度；额度耗尽会返回错误。

本文介绍 Typol 当前如何编译和执行 Schema 的 `@regex` 注解。正则语言及
可观察的匹配规则以 [Schema DSL 规范 §5.1](../schema-dsl/#51-正则语法与匹配)
为准；本文说明实现方式和限制，不扩展 DSL。

## 1. 范围与源码索引

`typol-regex` 是独立的 `no_std + alloc` 正则编译器和匹配器，不读取 Schema 文件、
配置环境或宿主资源。编译和匹配入口都需要 `RegexExecutionBudget`；通用的
`ExecutionBudget` 则计量其他工作。

Schema 编译会用同一份正则预算处理模式编译和 Schema 默认值中的正则检查。每次
配置解析及外来值校验也会在调用范围内共用一份正则预算。仅规范化外来值的操作
不会创建或消耗这份预算。

- 解析器和 AST：`crates/typol-regex/src/parser.rs`、`parser/character_class.rs`
- 字符集合：`crates/typol-regex/src/char_class.rs`
- NFA 编译器：`crates/typol-regex/src/compiler.rs`
- 匹配器和 scratch 工作区：`crates/typol-regex/src/matcher.rs`
- 错误及模式跨度：`crates/typol-regex/src/error.rs`
- Schema 和语义层接入：`crates/typol-schema/src/regex.rs`、
  `crates/typol-semantic/src/regex.rs`、`crates/typol-semantic/src/engine/regex.rs`
- 资源默认值：`crates/typol-policy/src/lib.rs`

## 2. Schema 编译

Schema 编译器接受目标为 `string` 或 `option<string>` 的 `@regex`。可选值为
`none` 时直接通过，不执行匹配；有值时搜索字符串。Schema 词法器先解码注解
字符串，再把解码后的 UTF-8 模式交给正则编译器；编译器解析 AST 并生成自有的
Thompson NFA。

编译结果及注解的 `SourceSpan` 会存入 `CompiledRegexConstraints`。运行期按该跨度
查找程序，不会重新解析或编译模式。解析器以显式帧处理嵌套分组，构造 NFA 时则
逐步检查指令数和字符区间数上限。

有效目标注解只有通过程序数量检查后，才会将尝试模式字节数计入总量；计数发生在
语法解析之前。因此语法错误仍消耗尝试字节额度，却不会计入公开 `RegexCounts`
产物统计。数量上限只约束成功保存的程序，并且会在处理下一条注解的字节数之前检查。

语法错误映射为 `REGEX_SYNTAX_ERROR`，位置指向 `@regex` 注解。资源、分配、容量和
内部错误保留不同诊断类别。底层语法错误只保存错误类型及解码模式内的
`PatternSpan`；底层错误不会复制模式文本或待匹配的值。

## 3. 解析与字符集合

解析器跨度是解码后模式中的 UTF-8 半开字节区间，不是 Schema 源文件偏移。解析器
使用显式帧，避免分组嵌套深度带动宿主调用栈增长。支持的语法详见
[Schema DSL 规范 §5.1](../schema-dsl/#51-正则语法与匹配)。

解析器拒绝空选择分支、空字符类、反向范围、端点不是字面字符的范围、重复量词和
未支持的转义。环视、反向引用和 Unicode 属性转义等宿主正则库语法也不受支持。

字符类会转换为按序排列、合并后的 Unicode 标量闭区间。补集只在标量值域内计算，
并跳过代理项区间 `U+D800..U+DFFF`。`\d`、`\w` 和 `\s` 使用 DSL 定义的
ASCII 字符集合，大写形式则取其补集。点号匹配除 LF 外的所有标量值。匹配过程不
做 Unicode 规范化、大小写折叠或区域设置处理。

## 4. Thompson NFA 与搜索

编译结果由指令数组和起始索引组成。指令可以消费字符集合、分支、跳转、断言输入
起止位置或标记匹配成功。分组只改变优先级，不捕获文本。有界重复会展开有限个
子程序副本；可选和无界重复使用分支及回路。有限重复上限为 255。

匹配器按 Unicode 标量扫描输入，并在每个输入位置加入模式起始态，因此默认执行
子串搜索。`^` 只在整个输入开头成功，`$` 只在输入末尾成功，两者都不是多行锚点。
空模式及其他可匹配空字符串的表达式可以在任意输入位置成功；要匹配整串，应同时
使用两个锚点。

匹配器以去重后的 NFA 状态集合推进，不进行回溯，因此选择和重复不会造成按路径
指数增长的回溯。字符区间成员检查使用二分查找。`max_search_steps` 计量一次搜索
处理的 NFA 状态数；`max_steps` 计量正则编译和匹配的累计显式工作。额度耗尽会
返回错误，不会返回部分匹配结果。

## 5. 资源限制

`RegexLimits::default()` 提供结构上限，`RegexExecutionLimits::default()` 提供运行期
额度。调用方可通过 `RegexOptions` 配置两者；这些默认值属于实现策略，不是 DSL 的
永久保证。

结构默认值：

- `max_regex_pattern_bytes`：单个解码模式最多 8,192 个 UTF-8 字节。
- `max_regex_total_pattern_bytes`：一个 Schema 最多累计 65,536 个尝试字节。
- `max_regex_count`：一个 Schema 最多保存 16 个正则程序。
- `max_regex_nesting_depth`：单个模式最多嵌套 64 层分组。
- `max_regex_instructions_per_pattern`：单个程序最多 4,096 条 NFA 指令。
- `max_regex_ranges_per_pattern`：单个程序最多使用 4,096 个字符区间。
- `max_regex_instructions_total`：一个 Schema 的程序最多共用 16,384 条指令。
- `max_regex_ranges_total`：一个 Schema 的程序最多共用 16,384 个字符区间。
- `max_regex_scratch_states`：一个工作区最多容纳 4,096 个状态槽位。

运行期默认值：

- `max_steps`：每次调用最多计量 67,108,864 个正则步骤。
- `max_search_steps`：单次搜索最多处理 67,108,864 个 NFA 状态。
- `max_workspace_bytes`：正则自有存活容量最多为 67,108,864 字节。

模式大小按 UTF-8 字节而不是 Unicode 字符数计算。语法无效的模式在进入字节统计后
仍会消耗尝试额度；公开的字节、区间和指令统计只计算已保存程序。量词展开会增加
指令和区间用量；单模式上限与 Schema 剩余的指令、区间上限共同约束解析和 NFA 构造。

正则预算会计量 AST、NFA 程序、字符集合和工作区的显式工作量及存活容量。预算自身
的固定 bookkeeping 不计入 `max_workspace_bytes`；该额度也不代表整个进程的内存
上限。逻辑资源超限、分配器拒绝、容量错误和内部不变量错误会保留为不同类别。

## 6. 工作区生命周期与错误

`RegexScratch` 保存活动态、下一态、遍历栈和去重标记。初始化时按程序指令数分配；
之后可在同一正则预算下、搜索开始前扩容，搜索期间不会扩容。扩容失败会保留具体
错误类别和已有工作区；仅发生分配失败不会耗尽正则预算。降低 scratch 状态上限后，
执行器会在复用前释放超出新上限的缓存。

语义层的 `RegexExecutor` 会在一次调用的多条约束间复用工作区。若下一次调用使用了
不同的正则预算，它会丢弃旧工作区，避免将旧容量计入新预算。对于空可选值，执行器
仍会先清理预算身份变化或超限的旧缓存，然后直接通过，不为新预算创建 scratch。

执行步数、搜索步数或计费工作区额度耗尽时，正则预算会记录失败并进入终止状态。
scratch 状态数等结构限制则在检测到超限的位置报告。上层 Schema 诊断会定位到注解，
也可能按常规规则附带源码上下文；底层错误只保留元数据，不复制解码模式或输入值。

## 7. 验证覆盖

`crates/typol-regex/tests/regex_behavior.rs` 覆盖搜索、锚点、量词、转义、ASCII 快捷
字符类、Unicode 标量、错误跨度及限制边界。正则 crate 的编译和预算测试也覆盖字符
区间计数与分配失败。

Schema 接入测试位于 `crates/typol-schema/tests`，包括
`regex_constraint_collection.rs`、`regex_constraint_resources.rs` 和
`regex_constraint_diagnostics.rs`。语义层的执行、预算和分配测试分别位于
`crates/typol-semantic/tests/regex_execution.rs`、`regex_budget.rs` 和
`regex_allocation.rs`。
