{
  pkgs,
  typolCli,
  sourceDir,
  schemaPath,
  configPath,
  name ? "typol-config-ir",
  environment ? "empty",
}:
let
  lib = pkgs.lib;
  normalizedPath = path: builtins.replaceStrings [ "\\" ] [ "/" ] path;
  safeRelativePath =
    label: path:
    let
      normalized = normalizedPath path;
      pieces = lib.splitString "/" normalized;
      valid =
        builtins.isString path
        && normalized != ""
        && builtins.substring 0 1 normalized != "/"
        && builtins.match "[A-Za-z]:.*" normalized == null
        && builtins.all (piece: piece != "" && piece != "." && piece != "..") pieces;
    in
    if valid then
      path
    else
      throw "typolConfigIr: ${label} must be a non-empty relative path without parent components";
  schema = safeRelativePath "schemaPath" schemaPath;
  config = safeRelativePath "configPath" configPath;
  source = builtins.path {
    path = sourceDir;
    name = "${name}-source";
  };
in
assert builtins.isPath sourceDir;
assert import ../assert-supported-system.nix pkgs.stdenv.hostPlatform.system;
assert environment == "empty";
pkgs.runCommand name { src = source; } ''
  set -eu
  cd "$src"
  ${typolCli}/bin/typol config export-ir \
    --schema ${lib.escapeShellArg schema} \
    --environment empty \
    --reject-secrets \
    ${lib.escapeShellArg config} > "$TMPDIR/config-ir.json"
  mkdir -p "$out"
  install -m 0444 "$TMPDIR/config-ir.json" "$out/config-ir.json"
''
