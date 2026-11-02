---
title: 进阶教程
description: 学习 Schema 导入、跨字段校验、显式环境变量访问、受信任的 Rust 映射、Serde 和 Config IR。
sidebar:
  order: 3
---

本教程基于[快速上手](../quickstart/)，介绍如何在可复用 Schema、更复杂的校验逻辑以及 Rust 或 JSON 数据边界中集成 Typol。使用文件或读取进程环境变量的示例需要启用 Typol 的 `std` feature。不依赖标准库、通过内存输入集成的方式见 [no_std 指南](../no_std/)。

## 将 Schema 拆分到多个文件

使用 `extern mod` 从其他 `.tschem` 文件导入声明。例如，将可复用的网络类型与应用 Schema 放在一起：

```text
schemas/
├── application.tschem
└── network.tschem
```

在 `application.tschem` 中声明外部模块，并通过模块路径引用其中的类型：

```text
extern mod network;

struct WorkerPool {
    min_threads: u16 @range(1, 256) = 2,
    max_threads: u16 @range(1, 256) = 8,
} @assert(self.min_threads <= self.max_threads, "最小线程数不能超过最大线程数")

entry struct ServiceConfig {
    endpoint: network::Endpoint,
    peers: [network::Endpoint] @size(min = 0, max = 8) @unique(by = "host"),
    worker_pool: WorkerPool = {},
}
@assert(
    !self.peers.any(|peer| peer.host == self.endpoint.host),
    "对等节点不能使用本机主机名"
)
```

在 `network.tschem` 中定义被导入的类型：

```text
mod address {
    type Host = string @host;
    type Port = u16 @range(1, 65535);
}

struct Endpoint {
    host: address::Host = "127.0.0.1",
    port: address::Port = 8080,
}
```

`SchemaLoader::compile_text` 适用于不含导入的 Schema。启用 `std` 后，`compile_file` 会使用文件 resolver：它会先在 `application.tschem` 旁查找 `network.tschem`，也支持以根文件名命名的目录布局：

```rust
let loaded = SchemaLoader::new(TypolOptions::default())
    .compile_file(Path::new("schemas/application.tschem"), out.reborrow())?;
```

文件 resolver 也会继续解析被导入文件中的导入。`LoadedSchema` 会持有已编译的 Schema，可供多次配置读取复用。每次调用 `compile_file` 都会启动新的加载会话，文件内容不会跨调用缓存。对于内存中的源或打包后的源，可以实现 `SchemaResolver` 并调用 `compile_with_resolver`；resolver 每次会收到请求源的身份和一个模块名。更底层的加载模型见 [Schema 实现说明](../../specification/schema-implementation/)。

## 校验值之间的关系

字段注解校验单个值或集合。`@assert` 则在结构体或已选中的枚举变体完整构造后执行，因此可以表达字段之间的关系。上例中，线程池检查了两个边界值，入口断言检查每个对等节点是否与本机 endpoint 冲突。

其他常见的集合约束包括：

```text
struct RetryPolicy {
    delays_ms: [u32] @size(min = 1, max = 5) @all(item > 0, "延迟必须为正数") @sum(max = 60000),
    targets: [network::Endpoint] @unique(by = "host"),
}
```

Schema 编译时会对表达式进行类型检查。`@all` 检查数组的每个元素或 map 的每个值；`@unique(by = "...")` 检查结构体数组中指定字段路径的唯一性；`@sum` 适用于数值数组。配置完全解析并通过局部校验后才会执行断言。如果解析或局部约束校验失败，根断言不会执行。

正则表达式采用搜索语义：只要有子串匹配就算成功。若要匹配整个字符串，请加上 `^` 和 `$`，例如 `@regex("^[a-z][a-z0-9-]*$")`。Typol 支持文档定义的正则子集，不等同于通用正则引擎的全部功能，详见 [Schema DSL 规范](../../specification/schema-dsl/)。

## 显式控制环境变量访问

只有目标字段使用同名 `@env` 声明后，才可以在该字段中使用 `env("NAME")`。Schema 本身就是环境变量访问白名单，配置文本不能任意读取进程变量。

```text
entry struct ServiceConfig {
    host: string @host @env("SERVICE_HOST") = "127.0.0.1",
}
```

```text
{
    host: env("SERVICE_HOST", "127.0.0.1"),
}
```

回退值也会按字段类型解析并校验。变量未设置时，若表达式提供了回退值，就使用该值；变量已设置但值无效时会报错，不会改用回退值。若表达式没有回退值，变量未设置时会遵循 Schema 默认值、可选字段或必填字段的常规规则。

每次读取都显式接收一个 `Environment` 提供器。不需要访问宿主环境时使用 `EmptyEnvironment`，这样读取结果不会依赖环境变量；确实需要读取进程变量时，可使用 `std` 提供的适配器：

```rust
let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
let config = binding.read(input, &mut environment, out)?;
```

`ProcessEnvironmentLimits` 限制进程环境适配器接受的变量名和值字节数。`ConfigLimits` 则单独限制一次配置读取中查询的不同变量数和缓存的总字节数。应用也可以实现 `Environment`，从 Secret Manager 或其他受控来源取值。查找、缓存与回退行为见 [Config 实现说明](../../specification/config-implementation/)。

## 绑定自定义 Rust 类型并使用 Serde

普通 Rust 结构体和枚举优先使用 `#[derive(Typol)]`。常规 `bind` 入口支持内建或 derive 生成的实现，其嵌套字段和枚举载荷也必须符合相同条件。

类型包含手写 `Typol` 实现时，为包含它的 derive 标记信任属性，并调用 `bind_trusted`：

```rust
#[derive(Typol)]
#[typol(trusted)]
struct ServiceConfig {
    account_id: LegacyAccountId,
    port: u16,
}

let binding = loaded.bind_trusted::<ServiceConfig>(out.reborrow())?;
```

`LegacyAccountId` 自身必须实现手写的 `Typol`。这类回调属于应用代码：Typol 只会对经 `ConversionContext` 分发的工作计量，无法约束回调直接执行的任意计算、阻塞或内存分配。输入在转换前会按 Schema 校验，但回调返回的 Rust 值不会再次校验。请保持手写实现简洁并经过审查。实现契约详见[Schema 实现说明](../../specification/schema-implementation/)。

可选的 Serde 桥接能在绑定 Schema 的上下文中读取或写入 Serde 类型。启用 Typol 的 `serde` feature 和 Serde 的 derive feature：

```toml
[dependencies]
typol = { version = "0.1", features = ["std", "serde"] }
serde = { version = "1", features = ["derive"] }
```

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Typol)]
struct ServiceConfig {
    host: String,
    port: u16,
}

let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;
let config = binding.read_serde(input, &mut environment, out)?;
```

Schema 字段名和类型必须与 Serde 数据模型一致。`read_serde` 会先解析并校验值，再构造目标类型。脱离 `TypedSchema` 桥接直接调用 `Serialize` 或 `Deserialize` 时，没有 Typol Schema 上下文，也不保证通过 Schema 校验。自定义 Serde 实现同样属于调用方代码，有独立的信任边界；支持的数据模型详见 [Schema 实现说明](../../specification/schema-implementation/)。

## 导出类型化 JSON：Config IR

启用 `config-ir` feature 后，可以将 `ConfigValue` 序列化为 Config IR v1。它可在 `no_std + alloc` 环境中使用，不会启用 `std`：

```toml
[dependencies]
typol = { version = "0.1", features = ["config-ir"] }
```

默认模式会先填入 Schema 默认值并完成完整校验，再生成 JSON。对于已通过校验读取到的值，可以使用如下 helper：

```rust
use std::vec::Vec;
use typol::{ConfigIr, ConfigIrError, ConfigIrOptions};
use typol::{core::ConfigValue, semantic::CompiledSchema};

fn export_ir(
    schema: &CompiledSchema,
    value: ConfigValue,
) -> Result<Vec<u8>, ConfigIrError> {
    ConfigIr::serialize(schema, value, ConfigIrOptions::new().with_pretty(true))
}
```

在提供分配器的 `no_std` 应用中也可以使用同一 API，并从 `alloc::vec::Vec` 导入输出类型。内存 Schema 加载和配置值读取不需要文件或进程环境适配器。

将 `LoadedSchema::read_value` 返回值的 `into_config_value()` 结果传给该 helper。若明确需要跳过默认值补齐和约束校验，可通过 `ConfigIrOptions::with_value_mode` 选择 `ConfigIrValueMode::AsProvided`。Map 条目和结构体字段的输出顺序是确定的；整数以 JSON 字符串编码，因此能保留完整的 `i128` 和 `u128` 范围。

标记为 `"secret": true` 的 IR 节点仍包含实际值。必须将该节点及其子树都当作敏感数据处理；标记只是元数据，不会加密或脱敏值。详见 [Config IR v1 规范](../../specification/config-ir/)。

## 配置资源上限并处理诊断

根据预期输入调整配置上限，而不是依赖更宽泛的宿主预算。例如，下面的设置限制每个配置源的大小、嵌套深度和收集的诊断数：

```rust
use typol::{TypolOptions, config::ConfigLimits};

let options = TypolOptions::default().with_config_limits(
    ConfigLimits::default()
        .with_max_source_bytes(256 * 1024)
        .with_max_nesting_depth(64)
        .with_max_diagnostics(32),
);
```

`TypolOptions` 还可以设置 Schema、绑定、文件、转换和执行策略。需要注意，这些上限约束的是库管理的输入和工作区，不是操作系统级的 CPU、进程总内存或实时性限制。手写 `Typol` 和 Serde 回调仍属于用户代码边界。

使用 `TypolError` 的诊断接口返回或展示错误，避免记录原始配置值。`@secret` 会保护诊断和安全显示中的敏感值，包括隐藏该值下层的路径。成功的 Rust 配置值和 Config IR 仍包含真实密钥，因此应用自身还需要控制日志和存储。

## 接下来

- 完整的多文件应用 Schema 和配置示例见 `examples/showcase`。
- 通过 [CLI 使用指南](../cli/)了解如何在命令行中检查文件或导出校验后的配置值。
- 阅读 [Schema DSL](../../specification/schema-dsl/) 和 [Config DSL](../../specification/config-dsl/) 了解规范定义的语言行为。
- 查阅 [Schema 实现说明](../../specification/schema-implementation/)、[Config 实现说明](../../specification/config-implementation/) 和 [Config IR v1 规范](../../specification/config-ir/) 了解 API 与格式契约。
