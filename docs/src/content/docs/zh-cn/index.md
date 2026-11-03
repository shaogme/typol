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

<div class="typol-flow" role="list" aria-label="Typol 配置工作流程">
  <div class="typol-flow-step" role="listitem">
    <span class="typol-flow-index">01</span>
    <strong>定义 Schema</strong>
    <code>.tschem</code>
  </div>
  <div class="typol-flow-step" role="listitem">
    <span class="typol-flow-index">02</span>
    <strong>解析配置</strong>
    <code>.tconf</code>
  </div>
  <div class="typol-flow-step" role="listitem">
    <span class="typol-flow-index">03</span>
    <strong>绑定 Rust 类型</strong>
    <code>#[derive(Typol)]</code>
  </div>
</div>

Typol 编译 `.tschem` Schema，按编译后的类型解析 `.tconf` 配置值并校验约束，最后将结果绑定到 Rust 类型。使用语言选择器可切换到 English。

## 文档

<div class="typol-doc-grid">
  <section class="typol-doc-card">
    <span class="typol-card-kicker">01 / 从这里开始</span>
    <h3>快速开始</h3>
    <p>构建类型化配置加载器，了解从 Schema 到 Rust 类型的完整流程。</p>
    <ul>
      <li><a href="guides/quickstart/">快速上手教程</a></li>
    </ul>
  </section>
  <section class="typol-doc-card">
    <span class="typol-card-kicker">02 / 按需学习</span>
    <h3>使用指南</h3>
    <p>根据任务选择 CLI 工作流、进阶集成或 no_std 应用指南。</p>
    <ul>
      <li><a href="guides/cli/">CLI 使用指南</a></li>
      <li><a href="guides/advanced/">进阶教程</a></li>
      <li><a href="guides/no_std/">no_std 指南</a></li>
    </ul>
  </section>
  <section class="typol-doc-card">
    <span class="typol-card-kicker">03 / 技术参考</span>
    <h3>技术规范</h3>
    <p>查阅语言规则、导出数据格式与各组件的实现行为。</p>
    <ul>
      <li><a href="specification/schema-dsl/">Schema DSL 规范</a></li>
      <li><a href="specification/config-dsl/">Config DSL 规范</a></li>
      <li><a href="specification/config-ir/">Config IR v1</a></li>
      <li><a href="specification/schema-implementation/">Schema 实现说明</a></li>
      <li><a href="specification/config-implementation/">Config 实现说明</a></li>
      <li><a href="specification/regex-implementation/">正则实现说明</a></li>
    </ul>
  </section>
</div>

## 本地运行文档站

在仓库根目录执行：

```sh
cd docs
npm install
npm run dev
```

运行 `npm run build` 可生成生产站点。
