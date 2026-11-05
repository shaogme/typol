---
title: no_std
description: 在使用 alloc 和显式宿主提供器的 no_std Rust 应用中使用 Typol。
sidebar:
  order: 2
---

Typol facade 默认兼容 `no_std`。它使用 `alloc` 保存拥有型值，因此适用于不使用 Rust 标准库、但仍提供分配器的应用。使用标准库读取文件的应用请从[快速上手](../quickstart/)开始。

## 选择 feature

`typol` crate 没有默认 feature。在 `no_std` 项目的 `Cargo.toml` 中可以明确写出这一选择：

```toml
[dependencies]
typol = { version = "0.1", default-features = false }
```

| Feature | 提供的功能 | 是否可在不启用 `std` 时使用 |
| --- | --- | --- |
| *无* | 内存 Schema 加载、文本或字节配置输入、类型绑定和显式 `Environment` 提供器 | 可以 |
| `serde` | 带 Schema 上下文的 Serde 桥接；Typol 会启用 Serde 的 `alloc` 支持 | 可以 |
| `hashbrown` | 为 `hashbrown::HashMap<String, T>` 提供 `Typol` 和 `BudgetedTypol` 绑定 | 可以 |
| `btree-map` | 为 `alloc::collections::BTreeMap<String, T>` 提供 `Typol` 和 `BudgetedTypol` 绑定 | 可以 |
| `std` | 文件和 reader 输入、文件 Schema 加载、进程环境访问和宿主诊断渲染 | 不可以 |
| `config-ir` | 使用 Serde 和 `alloc` 序列化 Config IR v1 JSON | 可以 |

即使启用 `std` feature，facade 自身仍以 `no_std` 方式编译；`std` 为 API 增加宿主适配器。`no_std` 可执行程序还必须由应用提供平台运行时和全局分配器，Typol 不提供这些组件。

启用 `config-ir` 后，可以在不启用 `std` 时将内存中的 `ConfigValue` 写入任意 `typol::io::Write` sink：

```rust
#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use typol::{
    ConfigIr, ConfigIrError, ConfigIrOptions,
    core::ConfigValue, semantic::CompiledSchema,
};

fn export_ir(
    schema: &CompiledSchema,
    value: ConfigValue,
) -> Result<Vec<u8>, ConfigIrError> {
    let mut output = Vec::new();
    ConfigIr::serialize_to(schema, value, ConfigIrOptions::default(), &mut output)?;
    Ok(output)
}
```

Schema 和配置值可以来自下文介绍的内存加载 API。目标仍需提供分配器，以存放拥有型值和序列化结果。

## 在内存中读取文本

下面的示例只使用不依赖 `std` 的 API。它编译一个不含导入的 Schema，将其绑定到拥有型 Rust 类型，再解析配置文本：

```rust
#![no_std]

extern crate alloc;

use alloc::{string::String, vec::Vec};
use typol::{
    EmptyEnvironment, SchemaLoader, Typol, TypolError, TypolOptions,
    io::{ReadRequest, SliceSource},
    opeo::{ErrSlot, OResult, Out},
};

const SCHEMA_TEXT: &str = r#"
entry struct ServiceConfig {
    name: string,
    ports: [u16],
}
"#;

const CONFIG_TEXT: &str = r#"{ name: "edge", ports: [8080, 8081] }"#;

#[derive(Typol)]
struct ServiceConfig {
    name: String,
    ports: Vec<u16>,
}

fn read_config<'slot>(out: Out<'slot, '_, TypolError>) -> OResult<'slot, ServiceConfig> {
    let loaded = SchemaLoader::new(TypolOptions::default()).compile_text(
        b"embedded-schema",
        SCHEMA_TEXT,
        out.reborrow(),
    )?;
    let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
    let mut environment = EmptyEnvironment;
    let source = SliceSource::new(CONFIG_TEXT.as_bytes());
    binding.read(ReadRequest::new(source, "embedded-config"), &mut environment, out)
}

fn load_service() -> Result<ServiceConfig, TypolError> {
    let mut errors = ErrSlot::<TypolError>::new();
    match errors.try_call(|out| read_config(out)) {
        Ok(config) => Ok(config),
        Err(caught) => Err(caught.take()),
    }
}
```

调用方传入 OPEO 错误上下文 `Out`；操作返回 `OResult`。示例中的 `load_service` 使用 `opeo::ErrSlot` 将 `OResult` 转为 core `Result`，并保留 `TypolError`。在 no_std 构建中，`TypolError` 提供阶段信息以及结构化的 Schema、配置、额度或编解码错误。它的 `Display` 文本是简短的阶段摘要；完整的宿主诊断渲染由 `std` 适配器提供。

## 提供输入源和环境访问

内存 API 将宿主访问留给应用处理：

- `SchemaLoader::compile_text` 用于编译不含 `extern mod` 导入的 Schema。若要从内存或打包资源解析导入，请实现 `SchemaResolver` 并将其传给 `SchemaLoader::compile_with_resolver`。由 `SchemaResolver` 决定如何取得源；Typol 在此路径中不会打开文件。
- `ReadRequest` 携带调用方提供的 `Read` 来源及显示名称。`SliceSource` 可适配文本或字节切片；`LoadedSchema::read_value` 和 `BoundSchema::read` 都显式接收 `Environment` 提供器。
- 如果配置不使用环境表达式，可使用 `EmptyEnvironment`。若要读取 `@env` 表达式，请实现 `Environment`，从应用选定的数据源取值。no_std 构建不会自动读取进程环境变量。

启用 `std` 后，同一加载器还可使用 `compile_file`、`FileConfigInput`、`ReadSource` 和 `ProcessEnvironment`。共有输入与宿主输入的差异见[Config 实现说明](../../specification/config-implementation/)。

## 考虑集合绑定

`OrderedMap<T>` 无需额外 feature 即可使用。它按 Rust 字符串序保存键；`try_insert` 会报告分配错误，遇到重复键时会替换并返回旧值。解码 Schema map 时会先对输入键排序一次，并在解码值前预留最终向量容量。

要使用 `hashbrown::HashMap<String, T>`，请启用 `typol/hashbrown`，并直接依赖相同版本的 `hashbrown`：

```toml
[dependencies]
hashbrown = { version = "0.17.1", default-features = false, features = ["default-hasher"] }
typol = { version = "0.1", default-features = false, features = ["hashbrown"] }
```

此 feature 不会启用 `std`。当前默认哈希构建器使用 foldhash，通常不防御 HashDoS；若键可由攻击者选择，请评估这一取舍。启用 `std` 后仍可使用标准库 `HashMap` 绑定，并保留其自身的默认哈希构建器。若同时启用 `serde` 和 `hashbrown`，Typol 会转发 hashbrown 的 Serde 支持；Serde 自身的解码回调可能在 Typol 可失败 map 预留路径之外分配内存。

要绑定 `alloc::collections::BTreeMap<String, T>`，请显式启用 `btree-map`：

```toml
[dependencies]
typol = { version = "0.1", default-features = false, features = ["btree-map"] }
```

此 feature 不会启用 `std`；`std::collections::BTreeMap` 使用同一实现。树节点没有可失败的预留接口，因此分配失败遵循全局分配器行为，可能 panic 或终止进程，而不会返回结构化转换错误。目标存储预算按逻辑 `(String, T)` 条目和键文本估算，不限制树节点或 allocator 元数据。转换细节见 [Schema 实现说明](../../specification/schema-implementation/)。

库的资源额度会统计 Typol 管理的操作和存储，但不能取代目标平台的分配策略，也不能约束应用及其提供器使用的全部内存。

## 接下来

- 阅读[快速上手](../quickstart/)，了解基于文件的 `std` 工作流。
- 阅读[进阶教程](../advanced/)，了解导入、自定义校验、Serde 和 Config IR。
- 查看 [Schema 实现说明](../../specification/schema-implementation/) 和 [Config 实现说明](../../specification/config-implementation/) 了解较底层的 API。
