{
  fetchurl,
  gnutar,
  gzip,
  runCommand,
  stdenvNoCC,
}:
let
  workspace = builtins.fromTOML (builtins.readFile ../Cargo.toml);
  version = workspace.workspace.package.version;
  system = stdenvNoCC.hostPlatform.system;
  supportedSystems = import ./systems.nix;
  supportedSystem = import ./assert-supported-system.nix system;
  indexPath = ./release-assets.json;
  indexExists = builtins.pathExists indexPath;
  index = if indexExists then builtins.fromJSON (builtins.readFile indexPath) else { };
  fail = message: throw "typol-cli release index: ${message}";
  require = condition: message: if condition then true else fail message;
  indexKeysValid =
    !indexExists
    || require (
      builtins.attrNames index == [
        "assets"
        "tag"
        "version"
      ]
    ) "expected version, tag, and assets fields";
  versionValid =
    !indexExists
    || require (
      builtins.isString index.version
      &&
        builtins.match "[0-9]+\\.[0-9]+\\.[0-9]+(-[A-Za-z0-9.-]+)?(\\+[A-Za-z0-9.-]+)?" index.version
        != null
    ) "invalid release version";
  tagValid =
    !indexExists || require (index.tag == "typol-cli-v${index.version}") "tag does not match version";
  assetSystems = import ./systems.nix;
  assetSystemsValid =
    !indexExists
    || require (
      builtins.isAttrs index.assets
      && builtins.attrNames index.assets == builtins.sort builtins.lessThan assetSystems
    ) "assets must contain exactly the supported Nix systems";
  assetValid =
    systemName:
    let
      asset = index.assets.${systemName};
    in
    require (
      builtins.isAttrs asset
      &&
        builtins.attrNames asset == [
          "archive"
          "hash"
        ]
    ) "${systemName} asset must contain archive and hash"
    && require (
      builtins.isString asset.archive
      && builtins.stringLength asset.archive >= builtins.stringLength "-v${index.version}.tar.gz"
      && builtins.substring 0 10 asset.archive == "typol-cli-"
      &&
        builtins.substring (
          builtins.stringLength asset.archive - builtins.stringLength "-v${index.version}.tar.gz"
        ) (builtins.stringLength "-v${index.version}.tar.gz") asset.archive == "-v${index.version}.tar.gz"
    ) "${systemName} archive name does not match the index version"
    && require (
      builtins.isString asset.hash && builtins.match "sha256-[A-Za-z0-9+/]{43}=" asset.hash != null
    ) "${systemName} hash is not a valid SRI SHA-256";
  validIndex =
    !indexExists
    || (
      indexKeysValid
      && versionValid
      && tagValid
      && assetSystemsValid
      && builtins.all assetValid assetSystems
    );
  currentRelease = indexExists && builtins.seq validIndex (index.version == version);
  unavailable =
    runCommand "typol-cli-${version}-no-nix-release" { } ''
      echo "No usable Nix Release is available for typol-cli ${version}." >&2
      echo "Select typol-cli-source to build from the repository source." >&2
      exit 1
    ''
    // {
      pname = "typol-cli";
      inherit version;
      meta = {
        description = "Typol CLI release package";
        mainProgram = "typol";
        platforms = supportedSystems;
      };
    };
  releaseAsset = if indexExists then index.assets.${system} else { };
  archive = if currentRelease then releaseAsset.archive else "";
in
builtins.seq supportedSystem (
  if indexExists then
    builtins.seq validIndex (
      if currentRelease then
        stdenvNoCC.mkDerivation {
          pname = "typol-cli";
          inherit version;
          src = fetchurl {
            name = archive;
            url = "https://github.com/shaogme/typol/releases/download/${index.tag}/${archive}";
            hash = releaseAsset.hash;
          };
          dontUnpack = true;
          nativeBuildInputs = [
            gnutar
            gzip
          ];
          installPhase = ''
            mkdir -p "$out/bin"
            tar -xzf "$src" --strip-components=1 -C "$TMPDIR"
            install -m 0555 "$TMPDIR/typol" "$out/bin/typol"
          '';
          meta = {
            description = "Typol configuration validation and typed JSON IR export tool";
            mainProgram = "typol";
            platforms = supportedSystems;
          };
        }
      else
        unavailable
    )
  else
    unavailable
)
