---
title: CLI 使用指南
description: 使用 Typol CLI 检查、格式化 Schema 和配置，并将校验后的值导出为 Config IR。
sidebar:
  order: 2
---

`typol` 命令行工具可以检查和格式化 `.tschem` Schema、校验和格式化 `.tconf` 配置文件，并将配置值导出为类型化 JSON。它与 [Rust 快速上手教程](../quickstart/)互为补充：可直接用 CLI 检查或格式化文件，也可在应用中使用 Rust 库读取配置。

CLI 支持英文和简体中文。帮助、命令行错误、操作提示以及详细的 Schema 和配置诊断都会使用同一种选定语言，并由 `typol-i18n` 中通过 `u18n` 在构建期生成的翻译目录渲染。

## 构建和运行

在 Typol 工作区根目录通过 Cargo 运行 CLI：

```sh
cargo run -p typol-cli -- --help
```

可在命令前使用 `--language en` 或 `--language zh-CN` 选择语言：

```sh
cargo run -p typol-cli -- --language zh-CN --help
```

省略 `--language` 时，CLI 会依次检查 `TYPOL_LANG`、`LC_ALL`、`LC_MESSAGES` 和 `LANG`，最后默认使用英文。`zh_CN.UTF-8` 等区域设置也可以识别。该选项也支持 `--language=zh-CN` 写法。

`--` 后的参数会传递给 Typol。若要构建独立的 release 二进制文件，运行 `cargo build --release -p typol-cli`；二进制文件位于 `target/release/typol`（Windows 上为 `target/release/typol.exe`）。以下示例假设当前目录是工作区根目录；文件路径均相对于当前工作目录解析。

## 命令

### 检查 Schema

```sh
cargo run -p typol-cli -- schema check examples/cli/cli_demo.tschem
```

该命令会编译 Schema 及其 `extern mod` 导入。成功时退出状态为 `0`，且不会向标准输出写入内容。

### 格式化 Schema

```sh
cargo run -p typol-cli -- schema fmt examples/cli/cli_demo.tschem
```

`schema fmt` 只解析语法，不加载导入，也不执行静态类型检查，然后以原子替换方式写回规范布局。它保留 token 写法、注释、声明顺序，以及输入采用的 LF 或 CRLF 换行。添加 `--check` 可比较规范结果而不写入文件；需要格式化时退出状态为 `1`。若 Schema 路径以 `-` 开头，可在路径前加 `--`。

### 检查配置

```sh
cargo run -p typol-cli -- config check \
  --schema examples/cli/cli_demo.tschem \
  --environment empty \
  examples/cli/cli_demo.tconf
```

必须使用 `--schema` 提供 Schema 文件，并提供一个配置文件路径。该命令会编译 Schema、读取配置、应用 Schema 默认值，并校验最终值。成功时退出状态为 `0`，且不会向标准输出写入内容。

### 导出 Config IR

```sh
cargo run -p typol-cli -- config export-ir \
  --schema examples/cli/cli_demo.tschem \
  --environment empty \
  --pretty \
  examples/cli/cli_demo.tconf
```

`config export-ir` 会执行相同的 Schema 和配置检查，然后将 Config IR v1 JSON 写入标准输出。添加 `--pretty` 可输出缩进格式；省略时输出紧凑格式。输出末尾带换行符，因此可以重定向或传给其他程序：

```sh
cargo run -p typol-cli -- config export-ir \
  --schema examples/cli/cli_demo.tschem \
  --environment empty \
  examples/cli/cli_demo.tconf > config.json
```

JSON 结构和兼容规则见 [Config IR v1 规范](../../specification/config-ir/)。

如果要把 IR 放入 Nix store，可添加 `--reject-secrets`。只要任意节点带有 `secret: true` 标记，命令就会在写出内容前失败。该选项会拒绝含秘密标记的配置，不会对值进行脱敏。

### 格式化配置

```sh
cargo run -p typol-cli -- config fmt \
  --schema examples/cli/cli_demo.tschem \
  --environment empty \
  examples/cli/cli_demo.tconf
```

`config fmt` 会先根据 Schema 校验文件，再将规范格式原子写回同一文件。非空数组和对象会按每层 4 个空格展开；单值枚举载荷使用多行圆括号分组，载荷括号内不加逗号。它保留源值、注释和条目顺序，不会把解析得到的环境变量值或省略的 Schema 默认字段写入文件。该布局与 Rust writer API 的 `TextLayout::Pretty` 输出一致。添加 `--check` 可逐段比较格式化结果与原文，不保留完整格式化副本，也不修改文件；需要格式化时退出状态为 `1`。

## 环境变量模式

`--environment` 接受以下两种值：

| 值 | 行为 |
| --- | --- |
| `inherit` | 读取进程环境变量。省略此选项时为默认模式。 |
| `empty` | 不读取进程环境变量。`env("NAME", fallback)` 表达式会使用回退值；没有回退值时，必需的环境值无法解析。 |

例如，CLI 示例配置会读取 `TYPOL_CLI_DEMO_KEY`。使用默认的 `inherit` 模式时，该环境变量可以提供配置值；使用 `empty` 时则采用表达式中的回退值。如 [Config DSL](../../specification/config-dsl/) 所述，目标 Schema 字段还必须通过匹配的 `@env("NAME")` 注解授权读取该变量。

## 选项和帮助

```text
typol schema check <schema.tschem>
typol schema fmt [--check] <schema.tschem>
typol config check --schema <schema.tschem> [--environment inherit|empty] <config.tconf>
typol config export-ir --schema <schema.tschem> [--environment inherit|empty] [--pretty] [--reject-secrets] <config.tconf>
typol config fmt --schema <schema.tschem> [--environment inherit|empty] [--check] <config.tconf>
```

- `--schema <路径>` 为任一 `config` 命令指定 Schema 文件。
- `--environment inherit|empty` 选择读取配置时使用的环境变量提供器。
- `--pretty` 仅适用于 `config export-ir`。
- `--reject-secrets` 仅适用于 `config export-ir`；检测到任意秘密标记时会阻止输出。
- `--check` 适用于 `schema fmt` 和 `config fmt`；它检查格式但不写入文件。
- `--language en|zh-CN` 选择 CLI 消息和详细诊断的语言，必须放在命令前；也可通过 `TYPOL_LANG` 设置。
- `-h` 和 `--help` 会显示命令帮助。例如：`typol --help`、`typol schema check --help` 或 `typol config export-ir --help`。
- 如果 Schema 或配置文件路径以 `-` 开头，可在路径前加 `--`，将其作为路径而非选项解析。

CLI 接受文件路径，不会从标准输入读取 Schema 或配置文本。Schema 导入路径相对于导入它的 Schema 文件解析。

## 输出和退出状态

检查和格式化命令成功时不会输出内容。`schema fmt --check` 或 `config fmt --check` 发现需要格式化时会向标准错误写入本地化提示。`config export-ir` 会将 JSON 写入标准输出。诊断和命令行错误会写入标准错误。

| 状态码 | 含义 |
| ---: | --- |
| `0` | 成功，包括显示帮助。 |
| `1` | Schema 或配置诊断、校验失败，或 `schema fmt --check` / `config fmt --check` 检查出需要格式化。 |
| `2` | 命令行用法错误，例如缺少参数或选项未知。 |
| `3` | 宿主或 I/O 错误、翻译渲染失败，或写入、序列化输出失败。|

`@secret` 值会在安全诊断中隐藏，但 Config IR 仍包含实际值，并添加 `"secret": true` 标记。导出的 JSON 应按敏感数据处理。

仓库中的 `examples/cli/CLI.md` 展示了枚举、导入模块、map、数组和数值边界情况的 CLI 示例。
