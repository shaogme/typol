# Typol

[English](README.md)

Typol 是面向 Rust 的 Schema 驱动配置库。它编译 `.tschem` Schema，按编译后的类型解析 `.tconf` 配置值，校验约束和断言，并可通过 `#[derive(Typol)]` 将结果绑定到 Rust 类型。

工作区要求 Rust 1.85 或更高版本。以下示例使用 `typol` facade crate；底层 crate 也可以独立使用。

Facade 默认使用 `no_std`，拥有型值使用 `alloc`。内存加载器接受 Schema 文本或 resolver；配置读取接受文本或调用方提供的 `ByteSource`，并要求显式传入 `Environment` provider。`serde` 和 `config-ir` feature 均可在不启用 `std` 时使用；Config IR 序列化返回由 `alloc` 支持的 `Vec<u8>`。启用 `std` 后可使用文件和 reader 输入、进程环境访问及宿主诊断渲染。

要读取进程环境变量，请使用 `ProcessEnvironment::new(ProcessEnvironmentLimits::default())` 创建 provider，再将它传给 `BoundSchema::read` 或 `LoadedSchema::read_value`。这两个类型由 facade 根级导出；环境额度由 provider 自身携带，应用可以按读取策略选择额度。

## 快速开始

此示例通过 `std` 宿主适配器在内存中编译 Schema，将其绑定到 Rust 类型，解析配置文本，应用 Schema 默认值，并通过显式 provider 解析已授权的环境表达式。要使用这些 API，请在 `typol` 依赖中启用 `std`。

```toml
[dependencies]
typol = { version = "0.1", features = ["std"] }
```

```rust
use typol::{
    opeo::ErrSlot, ConfigInput, EmptyEnvironment, SchemaLoader, Typol, TypolError, TypolOptions,
};

#[derive(Debug, PartialEq, Typol)]
struct ServiceConfig {
    name: String,
    port: u16,
    tags: Vec<String>,
    enabled: bool,
    endpoint: String,
}

let mut errors = ErrSlot::<TypolError>::new();
let result = errors.try_call(|mut out| {
    let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
        b"readme",
        r#"
entry struct ServiceConfig {
    name: string @length(1, 64),
    port: u16 = 8080,
    tags: [string] @size(min = 1, max = 8),
    enabled: bool = true,
    endpoint: string @env("APP_ENDPOINT") = "127.0.0.1:8080",
}
"#,
        out.reborrow(),
    )?;
    let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
    let mut environment = EmptyEnvironment;
    let config = binding.read(
        ConfigInput::text(
            r#"{ name: "edge", tags: ["api"], endpoint: env("APP_ENDPOINT") }"#,
        ),
        &mut environment,
        out,
    )?;

    assert_eq!(
        config,
        ServiceConfig {
            name: "edge".into(),
            port: 8080,
            tags: vec!["api".into()],
            enabled: true,
            endpoint: "127.0.0.1:8080".into(),
        }
    );
    Ok(())
});

assert!(result.is_ok());
```

Facade 的可失败操作接收 `Out` 错误上下文，并返回 OPEO 结果。`ErrSlot::try_call` 是执行这类操作并取得错误的一种方式。`compile_text` 用于不含导入的 Schema。对于含 `extern mod` 的 Schema，请使用 `SchemaLoader::compile_file`，或通过 `SchemaResolver` 调用较底层的编译器。

## 功能

- **Schema DSL：** 结构体、包含无载荷/单值/结构体载荷的枚举、类型别名、嵌套模块、默认值、类型化断言和校验注解。
- **Config DSL：** 类型化标量、可选值、数组、定长数组、字符串键 map、枚举值、注释，以及显式 `env("NAME")` 表达式。
- **校验：** 标量和集合约束、预编译正则、`@all` 谓词，以及对完整配置值执行的 `@assert` 检查。
- **Rust 集成：** `#[derive(Typol)]` 为 Rust 结构体和枚举生成类型描述；绑定时会对照已编译入口类型检查形态。绑定后的 Schema 可重复读取和写入多个值。
- **宿主适配器：** 启用 `std` feature 后，`typol-std` 提供文件加载、reader 输入、进程环境访问和安全诊断渲染。核心 parser 和 compiler 不直接访问文件系统或进程环境。
- **类型化 JSON 导出：** 启用 `config-ir` 后可导出 Config IR v1。该格式保留 Schema 类型信息并提供确定性排序。`secret: true` 标记敏感节点，但不会移除或加密其值。
- **资源边界：** 配置解析、Schema 编译、校验、正则、诊断和 IR 序列化均使用可配置额度；资源失败会显式报告，不返回部分结果。

正则表达式遵循 Typol 文档规定的子集和搜索语义。需要整个字符串都匹配时，请使用 `^` 和 `$`。`@secret` 控制诊断和安全展示；应用中的值及序列化输出仍包含真实数据。

## 手写 `Typol` 实现

`LoadedSchema::bind` 和 `TypedSchema::bind` 用于内建类型，以及所有嵌套字段和枚举载荷也带有隐藏 `BudgetedTypol` 标记的派生类型。应通过 `SchemaLoader` 加载 Schema、绑定 Rust 类型，再使用绑定读取配置。派生类型包含手写实现时，请添加 `#[typol(trusted)]`，再通过 `LoadedSchema::bind_trusted` 或 `TypedSchema::bind_trusted` 使用；返回的 `TrustedBoundSchema` 或 `TrustedTypedSchema` 会在 Rust 类型中保留此选择。

库额度覆盖库管理的转换节点、深度、工作区、输出和经上下文分发的目标存储，不是操作系统 CPU、进程内存或实时性上限。受信任回调仍可自行分配、阻塞或执行任意代码，其返回的 Rust 值也不会再次校验。普通结构体和枚举映射建议使用 derive；只有应用已审查并信任的特殊映射才保留手写实现。不可信代码需要通过独立进程或 Wasm 边界运行。即使绑定满足 `BudgetedTypol`，调用方提供的 Serde 实现仍属于独立的用户代码信任边界。

## 工作区 crate

| Crate | 职责 |
| --- | --- |
| `typol` | 公共 facade、Rust 类型绑定、加载、编解码及可选 Config IR |
| `typol-syntax` | 通用 lexer、token 及 Schema parser/AST |
| `typol-schema` | Schema 导入、名称和类型解析、静态检查及编译 |
| `typol-semantic` | 不可变编译 Schema 模型、约束视图、表达式程序及求值 |
| `typol-config` | 流式、Schema 指导的配置解析与校验 |
| `typol-regex` | 有界正则编译器和匹配器 |
| `typol-core` | 共享值类型、额度、预算、源跨度和诊断类型 |
| `typol-std` | 文件系统、进程环境及诊断宿主适配器 |
| `typol-i18n` | 由 `u18n` 编译的英文和简体中文 CLI 翻译 |
| `typol-macros` | `#[derive(Typol)]` 的实现 |
| `typol-cli` | Schema 检查、配置检查和 Config IR 导出命令 |

## 文档

[Typol 文档站](docs/src/content/docs/zh-cn/index.md) 使用 Starlight 构建，提供 English 和简体中文版本。DSL 与格式规范定义语法和语义，是规范性依据；实现文档说明当前各 crate 如何实现这些约定。

| 文档 | 内容 |
| --- | --- |
| [快速上手](docs/src/content/docs/zh-cn/guides/quickstart.md) | Schema、Config 与 Rust 类型绑定入门教程 |
| [no_std 指南](docs/src/content/docs/zh-cn/guides/no_std.md) | 不启用 `std` 时的内存加载、alloc 和显式 provider |
| [CLI 使用指南](docs/src/content/docs/zh-cn/guides/cli.md) | 构建并使用 CLI 检查文件和导出 Config IR |
| [进阶教程](docs/src/content/docs/zh-cn/guides/advanced.md) | Schema 导入、校验、Rust 绑定、Serde、资源上限与 Config IR |
| [Config DSL](docs/src/content/docs/zh-cn/specification/config-dsl.md) | `.tconf` 语法、值、默认值、可选值和环境表达式 |
| [Schema DSL](docs/src/content/docs/zh-cn/specification/schema-dsl.md) | `.tschem` 语法、类型、注解、断言和正则子集 |
| [Config 实现](docs/src/content/docs/zh-cn/specification/config-implementation.md) | 流式解析、校验、环境处理和诊断 |
| [Schema 实现](docs/src/content/docs/zh-cn/specification/schema-implementation.md) | 导入、静态编译、编译记录和运行期使用 |
| [Config IR v1](docs/src/content/docs/zh-cn/specification/config-ir.md) | 类型化 JSON 导出格式、排序、秘密标记和额度 |
| [正则实现](docs/src/content/docs/zh-cn/specification/regex-implementation.md) | 正则编译、NFA 搜索、工作区和资源边界 |
| [完整示例](examples/showcase/README.md) | 基于文件的 Schema 与配置端到端示例 |
| [CLI 示例](examples/cli/CLI.md) | CLI 检查和 Config IR 导出示例 |

从仓库根目录本地运行文档站：

```sh
cd docs
npm install
npm run dev
```

在 `docs` 目录运行 `npm run build` 可生成生产站点。

## 构建与测试

在工作区根目录运行：

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

运行完整示例或通过 CLI 检查 Schema：

```sh
cargo run -p typol-showcase
cargo run -p typol-cli -- schema check examples/cli/cli_demo.tschem
cargo run -p typol-cli -- config check --schema examples/cli/cli_demo.tschem --environment empty examples/cli/cli_demo.tconf
cargo run -p typol-cli -- --language zh-CN --help
```

CLI 可通过 `--language en|zh-CN` 选择英文或简体中文。省略该选项时，会依次检查
`TYPOL_LANG`、`LC_ALL`、`LC_MESSAGES` 和 `LANG`，最后默认使用英文。帮助、命令行错误和
CLI 操作错误由 `u18n` 生成的目录提供翻译；Typol 库生成的详细 Schema 和配置诊断目前仍以英文显示。
检测会将 `en_US.UTF-8` 等 POSIX 名称规范化为 BCP-47 tag；区域英文 tag 使用生成目录的回退，
所有 `zh-*` tag 都选择唯一的简体中文目录。目录 tag 回退只负责选择已编译目录，不会补充缺失的消息翻译。
翻译错误会沿调用方的错误通道返回，不会被固定文案替代。

项目采用 Apache-2.0 或 MIT 双许可证，详见 [LICENSE-APACHE](LICENSE-APACHE) 和 [LICENSE-MIT](LICENSE-MIT)。
