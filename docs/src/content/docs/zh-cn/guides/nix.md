---
title: Nix 集成
description: 在 Nix derivation 中校验公开配置，并使用纯 Nix helper 消费 Config IR。
---

当构建需要校验 Typol 配置，或读取 Config IR 的公开子集时，可以使用 Nix 集成。Flake 与 `default.nix` 入口会导出一致的 `typol-cli`、`lib.typolConfigIr`、`lib.mkConfigIr` 和 overlay。

## 构建公开 Config IR 文件

`mkConfigIr` 接收 Nixpkgs、CLI 包、源目录，以及相对于该目录的 Schema 和 Config 路径：

```nix
let
  typol = import ../.. { inherit pkgs; };
in
typol.lib.mkConfigIr {
  inherit pkgs;
  typolCli = typol.typol-cli;
  sourceDir = ./fixtures;
  schemaPath = "root.tschem";
  configPath = "public.tconf";
}
```

derivation 会保留整个源目录结构，让根 Schema 的 `extern mod` 可以从同目录解析导入。CLI 使用 `--environment empty` 和 `--reject-secrets`。无效配置会让构建失败；只要配置中存在任意 `secret: true` 标记，IR 就不会进入 derivation 输出。

源目录及其中的配置文件会成为 Nix store 输入。Config IR 即使标记为秘密仍保留真实值，因此不要把凭证放进这些文件。Nix 构造器只为不含秘密标记的配置创建 store 输出，也不会把任意环境变量传入 derivation。

IR 是普通 derivation 输出。可将文件路径传给应用 derivation，也可以在可信环境中读取预先生成的文件。Nix evaluation 不会自动读取该输出。

## 读取和选择值

`fromText` 和 `fromFile` 会解码 JSON 并校验完整的 v1 envelope。`validate` 用于校验已解码的 Nix 值。默认上限为 16 MiB 输入、128 层 IR 值或类型嵌套，以及 100,000 个 JSON/IR 处理节点。调用方可以调低额度，但不能提高硬上限。JSON 扫描器会在 `builtins.fromJSON` 折叠重复对象键之前拒绝它们。传给 `fromFile` 的文件必须先由 Nix 完整读入，然后才能检查字节上限。

```nix
let
  ir = typol.lib.typolConfigIr;
  envelope = ir.fromFile { } ./public-config-ir.json;
  decoded = ir.decode { } envelope;
  selected = ir.select {
    paths = [
      [ "endpoint" "host" ]
      [ "endpoint" "port" ]
    ];
  } decoded.value;
  port = ir.toInteger { min = 0; max = 65535; }
    (ir.at { } decoded.value [ "endpoint" "port" ]);
in {
  inherit selected port;
}
```

`at` 接受结构体字段和 map 键的字符串选择器，以及数组下标的非负整数选择器。错误会标出 IR 路径、期望类型和实际类型，但不会输出节点值。诊断中的 map 键路径会隐藏具体键名。

`select` 接受字符串路径并返回只包含这些路径的嵌套 attrset。它只检查各个选中子树中的秘密标记；附近未选择字段中的秘密不会影响结果。选择整个父节点会包含该节点的完整子树，因此子树含有秘密时会失败。`select`、`at`、类型读取器和 `project` 都支持显式设置 `includeSecrets = true`。

## 投影后的值形状

`project` 和 `select` 会把数据转换为普通 Nix 值：

| IR 类型 | Nix 值 |
| --- | --- |
| string、bool | Nix 字符串或布尔值 |
| 整数、浮点数 | 原始十进制字符串，保留大整数精度和 `-0` |
| option | `null` 或投影后的内层值 |
| array | 保持元素顺序的列表 |
| map、struct | 以 map 键或字段名为键的 attrset |
| enum | `{ variant = "..."; payload = ...; }`；载荷为 `{ kind = "unit"; }`、`{ kind = "single"; value = ...; }` 或 `{ kind = "struct"; fields = { ... }; }` |

读取节点时，`readString`、`readBool`、`readInteger` 和 `readFloat` 会保留对应的标量类型。`toInteger` 要求显式给出 Nix 整数 `min`、`max`，并拒绝超出范围的值；通常应将大整数保留为字符串。
