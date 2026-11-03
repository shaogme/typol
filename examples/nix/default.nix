{
  irFile ? ../cli/cli_demo-ir-v1.json,
}:
let
  ir = import ../../nix/lib/typol-config-ir.nix;
  decoded = ir.decode { } (ir.fromFile { } irFile);
  root = decoded.value;
  public = ir.select {
    paths = [
      [
        "endpoint"
        "host"
      ]
      [
        "endpoint"
        "port"
      ]
      [ "labels" ]
      [ "attempts" ]
      [ "comment" ]
    ];
  } root;
in
{
  rootType = decoded.rootType;
  endpoint = {
    host = public.endpoint.host;
    port =
      ir.toInteger
        {
          min = 0;
          max = 65535;
        }
        (
          ir.at { } root [
            "endpoint"
            "port"
          ]
        );
  };
  labels = public.labels;
  attempts = builtins.genList (
    index:
    ir.toInteger
      {
        min = 0;
        max = 4294967295;
      }
      (
        ir.at { } root [
          "attempts"
          index
        ]
      )
  ) (builtins.length public.attempts);
  comment = public.comment;
}
