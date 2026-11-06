# `.tschem` 格式化 API 与 CLI `schema fmt` 实施方案

**状态：已实施。**

## 目标

为 `.tschem` 提供可供 Rust 调用的规范化格式化 API，并在 CLI 中新增 `schema fmt`，支持原地格式化和只读 `--check`。允许为无损格式化调整语法层内部或公开 API；不为兼容旧实现增加并行的 formatter 或模糊的兼容入口。

## 现状与依据

- `README.md`、`README_CN.md` 和 Schema DSL 说明 `.tschem` 是声明类型、默认值、注解和断言的语言；CLI 现有 Schema 命令只有 `schema check`。
- `typol-syntax` 负责流式 lexer/parser 和 AST。lexer 的 `skip_trivia` 会消费注释，`Token` 不提供关键字/符号的原始源码切片，字符串 token 保存解码值；AST 虽保留声明顺序、结构和部分字面量语法，但不持有完整源码。因此直接从 AST 重打印会丢失注释或改变字符串、数字的原始写法。
- `typol-config` 已有 `ConfigFormatter`、格式选项、完整结果对象和资源限制。CLI 的 `config fmt` 会先校验配置，支持 `--check`，并通过标准文件适配器原子替换文件。
- Schema DSL 允许 `//` 与非嵌套 `/* ... */` 注释、LF/CRLF、字符串转义和多种嵌套结构；formatter 必须与 parser 的词法规则一致。
- Schema parser 默认限制单源 1 MiB、嵌套深度 128；现有 schema 编译采用流式读取且不保存全量源文。格式化本身需要完整输入和输出，所以格式化入口须单独受限，不能改变普通编译路径的内存特性。

## 行为约定

1. Formatter 只要求 `.tschem` 语法有效，不解析导入文件，也不执行静态语义校验。这样格式化不依赖 import resolver，且可整理语义尚未完成的 Schema。
2. 除空白外，所有 token 与注释都按原字节、原顺序保留，包括字符串转义形式、数字进制/写法和注释文本；不做语义值重编码或声明重排。
3. 采用唯一规范风格：4 空格缩进；非空模块、结构体、枚举、匿名结构体载荷和复合默认值展开；空结构保持紧凑；列表元素单独成行并加尾逗号；类型、路径、注解及表达式按语法规则保持紧凑并规范分隔符/运算符空格；同层声明之间留一个空行。
4. 输入含 CRLF 时输出 CRLF，否则输出 LF；输出以选定的换行符结尾。行注释保持在原 token 邻接位置，块注释保持源码顺序。混合换行输入按“含 CRLF 即使用 CRLF”统一。
5. 格式化必须幂等。资源超限、分配失败、结构不一致和输出 sink 错误各自保留明确错误类别；任何失败都不能返回半成品成功结果。

## API 设计

将实现放在 `typol-syntax`，与 Schema DSL 的 parser 和 AST 同层，并通过 `typol` facade 暴露常用类型。

- 增加 `SchemaDocument<'source>`，由 parse-only 入口创建，绑定借用的原始 `&str` 与成功解析的 `SchemaAst`，调用方不能把不匹配的源码和 AST 拼接起来。保留现有流式 `SchemaParser::parse` 编译路径，不让普通 Schema 加载开始缓存完整源码。
- 增加 `SchemaFormatOptions`、`SchemaFormatError` / `SchemaFormatErrorKind`、`FormattedSchema` 和 `SchemaFormatter`。选项提供输出字节、formatter 工作区和嵌套深度上限，不提供会产生多种规范结果的风格开关。
- 入口形态：`SchemaParser::parse_document(...) -> SchemaDocument`，随后将文档所有权交给 `SchemaFormatter::format(document, options, out) -> FormattedSchema`。消费式入口在分配 token 表前释放仅用于语法验证的 AST，避免 AST 与 formatter 工作区同时驻留。`FormattedSchema` 提供 `as_str()` 和消费式 `write_to()`，只在完整格式化成功后交付。
- formatter 通过源码跨度和 AST 结构确定字段列表、变体列表、类型、表达式与复合默认值的布局；词法扫描保留注释和 token 原文。parser 与 formatter 共用词法识别规则，避免字符串中的注释符、CRLF 和转义处理出现分叉。
- 将现有配置专用原子写入契约提升为通用源码写入 API：在 `typol-core` 定义 `SourceWriter`、`SourceExpectation`、`SourceRevision` 和 `SourceCommitError`；删除配置专用的重复命名，不保留旧名称别名。`typol-config::ConfigDocument::write_source` 与 CLI 的 `config fmt` 迁移到通用 trait；`typol-std` 将 `FileConfigSourceWriter` 改为通用 `FileSourceWriter`，继续承载现有跨平台临时文件、flush/sync、原子替换与错误分类实现。
- Schema CLI 通过同一个 `SourceWriter::replace_atomically` 写回格式化字节。文件系统路径使用 `ReplaceAny`：现有文件适配器无法把“字节快照比较”和路径替换做成一个原子操作；自定义后端仍可按需要支持 `ExactSnapshot`。这样保留完整、可替换的 host/no_std 写入边界，而不让 Schema formatter 依赖 `typol-config`。
- 在 `typol` 根及 `typol::schema` 导出 Schema 格式化 API。为 CLI 把 `SyntaxErrors` 接入统一的本地化错误报告，增加明确的 Schema parse failure stage，并携带源路径、位置和有界 excerpts；不要退回未本地化的 `Debug` / `Display` 输出。若无损实现要求调整 parser/token 内部表示，直接调整，不新增会丢注释的备用 formatter。

## CLI 设计

新增命令：

```text
typol schema fmt [--check] <schema.tschem>
```

- 默认模式先有界读取 UTF-8 源文件，调用 parse-only formatter，再通过通用 `SourceWriter` trait 以原子替换语义写回同一路径。不要用会先截断目标文件的普通写入。
- `--check` 只比较格式化结果与原文，不写文件；已规范时退出码 `0`，需要格式化时退出码 `1` 并向 stderr 输出本地化提示。
- 语法诊断经新增的 Schema parse failure adapter 接入现有双语诊断渲染路径；I/O、编码、资源和原子替换错误归入宿主操作失败，命令行用法错误仍为退出码 `2`。
- `schema check` 继续执行完整 Schema 编译；`config fmt` 继续处理 `.tconf`。两者保留各自语义，不增加顶层自动猜测文件类型的 `fmt` 命令。
- 有界读取使用 Schema 单源字节上限；CLI help、英文/简体中文 catalog、重复/无效参数处理和 `--` 路径行为同步更新。`config fmt` 与 `schema fmt` 共用 `FileSourceWriter` 和 `SourceExpectation::ReplaceAny` 路径，避免复制平台代码。

## 实施步骤

1. 在 Schema DSL 与实现文档中固定 formatter 的唯一风格、注释保留和语法/语义边界，准备覆盖每类声明、表达式、默认值及注释布局的规范输入输出样例。
2. 在 `typol-syntax` 增加 `SchemaDocument` parse-only 入口及源码跨度驱动的 formatter。重构词法识别以支持可靠注释扫描；保持编译器现有流式 parse 路径，不持有源码副本。所有扫描、栈和输出受显式限制及 execution budget 约束。
3. 将格式化类型从 `typol` facade 的根 API 和 `schema` 模块导出，并补齐 API 文档示例。
4. 将 Config 原子写入 trait、错误和 revision 类型泛化到 `typol-core`，迁移 `ConfigDocument::write_source`、现有 `config fmt`、`typol-std` 文件 writer 及其调用方；然后扩展 CLI 参数解析、命令分发、错误渲染和英/中文 help/catalog，新增 `schema fmt` 与 `--check`。
5. 更新英文/中文 CLI 指南、Schema implementation 文档、两个 README 的命令说明，以及 `examples/cli/CLI.md` 示例。

## 验收覆盖

- Formatter 单测：全部 Schema 声明种类、嵌套模块与类型、字段/变体/参数列表、数组与对象默认值、注解和断言表达式、空/非空结构、行/块注释、字符串内注释符、转义字符串、十进制/十六进制数字、LF/CRLF、混合换行、Unicode 注释、无末尾换行、幂等性和语法错误。
- 不变量检查：格式化前后 token/注释字节序列一致；再次解析成功；AST 结构及所有字面量文本保持；源码和结果均不越过配置额度；资源/分配失败不产生成功的部分结果。
- 写入 trait 覆盖：成功原子替换、目标已存在/快照不符/资源超限、临时文件写入/flush/sync/rename 故障、commit 后目录同步失败分类；确保迁移后 Config 和 Schema 调用方共享相同协议。
- CLI 集成覆盖：原地格式化、再次运行幂等、干净与脏 `--check`、`--check` 文件字节不变、语法错误双语诊断、无效选项/路径、超限文件、文件替换失败及退出码。
- 最终运行 `cargo fmt --all -- --check`、`cargo cop --all-targets --all-features --workspace`、`cargo clippy --all-targets --all-features --workspace` 和全工作区测试。

## 预期涉及文件

- 核心：`typol-core` 通用源码写入 API 及导出；`crates/typol-syntax/src/lib.rs`、`parser.rs`、`formatter.rs` 及 formatter 扫描模块。
- Config/host 迁移：`crates/typol-config/src/document.rs`、`lib.rs`、`crates/typol-std/src/filesystem/source_writer.rs`、`filesystem.rs` 及相关测试。
- Facade 和统一诊断：`crates/typol/Cargo.toml`、`crates/typol/src/lib.rs`、`crates/typol/src/api.rs`、`crates/typol/src/error.rs`、`crates/typol/src/loading_errors.rs`、必要时 `crates/typol-std/src/diagnostics/`。
- CLI 与本地化：`crates/typol-cli/src/arguments.rs`、`commands.rs`、`crates/typol-cli/tests/cli.rs`、`crates/typol-i18n/schema.u18n`、`locales/en.u18n`、`locales/zh-CN.u18n`。
- 文档：`README.md`、`README_CN.md`、英文/中文 CLI guide 和 Schema implementation、`examples/cli/CLI.md`。
