# Typol

[English](README.md) · [简体中文文档站](https://shaogme.github.io/typol/zh-cn/)

Typol 是面向 Rust 的 Schema 驱动配置库。`.tschem` 文件声明配置类型与规则，`.tconf` 文件提供具体值。Typol 编译 Schema、解析并校验配置，然后可通过 `#[derive(Typol)]` 将结果转换为匹配的 Rust 结构体和枚举。

工作区要求 Rust 1.85 或更高版本。以下示例使用 `typol` facade crate。

## 从这里开始

应用需要读取文件或访问进程环境时，请启用 `std` feature：

```toml
[dependencies]
typol = { version = "0.1", features = ["std"] }
```

仓库包含一个完整的文件加载示例：

```sh
cargo run -p typol-showcase
```

想按步骤构建应用，请先阅读[快速上手](docs/src/content/docs/zh-cn/guides/quickstart.md)。该教程依次讲解 Schema、配置、Rust 类型映射，以及连接这些步骤的方法。

## 数据模型

Schema 描述类型和校验规则，配置文本提供入口类型的一个实例。

```tschem
struct Endpoint {
    host: string,
    port: u16 = 8080,
}

entry struct ServiceConfig {
    name: string,
    endpoint: Endpoint,
    tags: [string],
    note: option<string>,
}
```

`Endpoint` 和 `ServiceConfig` 是 Schema 结构体。`entry` 选择配置根类型。应用可以定义同名且字段匹配的 Rust 结构体；`#[derive(Typol)]` 会生成它们的转换映射。Rust 方法和字段可见性仍由应用控制。

读取配置时，Schema 默认值会补齐省略字段。若要在不声明或绑定 Rust 类型的情况下，根据这些默认值生成完整 `.tconf`，可调用 `LoadedSchema::default_config_to_string` 或 `write_default_config`。每个必填字段都必须有显式 Schema 默认值；系统不会根据 Rust `Default` 或空容器约定推测值。详见[快速上手](docs/src/content/docs/zh-cn/guides/quickstart.md)和[进阶教程](docs/src/content/docs/zh-cn/guides/advanced.md)。

常见 Schema 类型与 Rust 类型的对应关系：

| Schema 类型 | Rust 类型 |
| --- | --- |
| `bool`、`string`、整数和浮点标量 | `bool`、`String`、相同位宽的整数或浮点类型 |
| `option<T>` | `Option<T>` |
| `[T]` | `Vec<T>` |
| `[T; N]` | `[T; N]` |
| `map<string, T>` | 启用 `std` 时为 `HashMap<String, T>`；不启用时为 `OrderedMap<T>` |
| Schema `struct` / `enum` | 使用 `#[derive(Typol)]` 的 Rust `struct` / `enum` |

Schema 和 Rust 声明中的字段名及类型必须匹配。[快速上手](docs/src/content/docs/zh-cn/guides/quickstart.md)包含私有字段和访问器方法的示例。

## Rust API 的调用关系

主要流程分为三个阶段：

| 阶段 | 类型或方法 | 作用 |
| --- | --- | --- |
| 编译 | `SchemaLoader::compile_text` 或 `compile_file` | 校验 Schema 并生成 `LoadedSchema`。文件加载需要 `std`。 |
| 绑定 | `LoadedSchema::bind::<T>()` | 检查 Rust 类型 `T` 与 Schema 入口是否一致，并生成可复用的 `BoundSchema<T>`。 |
| 读取 | `BoundSchema::read` 或 `read_file` | 解析、应用默认值、校验，并返回 `T`。每次读取都显式接收 `Environment` 提供器。 |
| 生成默认配置 | `LoadedSchema::default_config_to_string` 或 `write_default_config` | 物化 Schema 默认值和 option 的 `none`，校验完整值，并在不绑定 Rust 类型或接收环境提供器的情况下编码 `.tconf`。 |

`SchemaLoader::new(options)` 是创建 loader 的关联函数。表中的其他入口是作用于 loader、已加载 Schema 或绑定对象的方法。`LoadedSchema` 及其 binding 可以重复使用，以读取多份配置。

可能失败的操作接收 OPEO `Out` 错误上下文并返回 OPEO 结果。快速上手教程展示如何通过 `opeo::ErrSlot::try_call` 捕获并报告 `TypolError`。

配置输入同样是显式提供的：`BoundSchema::read` 接收包含 `Read` 源的 `ReadRequest`，例如 `SliceSource`。启用 `std` 后，可用 `read_file` 和 `read_io` 适配宿主文件和 reader。每次读取还会接收 `Environment` provider；Typol 不会隐式查询进程环境变量。

## 工作区结构

源码按职责拆分。普通应用使用的 API 位于 `crates/typol`；大多数用户无需直接依赖底层 crate。

| Crate | 职责 |
| --- | --- |
| `typol` | 公共 facade、加载、Rust 绑定、编解码及可选 Config IR |
| `typol-macros` | 实现 `#[derive(Typol)]` |
| `typol-syntax` | Lexer、Schema parser 和语法树 |
| `typol-model` | Schema 运算符和内建类型的共享定义 |
| `typol-schema` | 导入、名称和类型解析、静态检查及 Schema 编译 |
| `typol-semantic` | 编译后的 Schema 模型、约束、表达式程序及求值 |
| `typol-config` | Schema 指导的配置解析、校验、格式化和保留源文本的文档 |
| `typol-regex` | 有界正则编译器和匹配器 |
| `typol-core` | 共享值、源码、诊断和执行预算 |
| `typol-policy` | 共享资源限制策略 |
| `typol-report` | 本地化诊断报告格式化 |
| `typol-std` | 可选的文件、进程环境和诊断宿主适配器 |
| `typol-i18n` | 英文和简体中文消息目录 |
| `typol-cli` | Schema 与配置检查、`.tschem` 和 `.tconf` 格式化，以及 Config IR 导出 |

## 按需阅读

| 如果你想…… | 请阅读 |
| --- | --- |
| 构建类型化配置加载器，了解核心类型和方法 | [快速上手](docs/src/content/docs/zh-cn/guides/quickstart.md) |
| 使用导入 Schema、跨字段检查、Serde 或 Config IR | [进阶教程](docs/src/content/docs/zh-cn/guides/advanced.md) |
| 不依赖标准库使用 Typol | [no_std 指南](docs/src/content/docs/zh-cn/guides/no_std.md) |
| 从命令行检查或格式化文件，或导出 Config IR | [CLI 使用指南](docs/src/content/docs/zh-cn/guides/cli.md) |
| 查询 Schema 或配置语法 | [Schema DSL](docs/src/content/docs/zh-cn/specification/schema-dsl.md) · [Config DSL](docs/src/content/docs/zh-cn/specification/config-dsl.md) |
| 了解实现细节 | [Schema 实现](docs/src/content/docs/zh-cn/specification/schema-implementation.md) · [Config 实现](docs/src/content/docs/zh-cn/specification/config-implementation.md) |

文档站提供英文和简体中文版本。DSL 与格式规范定义语言行为；实现文档说明当前各 crate 如何实现这些规则。

## Feature 与构建

`typol` 默认不启用任何 feature。请按应用需要启用能力：

| Feature | 提供内容 |
| --- | --- |
| `std` | 文件和 reader 输入、基于文件的 Schema 加载、进程环境访问和宿主诊断渲染 |
| `serde` | 带 Schema 上下文的 Serde 读写 |
| `hashbrown` | `hashbrown::HashMap<String, T>` 绑定 |
| `btree-map` | `BTreeMap<String, T>` 绑定 |
| `config-ir` | Config IR v1 JSON 序列化；同时启用 `serde` |

不启用 `std` 时，可在内存中提供 Schema 和配置源，并在每次读取时传入 `Environment` 实现。拥有型值需要 allocator。

在工作区根目录执行：

```sh
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

在 `docs` 目录中运行 `npm run build` 可构建文档站。Nix 打包说明见 [Nix 指南](docs/src/content/docs/zh-cn/guides/nix.md)。项目采用 Apache-2.0 或 MIT 双许可证，详见 [LICENSE-APACHE](LICENSE-APACHE) 和 [LICENSE-MIT](LICENSE-MIT)。
