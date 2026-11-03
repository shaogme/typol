system:
let
  supportedSystems = import ./systems.nix;
in
if builtins.elem system supportedSystems then
  true
else
  throw "Unsupported Nix system '${system}'. Supported systems: ${builtins.concatStringsSep ", " supportedSystems}."
