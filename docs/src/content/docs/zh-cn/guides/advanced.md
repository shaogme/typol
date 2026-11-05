---
title: 进阶教程
description: 复用 Schema 绑定、组合 Schema、校验字段关系，并选择 Rust 与 JSON 集成方式。
sidebar:
  order: 3
---

本教程扩展[快速上手](../quickstart/)。先记住核心对象流程：

`SchemaLoader` 将 Schema 编译为 `LoadedSchema`；`LoadedSchema::bind::<T>()` 检查 Rust 类型并返回可复用的 `BoundSchema<T>`；绑定对象再将配置读为 `T`。应用需要读取多个文件时，应保留已加载的 Schema 和绑定对象。

文件 API 和进程环境访问需要启用 `std`。内存源和自定义 provider 不依赖 `std`，见 [no_std 指南](../no_std/)。下面的 Rust 片段是方法体，会沿用快速上手中的 `loaded`、`binding`、`out` 和导入；独立示例会展示所需设置。

## 复用已编译的 Schema 和绑定

编译和绑定一次，然后通过同一绑定读取多个输入：

```rust
let loaded = SchemaLoader::new(TypolOptions::default())
    .compile_file(Path::new("service.tschem"), out.reborrow())?;
let binding = loaded.bind::<ServiceConfig>(out.reborrow())?;

let first = binding.read_file(
    Path::new("service-a.tconf"),
    &mut environment,
    out.reborrow(),
)?;
let second = binding.read_file(
    Path::new("service-b.tconf"),
    &mut environment,
    out,
)?;
```

`compile_text(name, text, out)` 用于不含导入的 Schema。启用 `std` 后，可用 `compile_file` 解析文件导入；应用也可提供自己的 `SchemaResolver` 并调用 `compile_with_resolver`。每次编译调用都会建立新的加载会话，不会跨调用缓存文件内容。

根据需要选择读取方法：

| 调用 | 返回值 |
| --- | --- |
| `BoundSchema::read` / `read_file` | `T` 类型的 Rust 值 |
| `BoundSchema::read_with_value` | `BoundRead<T>`，同时包含 `T` 和已校验的 Typol 值树 |
| `LoadedSchema::read_value` | 不绑定 Rust 类型的已校验 Typol 值树 |
| `LoadedSchema::read_file` | 保留源文本和值位置、供编辑使用的 `ConfigDocument` |
| `LoadedSchema::read_file_value` | 从文件读取的已校验 Typol 值树 |

特别注意，`BoundSchema::read_file` 返回 Rust 类型化值；`LoadedSchema::read_file` 用于保留源格式，便于后续编辑。

## 将 Schema 拆分到多个文件

使用 `extern mod` 从其他 `.tschem` 文件导入可复用声明：

```text
schemas/
├── application.tschem
└── network.tschem
```

在 `application.tschem` 中声明模块，并通过模块路径引用导入类型：

```tschem
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

```tschem
mod address {
    type Host = string @host;
    type Port = u16 @range(1, 65535);
}

struct Endpoint {
    host: address::Host = "127.0.0.1",
    port: address::Port = 8080,
}
```

启用 `std` 后，`SchemaLoader::compile_file` 会使用文件 resolver。它会在根文件旁查找 `network.tschem`，并继续解析导入文件中的导入：

```rust
let loaded = SchemaLoader::new(TypolOptions::default())
    .compile_file(Path::new("schemas/application.tschem"), out.reborrow())?;
```

对于内存或打包资源，请实现 `SchemaResolver` 并调用 `compile_with_resolver`。resolver 每次会收到请求源身份和一个模块名。更底层的加载和编译流程见 [Schema 实现说明](../../specification/schema-implementation/)。

## 选择合适的校验规则

字段注解校验单个字段或集合。结构体或枚举变体上的 `@assert` 会检查完整值，可用于比较字段：

```tschem
struct RetryPolicy {
    delays_ms: [u32] @size(min = 1, max = 5)
        @all(item > 0, "延迟必须为正数")
        @sum(max = 60000),
    targets: [network::Endpoint] @unique(by = "host"),
}
```

`@all` 检查数组中的每个元素或 map 中的每个值。`@unique(by = "...")` 检查结构体数组中指定字段路径的唯一性；`@sum` 适用于数值数组。Schema 编译时会对表达式进行类型检查。完整配置值构造后才执行结构体和当前枚举变体的断言；若解析或局部校验失败，则不会执行这些断言。

正则表达式采用搜索语义：子串匹配即成功。需要匹配整个字符串时，请加上 `^` 和 `$`，例如 `@regex("^[a-z][a-z0-9-]*$")`。Typol 只实现文档定义的正则子集，并不支持通用正则引擎的全部功能。详见 [Schema DSL 参考](../../specification/schema-dsl/)。

## 显式控制环境访问

配置只能在同名 `@env` 授权的字段上使用 `env("NAME")`。Schema 就是环境变量访问白名单：

```tschem
entry struct ServiceConfig {
    host: string @host @env("SERVICE_HOST") = "127.0.0.1",
}
```

```text
{
    host: env("SERVICE_HOST", "127.0.0.1"),
}
```

回退值也按字段类型解析和校验。变量未设置时使用表达式回退值；变量已设置但值无效时会报错，不会改用回退值。没有表达式回退值时，未设置的变量遵循 Schema 默认值、可选字段或必填字段的常规规则。

每次读取都接收 `Environment` provider。若读取不应查询宿主环境，请使用 `EmptyEnvironment`。确实需要读取进程变量时，可在启用 `std` 后使用：

```rust
let mut environment = ProcessEnvironment::new(ProcessEnvironmentLimits::default());
let config = binding.read_file(
    Path::new("service.tconf"),
    &mut environment,
    out,
)?;
```

应用可实现 `Environment`，从 Secret Manager 或其他受控来源读取值。进程环境适配器限制接受的变量名和值长度；配置额度会单独限制一次读取查询的不同变量数量和缓存总字节数。查找与缓存行为见 [Config 实现说明](../../specification/config-implementation/)。

## 映射 Rust 结构体、枚举和递归类型

Rust 记录类型对应 Schema 结构体，字段名和类型必须匹配。字段可以保持私有，再通过应用方法暴露所需接口。枚举类型名、变体名和载荷形态也必须匹配。derive 支持无载荷、单载荷和命名字段载荷枚举。例如，以下 Schema：

```tschem
enum Storage {
    Memory,
    Local(string),
    ObjectStore {
        bucket: string,
        region: string,
    },
}
```

可以映射到：

```rust
#[derive(Typol)]
enum Storage {
    Memory,
    Local(String),
    ObjectStore {
        bucket: String,
        region: String,
    },
}
```

数组映射到 `Vec<T>` 或定长 `[T; N]`。`option<T>` 映射到 `Option<T>`。`map<string, T>` 可映射到启用 `std` 时的 `HashMap<String, T>`、不启用 `std` 时的 `OrderedMap<T>`，或[Schema 实现说明](../../specification/schema-implementation/)列出的可选 map 集成。

递归 Schema 类型必须存在有限值。例如，空子节点列表能结束有限树：

```tschem
struct Node {
    value: string,
    children: [Node],
}

entry struct TreeConfig {
    root: Node,
}
```

```rust
#[derive(Typol)]
struct Node {
    value: String,
    children: Vec<Node>,
}
```

没有有限分支的必填递归环会在 Schema 编译时被拒绝。递归值仍受嵌套深度、节点、执行、绑定和转换额度限制。

普通结构体和枚举应使用 `#[derive(Typol)]` 及常规 `bind` 方法。手写 `Typol` 实现属于应用代码，不能完全受库管理额度约束。若派生类型包含手写实现的字段类型，请添加 `#[typol(trusted)]` 并显式绑定：

```rust
#[derive(Typol)]
#[typol(trusted)]
struct ServiceConfig {
    account_id: LegacyAccountId,
    port: u16,
}

let binding = loaded.bind_trusted::<ServiceConfig>(out.reborrow())?;
```

`LegacyAccountId` 必须自行实现 `Typol`。Typol 会在转换前校验输入，但不会再次校验手写代码返回的 Rust 值。回调仍可分配内存、阻塞或执行额度之外的任意工作。仅对经过审查的映射使用此入口；它不是沙箱。

## 保留源格式并编辑配置

编辑器或应用需要保留注释、空白和源位置时，请使用 `LoadedSchema::read_file`：

```rust
let mut document = loaded.read_file(
    Path::new("service.tconf"),
    &mut environment,
    out,
)?;
```

使用 `ConfigDocument::apply_edits` 与 `ConfigPath` 批量替换、插入或删除类型化值。同一批次中的数组索引按原文解释，重叠编辑会被拒绝；删除末项后可在同批末尾追加。所有补丁会先应用到候选文本，再重新解析并校验；任何失败都会保留原文档。替换只重写目标值；插入会沿用附近可用项的换行和缩进。只需要校验值时，使用 `read_file_value` 或绑定对象的 `read_file`。

写回文档是独立的源文件操作。标准文件系统适配器无法原子地比较保留的源快照并替换路径，因此 `write_file` 对该类期望会返回 `AtomicReplaceUnsupported`。新路径使用 `write_file_with_expectation` 和 `MustBeAbsent`；有意覆盖时选择 `ReplaceAny`。自定义目标可以实现 `ConfigSourceWriter`，提供自己的原子替换行为。

## 写入类型化的 Rust 值

绑定对象会先按 Schema 校验完整值，再进行序列化。使用 `use typol::WriteOptions;` 导入 `WriteOptions`。调用顺序为 `write(&value, sink, options, out)`；`WriteOptions` 位于输出 sink 之后：

```rust
let mut text = String::new();
binding.write(
    &config,
    &mut text,
    WriteOptions::default().with_pretty(true),
    out,
)?;
```

默认输出是紧凑格式；pretty 格式使用两空格缩进。写入器不会追加结尾换行，格式空白也计入输出额度。首字节写入前会完成校验和转换；sink 或输出额度失败时可能留下已写入的前缀。标记为 `@secret` 的值仍会写入输出。

需要缓冲为 `String` 时使用 `write_to_string`。启用 `serde` 后，`write_serde` 使用同一组选项。

## 使用带 Schema 上下文的 Serde 桥接

启用 Typol 的 `serde` feature 和 Serde 的 derive feature：

```toml
[dependencies]
typol = { version = "0.1", features = ["std", "serde"] }
serde = { version = "1", features = ["derive"] }
```

`read_serde` 和 `write_serde` 会使用成功绑定的 Schema 校验 Serde 数据模型事件。字段名、类型和枚举载荷必须匹配 Schema。脱离桥接直接调用 Serde 的 `Serialize` 或 `Deserialize` trait 时没有 Typol Schema 上下文，也不承诺通过 Schema 校验。自定义 Serde 实现仍属于调用方代码，有自己的信任边界。支持的数据模型操作见 [Schema 实现说明](../../specification/schema-implementation/)。

## 导出类型化 JSON：Config IR

启用 `config-ir` 后，可将校验后的值序列化为 Config IR v1。它同时启用 `serde`，并支持 `no_std + alloc`：

```toml
[dependencies]
typol = { version = "0.1", features = ["config-ir"] }
```

`ConfigIrOptions::default()` 选择 `Validated`：校验提供的值，但不会补入缺失的默认值。选择 `ConfigIrValueMode::WithDefaults` 可补齐 Schema 默认值并校验；选择 `AsProvided` 则会跳过这两步。`LoadedSchema::read_value` 返回的值已经应用默认值，可将其 `into_config_value()` 交给序列化器。

Map 条目和结构体字段按确定顺序输出。整数以 JSON 字符串编码，以保留完整的 `i128` 和 `u128` 范围。标记为 `"secret": true` 的节点仍包含实际值；标记只是元数据，不会加密或脱敏。节点及其子树都应作为敏感数据处理。详见 [Config IR v1 参考](../../specification/config-ir/)。

## 配置资源上限并保护敏感值

`TypolOptions` 可以配置 Schema、配置、绑定、文件、转换和执行策略。使用 `use typol::{config::ConfigLimits, TypolOptions};` 导入示例所需类型。请根据预期输入调整相应上限，例如：

```rust
let options = TypolOptions::default().with_config_limits(
    ConfigLimits::default()
        .with_max_source_bytes(256 * 1024)
        .with_max_nesting_depth(64)
        .with_max_diagnostics(32),
);
```

这些上限约束库管理的输入和工作区，不是操作系统级 CPU、进程总内存或实时性上限。手写 `Typol` 和自定义 Serde 回调仍属于用户代码边界。

`@secret` 会保护诊断和安全展示，包括隐藏秘密值下层的路径。成功的 Rust 值和 Config IR 仍包含真实秘密内容，应用需要自行保护日志、序列化输出和存储。

## 接下来

- 完整多文件应用示例位于仓库的 `examples/showcase`。
- 通过 [CLI 使用指南](../cli/)校验或格式化文件，或导出 Config IR 文档。
- 阅读 [Schema DSL](../../specification/schema-dsl/) 和 [Config DSL](../../specification/config-dsl/)了解规范性语言行为。
- 查阅 [Schema 实现](../../specification/schema-implementation/)、[Config 实现](../../specification/config-implementation/) 和 [Config IR v1 参考](../../specification/config-ir/)了解详细约定。
