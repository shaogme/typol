{ pkgs }:
let
  ir = import ../lib/typol-config-ir.nix;
  demo = ir.fromFile ../../examples/cli/cli_demo-ir-v1.json;
  demoFromText = ir.parse (builtins.readFile ../../examples/cli/cli_demo-ir-v1.json);
  demoDecoded = ir.decode demo;
  numeric = ir.fromFile ../../examples/cli/cli_numeric-ir-v1.json;
  numericDecoded = ir.decode numeric;
  nixExample = import ../../examples/nix/default.nix { };

  findField = fields: name: builtins.head (builtins.filter (field: field.name == name) fields);

  rejects = value: !(builtins.tryEval (builtins.deepSeq (ir.validate value) true)).success;

  firstField = builtins.head demo.value.fields;
  duplicateField = demo // {
    value = demo.value // {
      fields = demo.value.fields ++ [ firstField ];
    };
  };

  labelsNode = (findField demo.value.fields "labels").value;
  duplicateMapKey = demo // {
    value = demo.value // {
      fields = map (
        field:
        if field.name == "labels" then
          field
          // {
            value = labelsNode // {
              entries = labelsNode.entries ++ [ (builtins.head labelsNode.entries) ];
            };
          }
        else
          field
      ) demo.value.fields;
    };
  };

  unsupportedVersion = demo // {
    version = 2;
  };
  unsupportedKind = demo // {
    value = demo.value // {
      fields = map (
        field:
        if field.name == "endpoint" then
          field
          // {
            value = field.value // {
              type = field.value.type // {
                kind = "future-kind";
              };
            };
          }
        else
          field
      ) demo.value.fields;
    };
  };

  replaceIntegerField =
    name: replacement: envelope:
    envelope
    // {
      value = envelope.value // {
        fields = map (
          field:
          if field.name == name then
            field
            // {
              value = field.value // {
                value = replacement;
              };
            }
          else
            field
        ) envelope.value.fields;
      };
    };
  negativeUnsigned = replaceIntegerField "unsigned_max" "-1" numeric;
  oversizedUnsigned =
    replaceIntegerField "unsigned_max" "340282366920938463463374607431768211456"
      numeric;
  undersizedSigned =
    replaceIntegerField "signed_min" "-170141183460469231731687303715884105729"
      numeric;

  enumSingle = ir.field demoDecoded.value "access_single";
  enumStruct = ir.field demoDecoded.value "access_struct";
  apiKey = ir.field demoDecoded.value "api_key";
  labels = (ir.field demoDecoded.value "labels").value;
in
assert demoDecoded.rootType == "CliDemo";
assert demoFromText == demo;
assert (ir.scalarValue "string" apiKey) == "demo-secret-value";
assert apiKey.secret;
assert enumSingle.secret;
assert enumSingle.value.payload.kind == "single";
assert enumStruct.value.payload.kind == "struct";
assert enumStruct.value.payload.fields.retries.value == "3";
assert labels.alpha.value == "first";
assert
  map (item: item.value) ((ir.field demoDecoded.value "attempts").value) == [
    "1"
    "2"
  ];
assert !(ir.field demoDecoded.value "comment").secret;
assert (ir.field demoDecoded.value "comment").value == null;
assert
  (ir.field numericDecoded.value "signed_min").value == "-170141183460469231731687303715884105728";
assert
  (ir.field numericDecoded.value "unsigned_max").value == "340282366920938463463374607431768211455";
assert (ir.field numericDecoded.value "negative_zero_f32").value == "-0";
assert (ir.field numericDecoded.value "negative_zero_f64").value == "-0";
assert (ir.field numericDecoded.value "left").type.name == "left::Item";
assert (ir.field numericDecoded.value "right").type.name == "right::Item";
assert nixExample.rootType == "CliDemo";
assert
  nixExample.endpoint == {
    host = "demo.internal";
    port = 8443;
  };
assert
  nixExample.access == {
    variant = "Proxy";
    endpoint = {
      host = "upstream.internal";
      port = 9443;
    };
    retries = 3;
  };
assert
  nixExample.labels == {
    alpha = "first";
    zeta = "last";
  };
assert
  nixExample.attempts == [
    1
    2
  ];
assert nixExample.comment == null;
assert !(builtins.hasAttr "api_key" nixExample);
assert rejects unsupportedVersion;
assert rejects unsupportedKind;
assert rejects negativeUnsigned;
assert rejects oversizedUnsigned;
assert rejects undersizedSigned;
assert rejects duplicateField;
assert rejects duplicateMapKey;
assert !(builtins.tryEval (builtins.deepSeq (ir.field demoDecoded.value "missing") true)).success;
assert !(builtins.tryEval (builtins.deepSeq (ir.scalarValue "bool" apiKey) true)).success;
pkgs.runCommand "typol-config-ir-check" { } "touch $out"
