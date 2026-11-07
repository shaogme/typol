---
title: "正则实现说明"
description: "说明 Typol 支持的正则子集、搜索语义、资源限制与错误处理。"
sidebar:
  order: 6
---

## 本页要点

:::tip[适用场景]
排查 `@regex` 编译、搜索匹配行为或正则专用资源错误时，请查阅本页。
:::

| 主题 | 关键规则 |
| --- | --- |
| 匹配方式 | 默认在任意位置搜索；要匹配整串，需要同时使用 `^` 和 `$`。 |
| 执行方式 | 模式编译为 Thompson NFA 并以无回溯方式执行；宿主正则库的扩展语法不受支持。 |
| 资源边界 | 编译和匹配受显式额度限制；额度耗尽会返回结构化错误，不会返回部分匹配结果。 |

本文说明 Typol 当前对 Schema `@regex` 注解的实现，包括编译表示、搜索执行、资源限制、诊断和运行期工作区。正则语言及其可观察匹配语义以[Schema DSL 规范 §5.1](../schema-dsl/#51-正则语法与匹配)为准；本文记录实现方式和实现边界，不扩展 DSL。

## 1. 实现范围

`typol-regex` 是独立的 `no_std + alloc` 正则编译与执行 crate。它不访问 Schema 文件、配置环境或宿主资源。编译与匹配入口必须接收 `RegexExecutionBudget`；Schema 和配置调用分别从 `RegexOptions` 为每次编译或运行创建该预算。通用 `ExecutionBudget` 只计量非正则工作。

主要源码位置：

| 职责 | 实现位置 |
| --- | --- |
| 正则语法解析及 AST | `crates/typol-regex/src/parser.rs`、`parser/character_class.rs` |
| Unicode 标量字符集合 | `crates/typol-regex/src/char_class.rs` |
| Thompson NFA 编译及不可变程序 | `crates/typol-regex/src/compiler.rs` |
| NFA 搜索及复用工作区 | `crates/typol-regex/src/matcher.rs` |
| 结构化错误和模式跨度 | `crates/typol-regex/src/error.rs` |
| Schema 注解编译和正则集合 | `crates/typol-schema/src/regex.rs`、`crates/typol-semantic/src/regex.rs` |
| 语义执行器与约束检查 | `crates/typol-semantic/src/engine/regex.rs`、`crates/typol-semantic/src/engine/rules.rs` |
| 正则资源默认值 | `crates/typol-policy/src/lib.rs` |

## 2. Schema 编译流程

Schema 编译器仅接受目标为 `string` 或 `option<string>` 的 `@regex`。可选值为 `none` 时跳过匹配；有值时按字符串内容执行搜索。

注解中的字符串先由 Schema 词法器按 DSL 规则解码，然后将解码后的 UTF-8 文本交给 `RegexCompiler`。正则编译器先按 Unicode 标量读取模式并构造内部 AST，再生成自有数据的 Thompson NFA。成功后，NFA 与注解的 `SourceSpan` 一起存入 `CompiledRegexConstraints`。运行期不重新解析或编译模式。

Schema 编译会累计每个有效目标 `@regex` 的尝试模式字节数，语法无效的模式也会消耗此资源额度；公开 `RegexCounts` 只统计成功保存的程序。`max_regex_count` 限制成功保存的程序数。Schema 剩余指令数和区间数会在解析及 NFA 构造期间传入正则编译器，避免先完整构造超额程序再丢弃。

Schema 编译器按注解源跨度索引已编译程序。语法错误映射为 `REGEX_SYNTAX_ERROR`，诊断定位在 `@regex` 注解；资源超限映射为资源诊断，分配失败映射为分配诊断。底层正则错误只携带错误类别和模式内跨度，不把模式文本写入错误对象或调试信息。

## 3. 解析与字符集合

解析器使用显式帧处理分组，避免由分组嵌套导致宿主调用栈随输入深度增长。模式内语法错误的 `PatternSpan` 使用 UTF-8 字节偏移，起止位置为半开区间；这与 DSL 字符串解码后的模式偏移相对应，不是 Schema 源文件中的字节跨度。

解析器实现 Schema DSL 规范列出的字面量、分组、选择、连接、锚点、量词、字符类和转义。它拒绝空选择分支、空字符类、反向范围、对非字面字符类端点使用范围、重复量词及未支持转义。解析器不会接受宿主正则库的额外语法，例如环视、反向引用或 Unicode 属性转义。

字符类编译为按起点排序且合并相邻区间的 Unicode 标量闭区间集合。补集只在 Unicode 标量值域中计算，并跳过代理项区间 `U+D800..U+DFFF`。`\d`、`\w`、`\s` 使用 DSL 定义的 ASCII 集合；大写形式在标量值域取补集。`.` 编译为除 LF 外的标量集合。匹配不做规范化、大小写折叠或区域设置处理。

## 4. Thompson NFA 与搜索语义

编译结果由指令数组和起始指令索引组成。指令种类为消费一个字符集合、分支、无条件跳转、起始/结束断言和匹配终态。分组仅影响解析优先级，不捕获文本。量词通过复制有限次子程序及分支/回路指令展开；有限重复上限为 255。

执行器逐 Unicode 标量扫描输入。在每个位置将模式起始态加入活动状态集合，因此实现的是搜索匹配，而非默认整串匹配。`^` 只在输入开头成功，`$` 只在输入结束成功；它们不改变为多行锚点。空模式和可匹配空字符串的表达式可以在输入任意位置成功。需要整串匹配时，调用者在模式中使用 `^` 和 `$`。

NFA 状态以去重集合推进，不进行回溯，因此选择和重复不会引入按路径指数展开的回溯搜索。字符集合查询使用排序区间二分查找。每次匹配受独立 `max_search_steps` 限制，计量该次搜索处理的 NFA 状态数；累计正则编译和匹配工作共享 `max_steps`。任一额度耗尽都会返回结构化资源错误，不返回部分匹配结果。

## 5. 资源限制

`RegexLimits::default()` 提供结构限制；调用方通过 `RegexExecutionLimits` 单独配置执行额度，再由 `RegexOptions` 组合两者。以下默认值是实现配置，不是 DSL 的永久语义承诺。

| 限制 | 默认值 | 作用范围 |
| --- | ---: | --- |
| `max_regex_pattern_bytes` | 8,192 | 单个解码后模式的 UTF-8 字节数 |
| `max_regex_total_pattern_bytes` | 65,536 | 一个 Schema 内有效目标注解的尝试模式字节数累计，包括语法无效模式 |
| `max_regex_count` | 16 | 一个 Schema 内成功保存的正则程序数 |
| `max_regex_nesting_depth` | 64 | 单个模式的分组嵌套深度 |
| `max_regex_instructions_per_pattern` | 4,096 | 单个编译程序的 NFA 指令数 |
| `max_regex_ranges_per_pattern` | 4,096 | 单个程序持有的字符区间数，按量词展开后的程序累计 |
| `max_regex_instructions_total` | 16,384 | 一个 Schema 中所有程序的指令数累计 |
| `max_regex_ranges_total` | 16,384 | 一个 Schema 中所有程序的字符区间数累计 |
| `max_regex_scratch_states` | 4,096 | 单次执行工作区的状态槽位数 |

`RegexExecutionLimits::default()` 的执行额度如下：

| 限制 | 默认值 | 作用范围 |
| --- | ---: | --- |
| `max_steps` | 67,108,864 | 一次调用内由正则编译器和匹配器显式计量的累计工作 |
| `max_search_steps` | 67,108,864 | 单次搜索处理的 NFA 状态数 |
| `max_workspace_bytes` | 67,108,864 | 一次调用中正则自有的存活容量 |

模式字节数按 UTF-8 字节而不是字符数统计。尝试模式字节数用于限制编译消耗，即使语法错误也不回退；公开模式字节、范围和指令统计只包含最终保存的程序，因此无效模式不会污染产物统计。量词展开会增加成功程序的指令和范围占用。Schema 级剩余额度与单程序限制共同约束解析和程序构造，任一超限都会使 Schema 编译或运行失败。一次 Schema 编译中的多个模式以及默认值正则检查共用一个正则预算；一次配置解析或外来值校验也在调用范围内共用一个正则预算，即使复用了调用方的通用预算。仅规范化外来值的操作不会创建或消耗正则预算。

编译 AST、NFA、字符集合和工作区所需的容器通过可失败分配向量计入正则预算的步数和存活工作区。正则预算的固定 bookkeeping 不计入其工作区上限。容量超限返回资源错误；分配器拒绝分配时返回分配错误。错误不包含模式或输入内容。

## 6. 工作区生命周期与错误

`RegexScratch` 为一次 NFA 搜索持有活动态、下一态、遍历栈和去重标记缓冲区。初次使用时按程序指令数创建；之后在匹配开始前按当前正则预算扩容，匹配过程不扩容。增长失败保留具体错误分类，且不会主动丢弃已有工作区。分配失败不会耗尽正则预算，原工作区仍可继续使用；正则工作区不足会令该预算进入终止状态。降低 scratch 上限时，超过上限的缓存会先释放。

语义层的 `RegexExecutor` 在多条约束间复用并增长工作区。绑定新的正则预算调用时会丢弃旧工作区，避免把先前调用占用的容量错误计入新调用；空可选值会先清理预算身份已变化或超过当前上限的旧缓存，再直接视为通过，不会为新预算创建 scratch。

正则相关错误分为语法错误、资源超限和分配失败。匹配错误只报告资源超限；Schema 诊断层将底层错误类别转换为稳定的诊断代码和注解位置。正则语法错误对象另含模式内 `PatternSpan`，供需要细粒度定位的直接 `typol-regex` 调用者使用。

## 7. 验证覆盖

实现行为由 `crates/typol-regex/tests/regex_behavior.rs` 覆盖，其中包括搜索/锚点、选择和重复、转义、ASCII 快捷类、Unicode 标量与代理项间隙、错误跨度和限制边界；编译器单元测试覆盖点号在三个及两个可用区间下的边界。`compilation_budget.rs`、`compilation_allocation.rs`、`allocation_failure.rs` 覆盖共享预算、有限栈、分配失败及释放行为。Schema 层的 `crates/typol-schema/tests/regex_constraint_collection.rs` 和 `regex_constraint_resources.rs` 覆盖导入约束、尝试额度、产物统计及诊断优先级；语义层 `crates/typol-semantic/tests/regex_execution.rs`、`regex_budget.rs` 和 `regex_allocation.rs` 覆盖工作区增长、增长失败、预算切换和上限回收。
