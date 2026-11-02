{
  irFile ? ../cli/cli_demo-ir-v1.json,
}:
let
  ir = import ../../nix/lib/typol-config-ir.nix;
  decoded = ir.decode (ir.fromFile irFile);
  root = decoded.value;

  unsignedValue = node:
    let
      kind = node.type.kind;
    in
    if builtins.elem kind [ "u8" "u16" "u32" ] then
      builtins.fromJSON (ir.scalarValue kind node)
    else
      throw "examples/nix: expected an unsigned integer scalar, got ${kind}";

  projectEndpoint = node: {
    host = ir.scalarValue "string" (ir.field node "host");
    port = unsignedValue (ir.field node "port");
  };

  accessNode = ir.field root "access_struct";
  accessPayload = accessNode.value.payload;
  labelsNode = ir.field root "labels";
in
{
  rootType = decoded.rootType;
  endpoint = projectEndpoint (ir.field root "endpoint");
  access = {
    variant = accessNode.value.variant;
    endpoint = projectEndpoint accessPayload.fields.endpoint;
    retries = unsignedValue accessPayload.fields.retries;
  };
  labels = builtins.mapAttrs (_: node: ir.scalarValue "string" node) labelsNode.value;
  attempts = map unsignedValue (ir.field root "attempts").value;
  comment = (ir.field root "comment").value;
}
