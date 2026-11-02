let
  formatName = "typol-config-ir";
  supportedVersion = 1;

  fail = message: throw "typolConfigIr: ${message}";

  require = condition: message: if condition then true else fail message;

  isNonEmptyString = value: builtins.isString value && value != "";

  hasDuplicates =
    values:
    builtins.length (
      builtins.attrNames (
        builtins.listToAttrs (
          map (value: {
            name = value;
            value = true;
          }) values
        )
      )
    ) != builtins.length values;

  requireUnique = values: label: require (!hasDuplicates values) "重复的${label}";

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

  integerBounds = {
    i8 = {
      positive = "127";
      negative = "128";
    };
    i16 = {
      positive = "32767";
      negative = "32768";
    };
    i32 = {
      positive = "2147483647";
      negative = "2147483648";
    };
    i64 = {
      positive = "9223372036854775807";
      negative = "9223372036854775808";
    };
    i128 = {
      positive = "170141183460469231731687303715884105727";
      negative = "170141183460469231731687303715884105728";
    };
    u8 = {
      positive = "255";
      negative = null;
    };
    u16 = {
      positive = "65535";
      negative = null;
    };
    u32 = {
      positive = "4294967295";
      negative = null;
    };
    u64 = {
      positive = "18446744073709551615";
      negative = null;
    };
    u128 = {
      positive = "340282366920938463463374607431768211455";
      negative = null;
    };
  };

  decimalMagnitudeWithin =
    value: maximum:
    let
      valueLength = builtins.stringLength value;
      maximumLength = builtins.stringLength maximum;
    in
    valueLength < maximumLength || (valueLength == maximumLength && value <= maximum);

  integerInRange =
    kind: value:
    let
      negative = builtins.substring 0 1 value == "-";
      magnitude =
        if negative then builtins.substring 1 (builtins.stringLength value - 1) value else value;
      bounds = integerBounds.${kind};
      maximum = if negative then bounds.negative else bounds.positive;
    in
    builtins.isString maximum && decimalMagnitudeWithin magnitude maximum;

  validateType =
    descriptor: path:
    if !builtins.isAttrs descriptor then
      fail "${path} 必须是类型描述对象"
    else if !builtins.hasAttr "kind" descriptor || !builtins.isString descriptor.kind then
      fail "${path}.kind 缺失或不是字符串"
    else
      let
        kind = descriptor.kind;
      in
      if builtins.elem kind scalarKinds then
        true
      else if kind == "option" then
        require (builtins.hasAttr "inner" descriptor) "${path}.inner 缺失"
        && validateType descriptor.inner "${path}.inner"
      else if kind == "array" then
        require (builtins.hasAttr "element" descriptor) "${path}.element 缺失"
        && validateType descriptor.element "${path}.element"
        && require (builtins.hasAttr "length" descriptor) "${path}.length 缺失"
        && (
          builtins.isNull descriptor.length || (builtins.isInt descriptor.length && descriptor.length >= 0)
        )
      else if kind == "map" then
        require (builtins.hasAttr "value" descriptor) "${path}.value 缺失"
        && validateType descriptor.value "${path}.value"
      else if kind == "struct" || kind == "enum" then
        require (
          builtins.hasAttr "name" descriptor && isNonEmptyString descriptor.name
        ) "${path}.name 缺失或为空"
      else
        fail "${path}.kind 不受支持：${kind}";

  validateFields =
    fields: path:
    if !builtins.isList fields then
      fail "${path} 必须是字段数组"
    else
      let
        validField =
          index:
          let
            field = builtins.elemAt fields index;
          in
          if !builtins.isAttrs field then
            fail "${path}[${toString index}] 必须是对象"
          else if !builtins.hasAttr "name" field || !isNonEmptyString field.name then
            fail "${path}[${toString index}].name 缺失或为空"
          else if !builtins.hasAttr "value" field then
            fail "${path}[${toString index}].value 缺失"
          else
            validateNode field.value "${path}.${field.name}";
        fieldChecks = builtins.genList validField (builtins.length fields);
        shapeCheck = builtins.all (check: check) fieldChecks;
        fieldNames = map (field: field.name) fields;
      in
      shapeCheck && requireUnique fieldNames "字段名 in ${path}";

  validateNode =
    node: path:
    if !builtins.isAttrs node then
      fail "${path} 必须是节点对象"
    else if !builtins.hasAttr "type" node then
      fail "${path}.type 缺失"
    else
      let
        typeCheck = validateType node.type "${path}.type";
        secretCheck = !builtins.hasAttr "secret" node || builtins.isBool node.secret;
        kind = node.type.kind or "";
        hasValue = builtins.hasAttr "value" node;
        value = if hasValue then node.value else null;
        isInteger = builtins.elem kind [
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
        ];
        isFloat = builtins.elem kind [
          "f32"
          "f64"
        ];
        validScalar =
          if kind == "bool" then
            hasValue && builtins.isBool value
          else if kind == "string" then
            hasValue && builtins.isString value
          else if isInteger then
            hasValue
            && builtins.isString value
            && builtins.match "-?(0|[1-9][0-9]*)" value != null
            && integerInRange kind value
          else if isFloat then
            hasValue
            && builtins.isString value
            && builtins.match "-?(0|[1-9][0-9]*)(\\.[0-9]+)?([eE][+-]?[0-9]+)?" value != null
          else
            true;
        payloadCheck =
          if kind == "option" then
            hasValue
            && (
              builtins.isNull value
              || (
                validateNode value "${path}.value"
                && require (value.type == node.type.inner) "${path} 的 option 子节点类型不匹配"
              )
            )
          else if kind == "array" then
            builtins.hasAttr "items" node
            && builtins.isList node.items
            && (builtins.isNull node.type.length || builtins.length node.items == node.type.length)
            && builtins.all (
              item:
              validateNode item "${path}.items" && require (item.type == node.type.element) "${path} 的数组元素类型不匹配"
            ) node.items
          else if kind == "map" then
            builtins.hasAttr "entries" node
            && builtins.isList node.entries
            && builtins.all (
              entry: builtins.isAttrs entry && builtins.hasAttr "key" entry && builtins.isString entry.key
            ) node.entries
            && requireUnique (map (entry: entry.key) node.entries) "map key in ${path}"
            && builtins.all (
              entry:
              builtins.hasAttr "value" entry
              && validateNode entry.value "${path}.entries.${entry.key}"
              && require (entry.value.type == node.type.value) "${path} 的 map value 类型不匹配"
            ) node.entries
          else if kind == "struct" then
            builtins.hasAttr "fields" node && validateFields node.fields "${path}.fields"
          else if kind == "enum" then
            builtins.hasAttr "variant" node
            && isNonEmptyString node.variant
            && builtins.hasAttr "payload" node
            && builtins.isAttrs node.payload
            && builtins.hasAttr "kind" node.payload
            && builtins.elem node.payload.kind [
              "unit"
              "single"
              "struct"
            ]
            && (
              if node.payload.kind == "unit" then
                true
              else if node.payload.kind == "single" then
                builtins.hasAttr "value" node.payload && validateNode node.payload.value "${path}.payload.value"
              else
                builtins.hasAttr "fields" node.payload
                && validateFields node.payload.fields "${path}.payload.fields"
            )
          else
            true;
      in
      typeCheck
      && require secretCheck "${path}.secret 必须是布尔值"
      && require validScalar "${path} 的标量载荷与类型不匹配"
      && require payloadCheck "${path} 的载荷形态与类型不匹配";

  validate =
    envelope:
    if !builtins.isAttrs envelope then
      fail "IR envelope 必须是对象"
    else
      let
        headerCheck =
          require ((envelope.format or null) == formatName) "format 必须是 ${formatName}"
          && require ((envelope.version or null) == supportedVersion) "不支持的 IR version"
          && require (isNonEmptyString (envelope.root_type or null)) "root_type 缺失或为空"
          && require (builtins.hasAttr "value" envelope) "value 缺失";
        nodeCheck =
          validateNode envelope.value "value"
          && require (envelope.value.type.kind == "struct") "根节点必须是 struct"
          && require (envelope.value.type.name == envelope.root_type) "root_type 与根节点类型名不匹配";
      in
      if headerCheck && nodeCheck then envelope else fail "无效 IR envelope";

  decodeFields =
    fields:
    builtins.listToAttrs (
      map (field: {
        name = field.name;
        value = decodeNode field.value;
      }) fields
    );

  decodeNode =
    node:
    let
      kind = node.type.kind;
      decodedValue =
        if kind == "array" then
          map decodeNode node.items
        else if kind == "map" then
          builtins.listToAttrs (
            map (entry: {
              name = entry.key;
              value = decodeNode entry.value;
            }) node.entries
          )
        else if kind == "struct" then
          decodeFields node.fields
        else if kind == "enum" then
          {
            variant = node.variant;
            payload =
              if node.payload.kind == "single" then
                {
                  kind = "single";
                  value = decodeNode node.payload.value;
                }
              else if node.payload.kind == "struct" then
                {
                  kind = "struct";
                  fields = decodeFields node.payload.fields;
                }
              else
                { kind = "unit"; };
          }
        else if kind == "option" && !builtins.isNull node.value then
          decodeNode node.value
        else
          node.value;
    in
    {
      type = node.type;
      secret = node.secret or false;
      value = decodedValue;
    };

  decode =
    envelope:
    let
      checked = validate envelope;
    in
    {
      rootType = checked.root_type;
      value = decodeNode checked.value;
    };

  field =
    node: name:
    if !builtins.isString name then
      fail "field 名称必须是字符串"
    else if !builtins.isAttrs node || (node.type.kind or "") != "struct" then
      fail "field 需要 struct 节点"
    else
      let
        fields = node.value;
      in
      if builtins.hasAttr name fields then fields.${name} else fail "struct 中不存在字段：${name}";

  scalarValue =
    expectedKind: node:
    if !builtins.isAttrs node || (node.type.kind or "") != expectedKind then
      fail "scalarValue 期望 ${expectedKind} 节点"
    else if !builtins.elem expectedKind scalarKinds then
      fail "scalarValue 只接受标量 kind"
    else
      node.value;

  parse = text: validate (builtins.fromJSON text);

  fromFile = path: parse (builtins.readFile path);
in
{
  inherit
    parse
    fromFile
    validate
    decode
    field
    scalarValue
    ;
}
