# Typol 使用示例

该 crate 演示从 Schema 文件加载配置，并通过 `#[derive(Typol)]` 将配置绑定到 Rust 类型。

## 运行

在仓库根目录执行：

```text
cargo run -p typol-showcase
cargo test -p typol-showcase
```

`schemas/application.tschem` 是入口 Schema，通过 `extern mod network;` 导入同目录中的 `schemas/network.tschem`。`config/demo.tconf` 是对应配置样例。三份 DSL 文件均与本示例 crate 一起维护。

示例包含类型别名、嵌套模块、默认值、可选字段、环境变量回退、敏感值标记、枚举的多种载荷、map、变长数组、定长数组、集合约束和结构体断言。Rust 类型使用 Typol 派生宏实现绑定；敏感字段不通过公开访问器返回。

读取配置时，应加载一次 Schema 并保留 `LoadedSchema`，再调用一次 `bind::<ShowcaseConfig>()`，通过同一个 `BoundSchema` 调用显式接收 `Environment` 的 `read`。需要继承进程环境时，创建 `ProcessEnvironment` 并将它传给 `read`。重复读取时复用该绑定，集成测试中的往返用例展示了这一流程。

进程环境变量可以覆盖样例配置中的值。未设置时，`env(...)` 使用配置中的回退值，未显式提供的字段采用 Schema 默认值。配置中的凭据文本仅供示例运行，不应用于部署。
