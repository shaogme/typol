---
title: Typol 文档
description: Typol（面向 Rust 的 Schema 驱动配置库）中英文双语文档。
template: splash
hero:
  tagline: 面向 Rust 的 Schema 驱动配置库。
  actions:
    - text: 快速上手
      link: guides/quickstart/
      icon: right-arrow
    - text: CLI 使用指南
      link: guides/cli/
      variant: minimal
    - text: 浏览规范
      link: specification/schema-dsl/
      variant: minimal
---

## 文档

Typol 编译 `.tschem` Schema，按编译后的类型解析 `.tconf` 配置值，校验约束和断言，并可通过 `#[derive(Typol)]` 将结果绑定到 Rust 类型。

教程介绍 Rust 集成和 CLI 工作流；下方规范说明 DSL 约定、当前实现、Config IR 导出格式和正则引擎。使用语言选择器可切换到 English。

- [快速上手教程](guides/quickstart/)
- [no_std 指南](guides/no_std/)
- [CLI 使用指南](guides/cli/)
- [进阶教程](guides/advanced/)
- [Config DSL 规范](specification/config-dsl/)
- [Schema DSL 规范](specification/schema-dsl/)
- [Config 实现说明](specification/config-implementation/)
- [Schema 实现说明](specification/schema-implementation/)
- [Config IR v1](specification/config-ir/)
- [正则实现说明](specification/regex-implementation/)

## 本地运行文档站

在仓库根目录执行：

```sh
cd docs
npm install
npm run dev
```

运行 `npm run build` 可生成生产站点。
