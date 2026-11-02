# CLI 示例

命令用法、参数和退出状态详见[CLI 使用指南](../../docs/src/content/docs/guides/cli.md)。

`cli_demo.tschem` 会导入同目录的 `common.tschem`，使用进程环境表达式，并覆盖结构体、三种枚举载荷、map、定长数组、option 和 secret 标记。下面的 `empty` 模式会忽略宿主环境变量，并使用 DSL 中的回退值：

```sh
cargo run -p typol-cli -- schema check examples/cli/cli_demo.tschem
cargo run -p typol-cli -- config check --schema examples/cli/cli_demo.tschem --environment empty examples/cli/cli_demo.tconf
cargo run -p typol-cli -- config export-ir --schema examples/cli/cli_demo.tschem --environment empty examples/cli/cli_demo.tconf
```

导出结果与 [`cli_demo-ir-v1.json`](cli_demo-ir-v1.json) 对应。`api_key` 和 `ApiToken` 的 payload 在 IR 中保留真实值并标记 `secret: true`；导出的 stdout 应按敏感数据处理。

Rust 调用方可在依赖中启用 `typol` 的 `config-ir` feature，然后直接复用相同的 v1 序列化器：

```toml
typol = { version = "0.1", features = ["config-ir"] }
```

```rust
use typol::{ConfigIr, ConfigIrOptions, ConfigIrValueMode};

let bytes = ConfigIr::serialize(
    loaded_schema.schema(),
    config_value,
    ConfigIrOptions::default().with_value_mode(ConfigIrValueMode::Validated).with_pretty(true),
)?;
```

`ConfigIr::serialize` 消费传入的 `ConfigValue`。`ConfigIrOptions` 默认使用 `Validated`，先补齐默认值再执行完整 Schema 校验；`WithDefaults` 只补齐默认值，`AsProvided` 则显式跳过规范化和约束校验。后两种模式可通过 `with_validation_options` 配置资源额度和执行预算。`ConfigIrOptions` 还可分别设置 pretty 输出、最大 JSON 字节数、排序工作区字节数和最大嵌套深度；默认字节额度均为 16 MiB，默认深度为 128，嵌套深度上限可降低但不能提高。调用方可用 `ConfigIrError::kind()` 区分输出超限与排序工作区超限；规范化和校验错误仍保留经过敏感信息处理的诊断载荷。序列化错误不包含配置值。IR 的 `secret: true` 只标记敏感节点，不会擦除其中的值。

[`all_types.tschem`](../config/all_types.tschem) 与 [`all_types.tconf`](../config/all_types.tconf) 展示所有整数位宽、`f32`/`f64`、语义类型、约束和完整容器形态。该示例需要为必填的 `TYPOL_NOTIFY_TOKEN` 提供演示值：

```sh
TYPOL_NOTIFY_TOKEN='example-only-token' cargo run -p typol-cli -- config export-ir --schema examples/config/all_types.tschem examples/config/all_types.tconf
```

[`cli_numeric.tschem`](cli_numeric.tschem) 与 [`cli_numeric.tconf`](cli_numeric.tconf) 展示 `i128::MIN`、`u128::MAX`、`f32`/`f64` 负零、Unicode 字符串，以及两个不同模块中同名 `Item` 的限定名。
