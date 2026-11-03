let
  formatName = "typol-config-ir";
  supportedVersion = 1;
  defaultMaxBytes = 16777216;
  defaultMaxDepth = 128;
  defaultMaxNodes = 100000;
  hardMaxBytes = 16777216;
  hardMaxDepth = 128;
  hardMaxNodes = 100000;
  hardMaxJsonDepth = 512;

  fail = message: throw "typolConfigIr: ${message}";

  require = condition: message: if condition then true else fail message;

  isNonEmptyString = value: builtins.isString value && value != "";

  hasExactKeys =
    value: keys:
    builtins.isAttrs value && builtins.attrNames value == builtins.sort builtins.lessThan keys;

  requireExactKeys =
    value: keys: path:
    require (hasExactKeys value keys) "${path} must have exactly these members: ${builtins.concatStringsSep ", " keys}";

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

  requireUnique = values: label: require (!hasDuplicates values) "Duplicate ${label}";

  limits =
    options:
    let
      supplied = if builtins.isAttrs options then options else fail "limits must be an attribute set";
      names = builtins.attrNames supplied;
      known = [
        "maxBytes"
        "maxDepth"
        "maxNodes"
      ];
      checkNames = require (builtins.all (
        name: builtins.elem name known
      ) names) "limits contains an unknown option";
      result = {
        maxBytes = supplied.maxBytes or defaultMaxBytes;
        maxDepth = supplied.maxDepth or defaultMaxDepth;
        maxNodes = supplied.maxNodes or defaultMaxNodes;
      };
      checkValue =
        name: maximum:
        require (
          builtins.isInt result.${name} && result.${name} > 0 && result.${name} <= maximum
        ) "limits.${name} must be a positive integer no greater than ${toString maximum}";
    in
    if
      checkNames
      && checkValue "maxBytes" hardMaxBytes
      && checkValue "maxDepth" hardMaxDepth
      && checkValue "maxNodes" hardMaxNodes
    then
      result
    else
      fail "Invalid input limits";

  jsonTokens =
    text:
    let
      pattern = ''("([^"\\]|\\.)*"|[][{}:,]|-?(0|[1-9][0-9]*)(\.[0-9]+)?([eE][+-]?[0-9]+)?|true|false|null|[[:space:]]+)'';
      pieces = builtins.split pattern text;
      normalize =
        piece:
        if builtins.isList piece then
          let
            token = builtins.head piece;
          in
          if builtins.match "[[:space:]]+" token != null then null else token
        else if piece == "" then
          null
        else
          fail "$: invalid JSON token";
    in
    map normalize pieces;

  nextToken =
    tokens:
    if tokens == [ ] then
      null
    else if builtins.isNull (builtins.head tokens) then
      nextToken (builtins.tail tokens)
    else
      builtins.head tokens;

  scanJson =
    text: configuredLimits:
    let
      inputLimits = limits configuredLimits;
      byteLength = builtins.stringLength text;
      tokens = jsonTokens text;
      scan =
        remaining: state:
        if remaining == [ ] then
          require (state.depth == 0) "$: JSON nesting is incomplete"
          && require (state.nodes <= inputLimits.maxNodes) "$: JSON node budget exceeded"
        else
          let
            stepCheck = require (
              state.steps < inputLimits.maxNodes * 8
            ) "$: JSON token processing budget exceeded";
            token = builtins.head remaining;
            rest = builtins.tail remaining;
            isString = builtins.isString token && builtins.substring 0 1 token == "\"";
            isKey = isString && nextToken rest == ":";
            current = if state.stack == [ ] then null else builtins.head state.stack;
            decodedKey =
              if isKey then
                builtins.tryEval (builtins.fromJSON token)
              else
                {
                  success = true;
                  value = null;
                };
            keyCheck =
              if !isKey then
                true
              else if !decodedKey.success then
                fail "$: invalid JSON string key"
              else if !builtins.isAttrs current || current.kind != "object" then
                true
              else if builtins.hasAttr decodedKey.value current.keys then
                fail "$: duplicate JSON object key at token ${toString state.nodes}"
              else
                true;
            nextStack =
              if token == "{" || token == "[" then
                [
                  {
                    kind = if token == "{" then "object" else "array";
                    keys = { };
                  }
                ]
                ++ state.stack
              else if token == "}" || token == "]" then
                if state.stack == [ ] then state.stack else builtins.tail state.stack
              else if isKey && builtins.isAttrs current && current.kind == "object" then
                [
                  (
                    current
                    // {
                      keys = current.keys // {
                        "${decodedKey.value}" = true;
                      };
                    }
                  )
                ]
                ++ builtins.tail state.stack
              else
                state.stack;
            nextDepth =
              if token == "{" || token == "[" then
                state.depth + 1
              else if token == "}" || token == "]" then
                state.depth - 1
              else
                state.depth;
            containerCheck =
              if token == "{" || token == "[" then
                require (state.depth < hardMaxJsonDepth) "$: JSON nesting exceeds the hard structural limit"
              else if token == "}" || token == "]" then
                require (state.stack != [ ]) "$: invalid JSON container close"
                && require (
                  current.kind == (if token == "}" then "object" else "array")
                ) "$: mismatched JSON container close"
              else
                true;
            isValue =
              !builtins.isNull token
              && token != "{"
              && token != "["
              && token != "}"
              && token != "]"
              && token != ":"
              && token != ","
              && !isKey;
            nextState = {
              stack = nextStack;
              depth = nextDepth;
              nodes = state.nodes + (if isValue || token == "{" || token == "[" then 1 else 0);
              steps = state.steps + 1;
            };
          in
          if stepCheck && keyCheck && containerCheck then scan rest nextState else false;
    in
    require (byteLength <= inputLimits.maxBytes) "$: JSON byte limit exceeded"
    && scan tokens {
      stack = [ ];
      depth = 0;
      nodes = 0;
      steps = 0;
    };

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

  integerKinds = [
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

  floatKinds = [
    "f32"
    "f64"
  ];

  zeros = count: builtins.concatStringsSep "" (builtins.genList (_: "0") count);
  floatBounds = {
    f32 = {
      minimum = "0.${zeros 44}1";
      maximum = "34028235${zeros 31}";
    };
    f64 = {
      minimum = "0.${zeros 323}5";
      maximum = "17976931348623157${zeros 292}";
    };
  };

  decimalParts =
    value:
    let
      parts = builtins.match "([0-9]+)(\\.([0-9]+))?" value;
    in
    {
      whole = builtins.elemAt parts 0;
      fraction = if builtins.isNull (builtins.elemAt parts 2) then "" else builtins.elemAt parts 2;
    };

  floatDecimalLess =
    left: right:
    let
      leftParts = decimalParts left;
      rightParts = decimalParts right;
      leftWholeLength = builtins.stringLength leftParts.whole;
      rightWholeLength = builtins.stringLength rightParts.whole;
      leftFractionLength = builtins.stringLength leftParts.fraction;
      rightFractionLength = builtins.stringLength rightParts.fraction;
      fractionLength =
        if leftFractionLength > rightFractionLength then leftFractionLength else rightFractionLength;
      leftFraction =
        leftParts.fraction + zeros (fractionLength - builtins.stringLength leftParts.fraction);
      rightFraction =
        rightParts.fraction + zeros (fractionLength - builtins.stringLength rightParts.fraction);
    in
    if leftWholeLength != rightWholeLength then
      leftWholeLength < rightWholeLength
    else if leftParts.whole != rightParts.whole then
      leftParts.whole < rightParts.whole
    else
      leftFraction < rightFraction;

  floatWithinRange =
    kind: value:
    if value == "-0" || value == "0" then
      true
    else
      let
        negative = builtins.substring 0 1 value == "-";
        magnitude =
          if negative then builtins.substring 1 (builtins.stringLength value - 1) value else value;
        bounds = floatBounds.${kind};
      in
      !floatDecimalLess bounds.maximum magnitude && !floatDecimalLess magnitude bounds.minimum;

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
      fail "${path} must be a type descriptor"
    else if !builtins.hasAttr "kind" descriptor || !builtins.isString descriptor.kind then
      fail "${path}.kind is missing or is not a string"
    else
      let
        kind = descriptor.kind;
        exactKeys =
          if builtins.elem kind scalarKinds then
            [ "kind" ]
          else if kind == "option" then
            [
              "inner"
              "kind"
            ]
          else if kind == "array" then
            [
              "element"
              "kind"
              "length"
            ]
          else if kind == "map" then
            [
              "kind"
              "value"
            ]
          else if kind == "struct" || kind == "enum" then
            [
              "kind"
              "name"
            ]
          else
            null;
        shapeCheck =
          if builtins.isNull exactKeys then
            fail "Unsupported ${path}.kind: ${kind}"
          else
            requireExactKeys descriptor exactKeys path;
      in
      shapeCheck
      && (
        if builtins.elem kind scalarKinds then
          true
        else if kind == "option" then
          validateType descriptor.inner "${path}.inner"
        else if kind == "array" then
          validateType descriptor.element "${path}.element"
          && require (
            builtins.isNull descriptor.length || (builtins.isInt descriptor.length && descriptor.length >= 0)
          ) "${path}.length must be null or a non-negative integer"
        else if kind == "map" then
          validateType descriptor.value "${path}.value"
        else
          require (isNonEmptyString descriptor.name) "${path}.name must not be empty"
      );

  validateField =
    field: path:
    if !builtins.isAttrs field then
      fail "${path} must be an object"
    else
      requireExactKeys field [ "name" "value" ] path
      && require (isNonEmptyString (field.name or null)) "${path}.name is missing or empty"
      && validateNode field.value "${path}.${field.name}";

  validateFields =
    fields: path:
    if !builtins.isList fields then
      fail "${path} must be an array of fields"
    else
      let
        fieldChecks = builtins.genList (
          index: validateField (builtins.elemAt fields index) "${path}[${toString index}]"
        ) (builtins.length fields);
        shapeCheck = builtins.all (check: check) fieldChecks;
        fieldNames = map (field: field.name) fields;
      in
      shapeCheck && requireUnique fieldNames "field name in ${path}";

  validateMapEntry =
    entry: path:
    if !builtins.isAttrs entry then
      fail "${path} must be an object"
    else
      requireExactKeys entry [ "key" "value" ] path
      && require (builtins.isString (entry.key or null)) "${path}.key must be a string"
      && validateNode entry.value "${path}.value";

  validateMapEntries =
    entries: path:
    if !builtins.isList entries then
      fail "${path} must be an array of map entries"
    else
      let
        entryChecks = builtins.genList (
          index: validateMapEntry (builtins.elemAt entries index) "${path}[${toString index}]"
        ) (builtins.length entries);
        shapeCheck = builtins.all (check: check) entryChecks;
        entryKeys = map (entry: entry.key) entries;
      in
      shapeCheck && requireUnique entryKeys "map key in ${path}";

  validateEnumPayload =
    payload: path:
    if
      !builtins.isAttrs payload || !builtins.hasAttr "kind" payload || !builtins.isString payload.kind
    then
      fail "${path} must contain a valid kind"
    else if payload.kind == "unit" then
      requireExactKeys payload [ "kind" ] path
    else if payload.kind == "single" then
      requireExactKeys payload [ "kind" "value" ] path && validateNode payload.value "${path}.value"
    else if payload.kind == "struct" then
      requireExactKeys payload [ "fields" "kind" ] path && validateFields payload.fields "${path}.fields"
    else
      fail "Unsupported ${path}.kind: ${payload.kind}";

  validateNode =
    node: path:
    if !builtins.isAttrs node then
      fail "${path} must be a node object"
    else if !builtins.hasAttr "type" node then
      fail "${path}.type is missing"
    else
      let
        typeCheck = validateType node.type "${path}.type";
        kind = if builtins.isAttrs node.type then node.type.kind or "" else "";
        hasValue = builtins.hasAttr "value" node;
        value = if hasValue then node.value else null;
        nodeKeys =
          if builtins.elem kind scalarKinds || kind == "option" then
            [
              "type"
              "value"
            ]
          else if kind == "array" then
            [
              "items"
              "type"
            ]
          else if kind == "map" then
            [
              "entries"
              "type"
            ]
          else if kind == "struct" then
            [
              "fields"
              "type"
            ]
          else if kind == "enum" then
            [
              "payload"
              "type"
              "variant"
            ]
          else
            [ "type" ];
        exactNodeKeys = nodeKeys ++ (if builtins.hasAttr "secret" node then [ "secret" ] else [ ]);
        shapeCheck = requireExactKeys node exactNodeKeys path;
        secretCheck = !builtins.hasAttr "secret" node || node.secret == true;
        validScalar =
          if kind == "bool" then
            hasValue && builtins.isBool value
          else if kind == "string" then
            hasValue && builtins.isString value
          else if builtins.elem kind integerKinds then
            hasValue
            && builtins.isString value
            && value != "-0"
            && builtins.match "-?(0|[1-9][0-9]*)" value != null
            && integerInRange kind value
          else if builtins.elem kind floatKinds then
            hasValue
            && builtins.isString value
            && builtins.match "-?(0|[1-9][0-9]*)(\\.[0-9]*[1-9])?" value != null
            && floatWithinRange kind value
          else
            true;
        payloadCheck =
          if kind == "option" then
            hasValue
            && (
              builtins.isNull value
              || (
                validateNode value "${path}.value"
                && require (value.type == node.type.inner) "${path} option child node type does not match"
              )
            )
          else if kind == "array" then
            builtins.isList node.items
            && (builtins.isNull node.type.length || builtins.length node.items == node.type.length)
            && builtins.all (
              item:
              validateNode item "${path}.items"
              && require (item.type == node.type.element) "${path} array element type does not match"
            ) node.items
          else if kind == "map" then
            validateMapEntries node.entries "${path}.entries"
            && builtins.all (
              entry: require (entry.value.type == node.type.value) "${path} map value type does not match"
            ) node.entries
          else if kind == "struct" then
            validateFields node.fields "${path}.fields"
          else if kind == "enum" then
            require (isNonEmptyString (node.variant or null)) "${path}.variant is missing or empty"
            && validateEnumPayload node.payload "${path}.payload"
          else
            true;
      in
      shapeCheck
      && typeCheck
      && require secretCheck "${path}.secret may only have the value true"
      && require validScalar "${path} scalar payload does not match its type"
      && require payloadCheck "${path} payload shape does not match its type";

  measureType =
    inputLimits: descriptor: path: depth: nodes:
    if depth > inputLimits.maxDepth then
      fail "${path}: type nesting depth limit exceeded"
    else if !builtins.isAttrs descriptor || !builtins.isString (descriptor.kind or null) then
      nodes + 1
    else
      let
        nextNodes = nodes + 1;
        checkNodes = require (nextNodes <= inputLimits.maxNodes) "${path}: IR node budget exceeded";
        kind = descriptor.kind;
      in
      if !checkNodes then
        nextNodes
      else if kind == "option" && builtins.hasAttr "inner" descriptor then
        measureType inputLimits descriptor.inner "${path}.inner" (depth + 1) nextNodes
      else if kind == "array" && builtins.hasAttr "element" descriptor then
        measureType inputLimits descriptor.element "${path}.element" (depth + 1) nextNodes
      else if kind == "map" && builtins.hasAttr "value" descriptor then
        measureType inputLimits descriptor.value "${path}.value" (depth + 1) nextNodes
      else
        nextNodes;

  measureList =
    inputLimits: items: path: depth: nodes: selectItem:
    if !builtins.isList items then
      nodes
    else
      let
        walk =
          remaining: index: currentNodes:
          if remaining == [ ] then
            currentNodes
          else
            walk (builtins.tail remaining) (index + 1) (
              selectItem (builtins.head remaining) "${path}[${toString index}]" depth currentNodes
            );
      in
      walk items 0 nodes;

  measureNode =
    inputLimits: node: path: depth: nodes:
    if depth > inputLimits.maxDepth then
      fail "${path}: value nesting depth limit exceeded"
    else
      let
        nextNodes = nodes + 1;
        checkNodes = require (nextNodes <= inputLimits.maxNodes) "${path}: IR node budget exceeded";
        hasType = builtins.isAttrs node && builtins.hasAttr "type" node;
        measuredType =
          if hasType then measureType inputLimits node.type "${path}.type" 1 nextNodes else nextNodes;
        kind = if hasType && builtins.isAttrs node.type then node.type.kind or "" else "";
        walkChild =
          child: childPath: childDepth: childNodes:
          measureNode inputLimits child childPath (childDepth + 1) childNodes;
        walkField =
          field: fieldPath: childDepth: childNodes:
          if builtins.isAttrs field && builtins.hasAttr "value" field then
            walkChild field.value "${path}.${field.name or "?"}" childDepth childNodes
          else
            childNodes;
        walkEntry =
          entry: entryPath: childDepth: childNodes:
          if builtins.isAttrs entry && builtins.hasAttr "value" entry then
            walkChild entry.value "${entryPath}.value" childDepth childNodes
          else
            childNodes;
        afterType =
          if kind == "option" && builtins.hasAttr "value" node && !builtins.isNull node.value then
            walkChild node.value "${path}.value" depth measuredType
          else if kind == "array" then
            measureList inputLimits (node.items or null) "${path}.items" depth measuredType walkChild
          else if kind == "map" then
            measureList inputLimits (node.entries or null) "${path}.entries" depth measuredType walkEntry
          else if kind == "struct" then
            measureList inputLimits (node.fields or null) "${path}.fields" depth measuredType walkField
          else if kind == "enum" && builtins.isAttrs (node.payload or null) then
            if node.payload.kind or "" == "single" && builtins.hasAttr "value" node.payload then
              walkChild node.payload.value "${path}.payload.value" depth measuredType
            else if node.payload.kind or "" == "struct" then
              measureList inputLimits (node.payload.fields or null
              ) "${path}.payload.fields" depth measuredType walkField
            else
              measuredType
          else
            measuredType;
      in
      if checkNodes then afterType else nextNodes;

  validate =
    configuredLimits: envelope:
    if !builtins.isAttrs envelope then
      fail "IR envelope must be an object"
    else
      let
        inputLimits = limits configuredLimits;
        budgetCheck = measureNode inputLimits (envelope.value or null) "value" 1 0;
        headerCheck =
          requireExactKeys envelope [ "format" "root_type" "value" "version" ] "IR envelope"
          && require ((envelope.format or null) == formatName) "format must be ${formatName}"
          && require ((envelope.version or null) == supportedVersion) "Unsupported IR version"
          && require (isNonEmptyString (envelope.root_type or null)) "root_type is missing or empty";
        nodeCheck =
          validateNode envelope.value "value"
          && require (envelope.value.type.kind == "struct") "Root node must be a struct"
          && require (
            envelope.value.type.name == envelope.root_type
          ) "root_type does not match the root node type name";
      in
      if budgetCheck <= inputLimits.maxNodes && headerCheck && nodeCheck then
        envelope
      else
        fail "Invalid IR envelope";

  pathField = path: name: "${path}.${name}";
  pathIndex = path: index: "${path}[${toString index}]";
  pathKey = path: "${path}[<map-key>]";

  decodeFields =
    fields: path: inheritedSecret:
    builtins.listToAttrs (
      map (field: {
        name = field.name;
        value = decodeNode field.value (pathField path field.name) inheritedSecret;
      }) fields
    );

  decodeNode =
    node: path: inheritedSecret:
    let
      kind = node.type.kind;
      effectiveSecret = inheritedSecret || (node.secret or false);
      indexed =
        values: makePath:
        builtins.genList (
          index: decodeNode (builtins.elemAt values index) (makePath index) effectiveSecret
        ) (builtins.length values);
      decodedValue =
        if kind == "array" then
          indexed node.items (pathIndex path)
        else if kind == "map" then
          builtins.listToAttrs (
            builtins.genList (
              index:
              let
                entry = builtins.elemAt node.entries index;
              in
              {
                name = entry.key;
                value = decodeNode entry.value (pathIndex path index) effectiveSecret;
              }
            ) (builtins.length node.entries)
          )
        else if kind == "struct" then
          decodeFields node.fields path effectiveSecret
        else if kind == "enum" then
          {
            variant = node.variant;
            payload =
              if node.payload.kind == "single" then
                {
                  kind = "single";
                  value = decodeNode node.payload.value (pathField path "payload") effectiveSecret;
                }
              else if node.payload.kind == "struct" then
                {
                  kind = "struct";
                  fields = decodeFields node.payload.fields (pathField path "payload") effectiveSecret;
                }
              else
                { kind = "unit"; };
          }
        else if kind == "option" && !builtins.isNull node.value then
          decodeNode node.value path effectiveSecret
        else
          node.value;
    in
    {
      type = node.type;
      secret = effectiveSecret;
      inherit path;
      value = decodedValue;
    };

  decode =
    configuredLimits: envelope:
    let
      checked = validate configuredLimits envelope;
    in
    {
      rootType = checked.root_type;
      value = decodeNode checked.value "$" false;
    };

  projectionOptions =
    options:
    if !builtins.isAttrs options then
      fail "options must be an attribute set"
    else if builtins.any (name: name != "includeSecrets") (builtins.attrNames options) then
      fail "unknown projection option"
    else if !builtins.isBool (options.includeSecrets or false) then
      fail "includeSecrets must be a boolean"
    else
      { includeSecrets = options.includeSecrets or false; };

  nodeContainsSecret =
    node:
    let
      kind = node.type.kind;
      value = node.value;
      nestedSecret =
        if kind == "option" then
          !builtins.isNull value && nodeContainsSecret value
        else if kind == "array" then
          builtins.any nodeContainsSecret value
        else if kind == "map" || kind == "struct" then
          builtins.any nodeContainsSecret (builtins.attrValues value)
        else if kind == "enum" then
          if value.payload.kind == "single" then
            nodeContainsSecret value.payload.value
          else if value.payload.kind == "struct" then
            builtins.any nodeContainsSecret (builtins.attrValues value.payload.fields)
          else
            false
        else
          false;
    in
    (node.secret or false) || nestedSecret;

  requireReadable =
    options: node:
    if !builtins.isAttrs node || !(builtins.hasAttr "type" node) then
      fail "$: expected a decoded IR node"
    else if nodeContainsSecret node && !options.includeSecrets then
      fail "${node.path or "$"}: selected value contains a secret node"
    else
      true;

  projectFields = fields: builtins.mapAttrs (_: projectNode) fields;

  projectEnumPayload =
    payload:
    if payload.kind == "single" then
      {
        kind = "single";
        value = projectNode payload.value;
      }
    else if payload.kind == "struct" then
      {
        kind = "struct";
        fields = projectFields payload.fields;
      }
    else
      { kind = "unit"; };

  projectNode =
    node:
    let
      kind = node.type.kind;
      value = node.value;
    in
    if kind == "option" then
      if builtins.isNull value then null else projectNode value
    else if kind == "array" then
      map projectNode value
    else if kind == "map" || kind == "struct" then
      projectFields value
    else if kind == "enum" then
      {
        variant = value.variant;
        payload = projectEnumPayload value.payload;
      }
    else
      value;

  project =
    options: node:
    let
      checkedOptions = projectionOptions options;
    in
    if requireReadable checkedOptions node then projectNode node else null;

  selectorPath =
    node: selector:
    let
      kind = node.type.kind;
      path = node.path or "$";
    in
    if kind == "struct" then
      if !builtins.isString selector then
        fail "${path}: expected a struct field name, got ${builtins.typeOf selector}"
      else
        pathField path selector
    else if kind == "map" then
      if !builtins.isString selector then
        fail "${path}: expected a map key string, got ${builtins.typeOf selector}"
      else
        pathKey path
    else if kind == "array" then
      if !builtins.isInt selector || selector < 0 then
        fail "${path}: expected a non-negative array index, got ${builtins.typeOf selector}"
      else
        pathIndex path selector
    else
      fail "${path}: expected a struct, map, or array, got ${kind}";

  childNode =
    node: selector:
    let
      kind = node.type.kind;
      path = selectorPath node selector;
    in
    if kind == "option" then
      if builtins.isNull node.value then
        fail "${node.path or "$"}: cannot traverse an empty option"
      else
        childNode node.value selector
    else if kind == "array" then
      if !builtins.isInt selector || selector < 0 then
        fail "${node.path or "$"}: expected a non-negative array index"
      else if selector >= builtins.length node.value then
        fail "${path}: array index is out of range"
      else
        builtins.elemAt node.value selector
    else if kind == "struct" || kind == "map" then
      if !builtins.isString selector then
        fail "${node.path or "$"}: expected a string field name or map key"
      else if !builtins.hasAttr selector node.value then
        fail "${path}: path does not exist"
      else
        builtins.getAttr selector node.value
    else
      fail "${path}: cannot select a child from ${kind}";

  at =
    options: root: selectors:
    let
      checkedOptions = projectionOptions options;
      resolve =
        node: remaining:
        if remaining == [ ] then
          if requireReadable checkedOptions node then node else null
        else
          resolve (childNode node (builtins.head remaining)) (builtins.tail remaining);
    in
    if !builtins.isList selectors then
      fail "$: path must be a list of field names, map keys, or array indexes"
    else
      resolve root selectors;

  expectKind =
    options: expected: node:
    if requireReadable options node then
      let
        actual = if builtins.isAttrs node.type then node.type.kind or "unknown" else "unknown";
      in
      if actual == expected then
        node.value
      else
        fail "${node.path or "$"}: expected ${expected}, got ${actual}"
    else
      null;

  readString = options: node: expectKind (projectionOptions options) "string" node;
  readBool = options: node: expectKind (projectionOptions options) "bool" node;
  readFloat =
    options: node:
    let
      actual = node.type.kind or "unknown";
      checkedOptions = projectionOptions options;
    in
    if requireReadable checkedOptions node then
      if builtins.elem actual floatKinds then
        node.value
      else
        fail "${node.path or "$"}: expected f32 or f64, got ${actual}"
    else
      null;
  readInteger =
    options: node:
    let
      actual = node.type.kind or "unknown";
      checkedOptions = projectionOptions options;
    in
    if requireReadable checkedOptions node then
      if builtins.elem actual integerKinds then
        node.value
      else
        fail "${node.path or "$"}: expected an integer kind, got ${actual}"
    else
      null;

  magnitudeLess =
    left: right:
    builtins.stringLength left < builtins.stringLength right
    || (builtins.stringLength left == builtins.stringLength right && left < right);

  decimalLess =
    left: right:
    let
      leftNegative = builtins.substring 0 1 left == "-";
      rightNegative = builtins.substring 0 1 right == "-";
      leftMagnitude =
        if leftNegative then builtins.substring 1 (builtins.stringLength left - 1) left else left;
      rightMagnitude =
        if rightNegative then builtins.substring 1 (builtins.stringLength right - 1) right else right;
      magnitudeComparison = magnitudeLess leftMagnitude rightMagnitude;
    in
    if leftNegative != rightNegative then
      leftNegative
    else if leftNegative then
      !magnitudeComparison && leftMagnitude != rightMagnitude
    else
      magnitudeComparison;

  toInteger =
    options: node:
    let
      checkedOptions = projectionOptions (
        builtins.removeAttrs options [
          "min"
          "max"
        ]
      );
      validBounds =
        builtins.isInt (options.min or null)
        && builtins.isInt (options.max or null)
        && options.min <= options.max;
      actual = node.type.kind or "unknown";
      decimal = if builtins.elem actual integerKinds then node.value else "";
      within =
        validBounds
        && !decimalLess decimal (toString options.min)
        && !decimalLess (toString options.max) decimal;
    in
    if !validBounds then
      fail "${node.path or "$"}: integer conversion requires ordered Nix integer min and max bounds"
    else if requireReadable checkedOptions node then
      if !builtins.elem actual integerKinds then
        fail "${node.path or "$"}: expected an integer kind, got ${actual}"
      else if !within then
        fail "${node.path or "$"}: integer is outside the declared conversion range"
      else
        builtins.fromJSON decimal
    else
      null;

  hasPathPrefix =
    prefix: path:
    let
      samePrefix = builtins.genList (index: builtins.elemAt prefix index == builtins.elemAt path index) (
        builtins.length prefix
      );
    in
    builtins.length prefix <= builtins.length path && builtins.all (value: value) samePrefix;

  insertSelection =
    result: selectors: value:
    if selectors == [ ] then
      value
    else
      let
        key = builtins.head selectors;
        existing = if builtins.hasAttr key result then builtins.getAttr key result else { };
        nested = if builtins.isAttrs existing then existing else fail "select paths overlap at ${key}";
      in
      result // { "${key}" = insertSelection nested (builtins.tail selectors) value; };

  select =
    options: root:
    let
      checkedOptions = projectionOptions (builtins.removeAttrs options [ "paths" ]);
      paths = options.paths or null;
      validPaths =
        builtins.isList paths
        && builtins.all (path: builtins.isList path && builtins.all builtins.isString path) paths;
      duplicateOrOverlap =
        if !validPaths then
          false
        else
          builtins.any (
            index:
            builtins.any (
              otherIndex:
              otherIndex != index
              && (
                hasPathPrefix (builtins.elemAt paths index) (builtins.elemAt paths otherIndex)
                || hasPathPrefix (builtins.elemAt paths otherIndex) (builtins.elemAt paths index)
              )
            ) (builtins.genList (otherIndex: otherIndex) (builtins.length paths))
          ) (builtins.genList (index: index) (builtins.length paths));
      selectPath =
        path:
        if path == [ ] then
          project { includeSecrets = checkedOptions.includeSecrets; } root
        else
          project { includeSecrets = checkedOptions.includeSecrets; } (
            at { includeSecrets = checkedOptions.includeSecrets; } root path
          );
      build = builtins.foldl' (result: path: insertSelection result path (selectPath path)) { } paths;
    in
    if !validPaths then
      fail "select.paths must be a list of string paths"
    else if duplicateOrOverlap then
      fail "select.paths contains duplicate or overlapping paths"
    else if builtins.any (path: path == [ ]) paths then
      if builtins.length paths == 1 then
        selectPath [ ]
      else
        fail "select cannot combine the root path with child paths"
    else
      build;

  fromText =
    configuredLimits: text:
    let
      checkedLimits = limits configuredLimits;
      scanCheck = scanJson text checkedLimits;
      decoded = builtins.tryEval (builtins.fromJSON text);
    in
    if scanCheck then
      if decoded.success then validate checkedLimits decoded.value else fail "$: invalid JSON text"
    else
      fail "$: invalid JSON text";

  fromFile = configuredLimits: path: fromText configuredLimits (builtins.readFile path);
in
{
  inherit
    fromText
    fromFile
    validate
    decode
    at
    readString
    readBool
    readInteger
    readFloat
    toInteger
    select
    project
    ;
}
