---
title: 快速上手
description: 使用 Typol 构建一个简单的类型化配置加载器。
sidebar:
  order: 1
---

本教程将创建一个 Rust 程序：读取 `.tschem` Schema 和 `.tconf` 配置文件，校验配置，并将结果转换为 Rust 结构体。

## 在 Rust 项目中添加 Typol

Typol 要求 Rust 1.85 或更新版本。在应用的 `Cargo.toml` 中添加 `typol` crate，并启用 `std` feature。该 feature 提供文件输入和宿主环境适配器。不使用标准库的应用请参阅 [no_std 指南](../no_std/)。

```toml
[dependencies]
typol = { version = "0.1", features = ["std"] }
```

在项目根目录创建以下文件：

```text
service-app/
├── Cargo.toml
├── service.tschem
├── service.tconf
└── src/
    └── main.rs
```

如果从空项目开始，先运行 `cargo new service-app`，然后执行 `cd service-app`，再添加 Schema 和配置文件。

## 在 Schema 中描述配置

将以下内容保存到 `service.tschem`：

```text
entry struct ServiceConfig {
    service_name: string @length(1, 64),
    host: string @host = "127.0.0.1",
    port: u16 @range(1, 65535) = 8080,
    tags: [string] @size(min = 1, max = 8) = ["api"],
    enabled: bool = true,
    note: option<string>,
}
```

`entry` 用来声明配置的根类型。`service_name` 是必填字段，长度必须为 1–64 个字符。其他字段展示了标量类型、列表、约束和默认值。`note` 是没有默认值的可选字段；省略时会得到 `none`。

## 编写配置文件

将以下内容保存到 `service.tconf`：

```text
{
    service_name: "edge",
    host: "edge.internal",
}
```

配置使用 Schema 中的字段名和值类型。省略的字段会采用 Schema 默认值；省略可选字段时会得到 `none`。显式提供的值仍须符合类型和约束。例如，将 `port` 设为 `0` 会因不满足 `@range` 而校验失败。

## 将 Schema 绑定到 Rust 并读取文件

将以下内容保存到 `src/main.rs`：

```rust
use std::{path::Path, process::ExitCode};

use typol::{
    EmptyEnvironment, SchemaLoader, Typol, TypolError, TypolOptions,
    host::{DiagnosticRenderError, Language, RenderOptions, Translator},
    opeo::ErrSlot,
};

#[derive(Debug, Typol)]
struct ServiceConfig {
    service_name: String,
    host: String,
    port: u16,
    tags: Vec<String>,
    enabled: bool,
    note: Option<String>,
}

fn main() -> ExitCode {
    let mut errors = ErrSlot::<TypolError>::new();
    match errors.try_call(|mut out| {
        let loaded = SchemaLoader::new(TypolOptions::default())
            .compile_file(Path::new("service.tschem"), out.reborrow())?;
        let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
        let mut environment = EmptyEnvironment;
        let config = binding.read_file(Path::new("service.tconf"), &mut environment, out)?;

        println!("{config:#?}");
        Ok(())
    }) {
        Ok(()) => ExitCode::SUCCESS,
        Err(caught) => {
            let mut render_errors = ErrSlot::<DiagnosticRenderError>::new();
            match render_errors.call(|out| {
                caught.get().render(
                    Translator::new(Language::Chinese),
                    RenderOptions::default(),
                    out,
                )
            }) {
                Ok(diagnostics) => {
                    eprintln!("{diagnostics}");
                    ExitCode::FAILURE
                }
                Err(render_error) => {
                    eprintln!("诊断渲染失败：{render_error}");
                    ExitCode::from(3)
                }
            }
        }
    }
}
```

在项目根目录运行程序：

```sh
cargo run
```

`#[derive(Typol)]` 会生成 Schema 中 `ServiceConfig` 与 Rust 结构体之间的映射。`bind` 会检查字段和类型是否匹配。`read` 读取并校验配置，然后返回类型化的值。示例使用 `EmptyEnvironment`，因此运行结果不受当前机器环境变量影响。

启用 `std` 后，可用 `TypolError::render` 和显式传入的 `Translator` 渲染库诊断。该方法会区分 Typol 操作错误与诊断渲染或翻译错误，应用可以单独处理渲染失败。

对于此配置，返回的结构体中 `port` 为 `8080`、`tags` 为 `["api"]`、`enabled` 为 `true`、`note` 为 `None`；这些值来自 Schema 默认值或可选字段规则。

## 按需读取环境变量

环境变量读取是显式的。若要允许 `host` 字段读取 `SERVICE_HOST`，请在 Schema 字段声明中加入 `@env("SERVICE_HOST")`：

```text
host: string @host @env("SERVICE_HOST") = "127.0.0.1",
```

然后将配置文件改为：

```text
{
    service_name: "edge",
    host: env("SERVICE_HOST", "127.0.0.1"),
}
```

然后在文件顶部的 `typol` 导入中用 `ProcessEnvironment` 和 `ProcessEnvironmentLimits` 替换 `EmptyEnvironment`，并将环境提供器的初始化代码改为：

```rust
let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
```

环境变量未设置时会使用表达式中的回退值。变量已设置时，Typol 会按字段声明的类型解析它，并继续检查字段约束。只有声明了匹配 `@env` 注解的字段才能使用该表达式。

## 接下来

- 阅读 [Schema DSL 规范](../../specification/schema-dsl/)，了解类型、默认值、约束和断言。
- 阅读 [Config DSL 规范](../../specification/config-dsl/)，了解配置值、枚举、map 和环境表达式。
- 阅读 [CLI 使用指南](../cli/)，了解如何直接检查 Schema 和配置文件，或导出 Config IR。
- 阅读 [no_std 指南](../no_std/)，了解不依赖标准库的内存加载和显式提供器。
- 阅读 [进阶教程](../advanced/)，了解导入、受信任的手写类型映射、资源额度和 Config IR。
