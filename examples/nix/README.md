# Nix 集成示例

此示例演示如何将 Typol CLI 导出的 Config IR 读入 Nix，并投影成应用可直接使用的属性集。默认输入为 `../cli/cli_demo-ir-v1.json`；可通过 `irFile` 参数指定其他导出文件。

在仓库根目录运行以下命令，先导出配置 IR，再查看示例结果：

```sh
nix run .#typol-cli -- config export-ir \
  --schema examples/cli/cli_demo.tschem \
  --environment empty \
  examples/cli/cli_demo.tconf > /tmp/typol-config-ir.json

nix eval --impure --json --expr \
  'import ./examples/nix/default.nix { irFile = /tmp/typol-config-ir.json; }'
```

示例从 IR 中选择公开字段，并将有界无符号整数转换为 Nix 整数。敏感字段不会进入输出。IR 校验和解码由 `nix/lib/typol-config-ir.nix` 完成。

仓库的 `nix flake check` 中包含该示例的回归断言，覆盖结构体和枚举载荷、map、数组、空可选值以及敏感字段省略。
