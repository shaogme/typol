{ pkgs }:
let
  ir = import ../lib/typol-config-ir.nix;
  demo = ir.fromFile { } ../../examples/cli/cli_demo-ir-v1.json;
  demoDecoded = ir.decode { } demo;
  numeric = ir.fromFile { } ../../examples/cli/cli_numeric-ir-v1.json;
  numericDecoded = ir.decode { } numeric;
  nixExample = import ../../examples/nix/default.nix { };

  scalarKinds = [
    "bool"
    "string"
    "i8"
    "i16"
    "i32"
    "i64"
    "i128"
    "u8"
    "u16"
    "u32"
    "u64"
    "u128"
    "f32"
    "f64"
  ];
  scalarPayload =
    kind:
    if kind == "bool" then
      true
    else if kind == "string" then
      "text"
    else
      "0";
  scalarNode = kind: value: {
    type = { inherit kind; };
    inherit value;
  };
  field = name: value: {
    inherit name value;
  };
  scalarFields = map (kind: field kind (scalarNode kind (scalarPayload kind))) scalarKinds;
  stringType = {
    kind = "string";
  };
  integerType = {
    kind = "u8";
  };
  validFields = scalarFields ++ [
    (field "none" {
      type = {
        kind = "option";
        inner = stringType;
      };
      value = null;
    })
    (field "some" {
      type = {
        kind = "option";
        inner = stringType;
      };
      value = scalarNode "string" "present";
    })
    (field "dynamic_array" {
      type = {
        kind = "array";
        element = stringType;
        length = null;
      };
      items = [
        (scalarNode "string" "a")
        (scalarNode "string" "b")
      ];
    })
    (field "fixed_array" {
      type = {
        kind = "array";
        element = integerType;
        length = 2;
      };
      items = [
        (scalarNode "u8" "1")
        (scalarNode "u8" "2")
      ];
    })
    (field "map" {
      type = {
        kind = "map";
        value = stringType;
      };
      entries = [
        {
          key = "";
          value = scalarNode "string" "empty key is valid";
        }
        {
          key = "one";
          value = scalarNode "string" "value";
        }
      ];
    })
    (field "record" {
      type = {
        kind = "struct";
        name = "Nested";
      };
      fields = [ (field "member" (scalarNode "string" "value")) ];
    })
    (field "unit_enum" {
      type = {
        kind = "enum";
        name = "State";
      };
      variant = "Ready";
      payload = {
        kind = "unit";
      };
    })
    (field "single_enum" {
      type = {
        kind = "enum";
        name = "State";
      };
      variant = "Count";
      payload = {
        kind = "single";
        value = scalarNode "u8" "1";
      };
    })
    (field "struct_enum" {
      type = {
        kind = "enum";
        name = "State";
      };
      variant = "Retry";
      payload = {
        kind = "struct";
        fields = [ (field "count" (scalarNode "u8" "1")) ];
      };
    })
  ];
  sample = {
    format = "typol-config-ir";
    version = 1;
    root_type = "Sample";
    value = {
      type = {
        kind = "struct";
        name = "Sample";
      };
      fields = validFields;
    };
  };

  rejects = value: !(builtins.tryEval (builtins.deepSeq (ir.validate { } value) true)).success;
  rejectsProjection = node: !(builtins.tryEval (builtins.deepSeq (ir.project { } node) true)).success;
  rejectsSelection =
    options: node: !(builtins.tryEval (builtins.deepSeq (ir.select options node) true)).success;
  rejectsText =
    options: text: !(builtins.tryEval (builtins.deepSeq (ir.fromText options text) true)).success;
  rejectsIntegerConversion =
    options: node: !(builtins.tryEval (builtins.deepSeq (ir.toInteger options node) true)).success;
  mapFieldNode =
    name: update: envelope:
    envelope
    // {
      value = envelope.value // {
        fields = map (
          item: if item.name == name then item // { value = update item.value; } else item
        ) envelope.value.fields;
      };
    };
  updateNode =
    name: update: envelope:
    mapFieldNode name (node: update node) envelope;
  updateNodeValue =
    name: value: envelope:
    updateNode name (node: node // { inherit value; }) envelope;
  updateNodeType =
    name: update: envelope:
    updateNode name (node: node // { type = update node.type; }) envelope;
  without = value: name: builtins.removeAttrs value [ name ];

  allScalarsAndShapesDecode = ir.decode { } sample;
  allScalarFieldsDecode = map (
    kind: ir.project { } (ir.at { } allScalarsAndShapesDecode.value [ kind ])
  ) scalarKinds;
  optionsDecode = ir.at { } allScalarsAndShapesDecode.value [ "some" ];
  arraysDecode = ir.at { } allScalarsAndShapesDecode.value [ "dynamic_array" ];
  mapsDecode = ir.at { } allScalarsAndShapesDecode.value [ "map" ];
  enumDecode = ir.at { } allScalarsAndShapesDecode.value [ "single_enum" ];

  badEnvelope = [
    (without demo "format")
    (without demo "version")
    (without demo "root_type")
    (without demo "value")
    (demo // { unexpected = true; })
    (demo // { format = 1; })
    (demo // { version = "1"; })
    (demo // { version = 2; })
    (demo // { root_type = ""; })
    (demo // { root_type = 1; })
    (demo // { root_type = "Other"; })
    (demo // { value = null; })
  ];
  badTypes = [
    (updateNodeType "endpoint" (type: without type "kind") demo)
    (updateNodeType "endpoint" (type: type // { extra = true; }) demo)
    (updateNodeType "endpoint" (type: type // { kind = 4; }) demo)
    (updateNodeType "endpoint" (type: type // { kind = "future-kind"; }) demo)
    (updateNodeType "comment" (type: without type "inner") demo)
    (updateNodeType "comment" (_: "string") demo)
    (updateNodeType "comment" (type: type // { extra = true; }) demo)
    (updateNodeType "attempts" (type: without type "element") demo)
    (updateNodeType "attempts" (type: without type "length") demo)
    (updateNodeType "attempts" (type: type // { extra = true; }) demo)
    (updateNodeType "attempts" (type: type // { length = -1; }) demo)
    (updateNodeType "attempts" (type: type // { length = "2"; }) demo)
    (updateNodeType "attempts" (type: type // { element = "string"; }) demo)
    (updateNodeType "labels" (type: without type "value") demo)
    (updateNodeType "labels" (type: type // { extra = true; }) demo)
    (updateNodeType "endpoint" (type: without type "name") demo)
    (updateNodeType "endpoint" (type: type // { name = ""; }) demo)
    (updateNodeType "endpoint" (type: type // { name = 1; }) demo)
    (updateNodeType "endpoint" (
      type:
      type
      // {
        name = "Endpoint";
        extra = true;
      }
    ) demo)
    (updateNodeType "api_key" (type: type // { extra = true; }) demo)
    (updateNodeType "access_single" (type: type // { name = ""; }) demo)
    (updateNodeType "access_single" (type: type // { name = 1; }) demo)
    (updateNodeType "access_single" (type: type // { extra = true; }) demo)
  ];
  badScalarNodes = [
    (updateNode "endpoint" (node: without node "type") demo)
    (updateNode "api_key" (node: without node "value") demo)
    (updateNode "endpoint" (node: node // { extra = true; }) demo)
    (updateNode "api_key" (node: node // { extra = true; }) demo)
    (updateNode "api_key" (node: node // { value = 1; }) demo)
    (updateNode "endpoint" (node: node // { secret = false; }) demo)
    (updateNode "endpoint" (node: node // { secret = "true"; }) demo)
    (updateNodeValue "endpoint" 3 demo)
    (updateNodeValue "api_key" null demo)
  ];
  badOptionsAndArrays = [
    (updateNode "comment" (node: without node "value") demo)
    (updateNode "comment" (node: node // { items = [ ]; }) demo)
    (updateNode "attempts" (node: node // { extra = true; }) demo)
    (updateNode "attempts" (node: without node "items") demo)
    (updateNode "attempts" (node: node // { items = "not an array"; }) demo)
    (updateNode "attempts" (node: node // { items = [ (scalarNode "string" "wrong") ]; }) demo)
    (updateNodeType "attempts" (type: type // { length = 99; }) demo)
    (updateNode "attempts" (
      node: node // { items = node.items ++ [ (builtins.head node.items) ]; }
    ) demo)
  ];
  badMaps = [
    (updateNode "labels" (node: node // { extra = true; }) demo)
    (updateNode "labels" (node: without node "entries") demo)
    (updateNode "labels" (node: node // { entries = "not an array"; }) demo)
    (updateNode "labels" (
      node: node // { entries = node.entries ++ [ (builtins.head node.entries) ]; }
    ) demo)
    (updateNode "labels" (
      node: node // { entries = [ (without (builtins.head node.entries) "key") ]; }
    ) demo)
    (updateNode "labels" (
      node: node // { entries = [ ((builtins.head node.entries) // { key = 1; }) ]; }
    ) demo)
    (updateNode "labels" (
      node: node // { entries = [ ((builtins.head node.entries) // { extra = true; }) ]; }
    ) demo)
    (updateNode "labels" (
      node: node // { entries = [ (without (builtins.head node.entries) "value") ]; }
    ) demo)
    (updateNode "labels" (
      node: node // { entries = [ ((builtins.head node.entries) // { value = "not a node"; }) ]; }
    ) demo)
    (updateNode "labels" (
      node:
      node // { entries = [ ((builtins.head node.entries) // { value = scalarNode "bool" true; }) ]; }
    ) demo)
  ];
  badStructs = [
    (updateNode "endpoint" (node: without node "fields") demo)
    (updateNode "endpoint" (node: node // { fields = "not an array"; }) demo)
    (updateNode "endpoint" (
      node: node // { fields = [ (without (builtins.head node.fields) "name") ]; }
    ) demo)
    (updateNode "endpoint" (
      node: node // { fields = [ ((builtins.head node.fields) // { name = ""; }) ]; }
    ) demo)
    (updateNode "endpoint" (
      node: node // { fields = [ ((builtins.head node.fields) // { name = 1; }) ]; }
    ) demo)
    (updateNode "endpoint" (
      node: node // { fields = [ (without (builtins.head node.fields) "value") ]; }
    ) demo)
    (updateNode "endpoint" (
      node: node // { fields = [ ((builtins.head node.fields) // { extra = true; }) ]; }
    ) demo)
    (updateNode "endpoint" (
      node: node // { fields = node.fields ++ [ (builtins.head node.fields) ]; }
    ) demo)
  ];
  badEnums = [
    (updateNode "access_single" (node: node // { extra = true; }) demo)
    (updateNode "access_single" (node: without node "variant") demo)
    (updateNode "access_single" (node: node // { variant = ""; }) demo)
    (updateNode "access_single" (node: node // { variant = 1; }) demo)
    (updateNode "access_single" (node: node // { payload = "not an object"; }) demo)
    (updateNode "access_single" (node: without node "payload") demo)
    (updateNode "access_single" (
      node:
      node
      // {
        payload = {
          kind = "unit";
          extra = true;
        };
      }
    ) demo)
    (updateNode "access_single" (
      node:
      node
      // {
        payload = {
          kind = "unknown";
        };
      }
    ) demo)
    (updateNode "access_single" (
      node:
      node
      // {
        payload = {
          kind = "single";
        };
      }
    ) demo)
    (updateNode "access_single" (
      node:
      node
      // {
        payload = {
          kind = "single";
          value = scalarNode "bool" true;
          extra = true;
        };
      }
    ) demo)
    (updateNode "access_struct" (
      node:
      node
      // {
        payload = {
          kind = "struct";
        };
      }
    ) demo)
  ];
  badIntegers = [
    (updateNodeValue "unsigned_max" "-1" numeric)
    (updateNodeValue "unsigned_max" "340282366920938463463374607431768211456" numeric)
    (updateNodeValue "signed_min" "-170141183460469231731687303715884105729" numeric)
    (updateNodeValue "signed_min" "01" numeric)
    (updateNodeValue "signed_min" "-0" numeric)
    (updateNodeValue "signed_min" "1.0" numeric)
  ];
  badFloats = map (value: updateNodeValue "negative_zero_f64" value numeric) [
    "NaN"
    "inf"
    "-Infinity"
    ".1"
    "1."
    "+1"
    "01"
    "1e3"
    "-0.0"
    "1.20"
  ];
  repeatedZeroes = count: builtins.concatStringsSep "" (builtins.genList (_: "0") count);
  f32Maximum = "34028235${repeatedZeroes 31}";
  f32Minimum = "0.${repeatedZeroes 44}1";
  f32TooLarge = "34028236${repeatedZeroes 31}";
  f64Minimum = "0.${repeatedZeroes 323}5";
  f64Maximum = "17976931348623157${repeatedZeroes 292}";
  f64TooSmall = "0.${repeatedZeroes 324}1";
  f64TooLarge = "17976931348623158${repeatedZeroes 292}";
  floatBoundaryCase = updateNodeValue "negative_zero_f64" f64Minimum (
    updateNodeValue "negative_zero_f32" f32Maximum numeric
  );
  otherFloatBoundaryCases = [
    (updateNodeValue "negative_zero_f32" f32Minimum numeric)
    (updateNodeValue "negative_zero_f64" f64Maximum numeric)
  ];
  badFloatRanges = [
    (updateNodeValue "negative_zero_f32" f32TooLarge numeric)
    (updateNodeValue "negative_zero_f32" "0.0000000000000000000000000000000000000000000001" numeric)
    (updateNodeValue "negative_zero_f64" f64TooLarge numeric)
    (updateNodeValue "negative_zero_f64" f64TooSmall numeric)
  ];
  duplicateJsonKeys = [
    ''{"format":"typol-config-ir","version":1,"root_type":"X","value":{"type":{"kind":"struct","name":"X"},"fields":[],"fields":[]}}''
    ''{"format":"typol-config-ir","version":1,"root_type":"X","value":{"type":{"kind":"struct","name":"X"},"fields":[],"\u0066ields":[]}}''
  ];
  invalidCases =
    badEnvelope
    ++ badTypes
    ++ badScalarNodes
    ++ badOptionsAndArrays
    ++ badMaps
    ++ badStructs
    ++ badEnums
    ++ badIntegers
    ++ badFloats
    ++ badFloatRanges;

  secretRoot = ir.decode { } (
    demo
    // {
      value = demo.value // {
        secret = true;
      };
    }
  );
  secretNode = ir.at { includeSecrets = true; } demoDecoded.value [ "api_key" ];
  publicNode = ir.at { } demoDecoded.value [ "endpoint" ];
  boundedPort =
    ir.toInteger
      {
        min = 0;
        max = 65535;
      }
      (
        ir.at { } demoDecoded.value [
          "endpoint"
          "port"
        ]
      );
  selectedPublicFields = ir.select {
    paths = [
      [
        "endpoint"
        "host"
      ]
      [ "attempts" ]
    ];
  } demoDecoded.value;
  explicitlyProjectedSecret = ir.project { includeSecrets = true; } secretNode;
  explicitlyProjectedParentSecret = ir.project { includeSecrets = true; } secretRoot.value;
in
assert ir.validate { } sample == sample;
assert builtins.length allScalarFieldsDecode == builtins.length scalarKinds;
assert optionsDecode.value.value == "present";
assert
  map (item: item.value) arraysDecode.value == [
    "a"
    "b"
  ];
assert (builtins.getAttr "" mapsDecode.value).value == "empty key is valid";
assert enumDecode.value.payload.kind == "single";
assert demoDecoded.rootType == "CliDemo";
assert ir.fromText { } (builtins.readFile ../../examples/cli/cli_demo-ir-v1.json) == demo;
assert builtins.all (value: ir.validate { } value == value) (
  otherFloatBoundaryCases ++ [ floatBoundaryCase ]
);
assert
  ir.readString { } (
    ir.at { } demoDecoded.value [
      "endpoint"
      "host"
    ]
  ) == "demo.internal";
assert ir.readBool { } (ir.at { } allScalarsAndShapesDecode.value [ "bool" ]) == true;
assert
  ir.readInteger { } (ir.at { } numericDecoded.value [ "signed_min" ])
  == "-170141183460469231731687303715884105728";
assert ir.readFloat { } (ir.at { } numericDecoded.value [ "negative_zero_f64" ]) == "-0";
assert boundedPort == 8443;
assert (ir.readString { includeSecrets = true; } secretNode) == "demo-secret-value";
assert secretNode.secret;
assert (ir.at { includeSecrets = true; } demoDecoded.value [ "access_single" ]).secret;
assert (ir.at { } demoDecoded.value [ "access_struct" ]).value.payload.kind == "struct";
assert (ir.at { } demoDecoded.value [ "access_struct" ]).value.payload.fields.retries.value == "3";
assert (ir.at { } demoDecoded.value [ "labels" ]).value.alpha.value == "first";
assert
  map (item: item.value) (ir.at { } demoDecoded.value [ "attempts" ]).value == [
    "1"
    "2"
  ];
assert !(ir.at { } demoDecoded.value [ "comment" ]).secret;
assert (ir.at { } demoDecoded.value [ "comment" ]).value == null;
assert
  (ir.at { } numericDecoded.value [ "signed_min" ]).value
  == "-170141183460469231731687303715884105728";
assert
  (ir.at { } numericDecoded.value [ "unsigned_max" ]).value
  == "340282366920938463463374607431768211455";
assert (ir.at { } numericDecoded.value [ "negative_zero_f32" ]).value == "-0";
assert (ir.at { } numericDecoded.value [ "negative_zero_f64" ]).value == "-0";
assert (ir.at { } numericDecoded.value [ "left" ]).type.name == "left::Item";
assert (ir.at { } numericDecoded.value [ "right" ]).type.name == "right::Item";
assert
  ir.project { } publicNode == {
    host = "demo.internal";
    port = "8443";
  };
assert rejectsProjection secretNode;
assert rejectsProjection demoDecoded.value;
assert rejectsSelection { paths = [ [ "api_key" ] ]; } demoDecoded.value;
assert rejectsSelection { paths = [ [ ] ]; } demoDecoded.value;
assert rejectsIntegerConversion {
  min = 0;
  max = 9223372036854775807;
} (ir.at { } numericDecoded.value [ "unsigned_max" ]);
assert rejectsText { maxBytes = 1; } (builtins.toJSON sample);
assert rejectsText { maxDepth = 1; } (builtins.toJSON sample);
assert rejectsText { maxNodes = 1; } (builtins.toJSON sample);
assert rejectsText { maxNodes = 100001; } (builtins.toJSON sample);
assert builtins.all (text: rejectsText { } text) duplicateJsonKeys;
assert selectedPublicFields.endpoint.host == "demo.internal";
assert !(builtins.hasAttr "api_key" selectedPublicFields);
assert
  selectedPublicFields.attempts == [
    "1"
    "2"
  ];
assert
  ir.readInteger { } (
    ir.at { } demoDecoded.value [
      "attempts"
      1
    ]
  ) == "2";
assert explicitlyProjectedSecret == "demo-secret-value";
assert explicitlyProjectedParentSecret.endpoint.host == "demo.internal";
assert nixExample.rootType == "CliDemo";
assert
  nixExample.endpoint == {
    host = "demo.internal";
    port = 8443;
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
assert builtins.all rejects invalidCases;
assert
  !(builtins.tryEval (builtins.deepSeq (ir.at { } demoDecoded.value [ "missing" ]) true)).success;
assert
  !(builtins.tryEval (builtins.deepSeq (ir.readBool { includeSecrets = true; } secretNode) true))
  .success;
pkgs.runCommand "typol-config-ir-check" { } "touch $out"
