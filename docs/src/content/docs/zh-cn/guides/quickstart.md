---
title: 快速上手
description: 了解 Typol 的 Schema 类型、Rust 映射、核心方法和配置读取流程。
sidebar:
  order: 1
---

本教程会构建一个读取 Schema 和配置文件的小型 Rust 程序，并介绍其中的三部分：

1. `.tschem` Schema 声明数据类型和校验规则。
2. `.tconf` 文件为 Schema 的入口类型提供具体值。
3. Rust 结构体和枚举在应用中表示校验后的值。

常见调用顺序是 `SchemaLoader → LoadedSchema → BoundSchema<T> → T`。每一步职责不同：编译、绑定，然后读取。

## 创建项目

Typol 要求 Rust 1.85 或更高版本。文件加载和进程环境适配器需要启用 `std` feature：

```toml
[dependencies]
typol = { version = "0.1", features = ["std"] }
```

运行 `cargo new service-app` 创建项目，再添加这些文件：

```text
service-app/
├── Cargo.toml
├── service.tschem
├── service.tconf
└── src/
    └── main.rs
```

不使用标准库的应用请阅读 [no_std 指南](../no_std/)。

## 1. 声明 Schema 类型

将以下内容保存到 `service.tschem`：

```tschem
struct Endpoint {
    host: string @host,
    port: u16 @range(1, 65535) = 8080,
}

entry struct ServiceConfig {
    service_name: string @length(1, 64),
    endpoint: Endpoint,
    tags: [string] @size(min = 1, max = 8) = ["api"],
    enabled: bool = true,
    note: option<string>,
}
```

Schema 中的 `struct` 将命名字段组合为一个类型。每个字段都声明类型，也可附加注解或默认值。`entry` 标记唯一的配置根结构体。

这里 `ServiceConfig` 是入口类型，`endpoint` 字段使用另一个已声明类型。`service_name` 是必填字段；`endpoint.port`、`tags` 和 `enabled` 有默认值；`note` 是可选字段。Schema 默认值与配置中显式提供的值一样会经过校验。

常见 Schema 类型：

| Schema 类型 | 含义 |
| --- | --- |
| `bool`、`string`、`i8`…`i128`、`u8`…`u128`、`f32`、`f64` | 布尔、字符串、整数和有限浮点标量 |
| `option<T>` | 可选的 `T`；空值写作 `none` |
| `[T]` | 长度可变的数组 |
| `[T; N]` | 长度恰为 `N` 的数组 |
| `map<string, T>` | 字符串键 map |
| `struct`、`enum` | 用户声明的记录类型和选择类型 |

`@range`、`@length` 和 `@size` 等注解可增加校验规则。支持的类型和注解见 [Schema DSL 参考](../../specification/schema-dsl/)。

## 2. 提供配置值

将以下内容保存到 `service.tconf`：

```text
{
    service_name: "edge",
    endpoint: {
        host: "edge.internal",
        port: 8443,
    },
    note: """
        Drains active requests
        before restart.
        """,
}
```

配置的键和值遵循 Schema。缺失字段采用默认值；省略的可选字段变为 `none`。显式提供的值仍须符合声明类型和约束。`note` 块字符串会移除公共前置缩进，并解码为两行文本。例如，`port: 0` 不满足 `@range(1, 65535)`。

## 3. 将 Schema 类型映射到 Rust

使用相同名称和匹配字段声明 Rust 类型：

```rust
use typol::Typol;

#[derive(Debug, Typol)]
struct Endpoint {
    host: String,
    port: u16,
}

#[derive(Debug, Typol)]
struct ServiceConfig {
    service_name: String,
    endpoint: Endpoint,
    tags: Vec<String>,
    enabled: bool,
    note: Option<String>,
}

impl ServiceConfig {
    fn service_name(&self) -> &str {
        &self.service_name
    }

    fn port(&self) -> u16 {
        self.endpoint.port
    }
}
```

派生宏会生成 Rust 类型与 Schema 类型之间的转换。Rust 类型名、字段名和类型、枚举变体名及载荷形态都必须对应 Schema 声明；绑定时会检查这种映射。derive 不会生成访问器：`impl` 块中的方法由应用自行定义。例如，`port(&self)` 读取私有字段，同时让其他模块无法直接访问该字段。

常见类型映射：

| Schema 类型 | Rust 类型 |
| --- | --- |
| `bool`、`string`、整数/浮点标量 | `bool`、`String`、相同位宽的整数/浮点类型 |
| `option<T>` | `Option<T>` |
| `[T]` | `Vec<T>` |
| `[T; N]` | `[T; N]` |
| `map<string, T>` | 启用 `std` 时为 `HashMap<String, T>`；不启用时为 `OrderedMap<T>` |
| `struct Name` / `enum Name` | 派生 `Typol` 的 Rust `struct Name` / `enum Name` |

派生 Rust 结构体必须包含 Schema 中的每个字段，即便配置可以省略某字段并依靠默认值或 `option` 规则补齐。Rust 结构体中的可选字段仍用 `Option<T>` 表示。Schema 枚举变体可以无载荷、携带单个载荷或携带命名字段；示例见[进阶教程](../advanced/)。

## 4. 编译、绑定并读取

将以下内容保存到 `src/main.rs`。程序先编译 Schema 文件，再绑定到 `ServiceConfig`，最后读取并校验配置文件：

```rust
use std::{path::Path, process::ExitCode};

use typol::{
    DefaultConfigOptions, EmptyEnvironment, SchemaLoader, Typol, TypolError, TypolOptions,
    opeo::ErrSlot,
};

#[derive(Debug, Typol)]
struct Endpoint {
    host: String,
    port: u16,
}

#[derive(Debug, Typol)]
struct ServiceConfig {
    service_name: String,
    endpoint: Endpoint,
    tags: Vec<String>,
    enabled: bool,
    note: Option<String>,
}

impl ServiceConfig {
    fn service_name(&self) -> &str {
        &self.service_name
    }

    fn port(&self) -> u16 {
        self.endpoint.port
    }
}

fn main() -> ExitCode {
    let mut errors = ErrSlot::<TypolError>::new();
    match errors.try_call(|mut out| {
        let loader = SchemaLoader::new(TypolOptions::default());
        let loaded = loader.compile_file(Path::new("service.tschem"), out.reborrow())?;
        let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
        let mut environment = EmptyEnvironment;
        let config = binding.read_file(
            Path::new("service.tconf"),
            &mut environment,
            out,
        )?;

        println!("{} listens on port {}", config.service_name(), config.port());
        Ok(())
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(caught) => {
            eprintln!("{}", caught.get());
            ExitCode::FAILURE
        }
    }
}
```

在项目目录运行：

```sh
cargo run
```

这条调用链上的方法分别是：

| 调用 | 作用 |
| --- | --- |
| `SchemaLoader::new(options)` | 使用指定的额度和策略创建 loader。 |
| `loader.compile_file(path, out)` | 加载并编译 Schema 文件。该文件 API 需要 `std`。内存 Schema 可使用 `compile_text(name, text, out)`。 |
| `loaded.bind::<ServiceConfig>(out)` | 检查 Rust 类型与 Schema 入口是否匹配，并准备可复用的绑定。 |
| `binding.read_file(path, environment, out)` | 读取配置、应用默认值、校验并转换为 `ServiceConfig`。 |

`SchemaLoader::new` 是关联函数，通过类型名调用。`compile_file`、`bind` 和 `read_file` 是作用于实例的方法。可能失败的方法接收 `Out` 错误上下文并返回 OPEO 结果；`ErrSlot::try_call` 会提供该上下文并捕获产生的 `TypolError`。

如果需要在不创建 Rust 结构体的情况下生成初始 `.tconf`，可以直接从已加载的 Schema 生成：

```rust
let defaults = loaded.default_config_to_string(
    DefaultConfigOptions::default().with_pretty(true),
    out.reborrow(),
)?;
```

入口结构体中的必填字段都需要显式 Schema 默认值；省略的 `option<T>` 会生成为 `none`。嵌套结构、数组、map 和枚举变体只有在其字段声明了显式默认值时才会生成。该 API 不读取 `@env` 值。默认策略会将真实 `@secret` 值写入文本，请按敏感数据保护结果。输出限额、秘密值拒绝和 sink 行为见[进阶教程](../advanced/)。

`LoadedSchema` 及其 binding 可以保留并复用，以读取多份配置。binding 只需检查一次 Rust 类型；每次读取仍会解析和校验输入。如果需要 Typol 的已校验值树，而不是 Rust 类型，请使用 `LoadedSchema::read_value`。

本例中的文件方法需要启用 `std`。使用内存输入时，`BoundSchema::read` 接收包装 `Read` 源的 `ReadRequest`，例如 `SliceSource`；`ReadRequest::from_text` 可直接适配文本。[no_std 指南](../no_std/)提供了完整程序示例。

## 仅在授权时读取环境变量

环境访问是显式的。要允许 `host` 使用 `SERVICE_HOST`，请在 Schema 字段上加入同名 `@env` 注解：

```tschem
host: string @host @env("SERVICE_HOST") = "127.0.0.1",
```

配置中即可写入：

```text
{
    service_name: "edge",
    endpoint: {
        host: env("SERVICE_HOST", "127.0.0.1"),
        port: 8443,
    },
}
```

在 `typol` 导入中用 `ProcessEnvironment` 和 `ProcessEnvironmentLimits` 替换 `EmptyEnvironment`，然后创建 provider：

```rust
let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
```

环境变量未设置时使用回退值。变量已设置时，Typol 会按字段声明类型解析，并继续检查约束。只有声明了匹配 `@env` 名称的字段才能使用该环境表达式。关于自定义 provider 和失败处理，请读[进阶教程](../advanced/)。

## 接下来

- 阅读 [Schema DSL 参考](../../specification/schema-dsl/)了解类型和校验规则。
- 阅读 [Config DSL 参考](../../specification/config-dsl/)了解配置值、枚举、map 和环境表达式。
- 通过 [CLI 指南](../cli/)直接检查文件，无需编写 Rust 应用。
- 阅读[进阶教程](../advanced/)了解 Schema 导入、更复杂的校验、Serde 和 Config IR。
- 阅读 [no_std 指南](../no_std/)了解不依赖标准库的内存输入方式。
