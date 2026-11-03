import argparse
import base64
import binascii
import hashlib
import json
import re
import sys
import tomllib
from pathlib import Path


SYSTEM_TARGETS = {
    "x86_64-linux": "x86_64-unknown-linux-musl",
    "aarch64-linux": "aarch64-unknown-linux-musl",
    "aarch64-darwin": "aarch64-apple-darwin",
}
VERSION_PATTERN = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)
SRI_PATTERN = re.compile(r"^sha256-[A-Za-z0-9+/]{43}=$")


def fail(message):
    raise ValueError(message)


def version_parts(version):
    match = VERSION_PATTERN.fullmatch(version)
    if match is None:
        fail(f"Invalid semantic version: {version}")
    prerelease = match.group(4)
    identifiers = []
    if prerelease is not None:
        for identifier in prerelease.split("."):
            if identifier.isdigit():
                if len(identifier) > 1 and identifier.startswith("0"):
                    fail(f"Invalid numeric prerelease identifier: {identifier}")
                identifiers.append((0, int(identifier)))
            else:
                identifiers.append((1, identifier))
    return (
        int(match.group(1)),
        int(match.group(2)),
        int(match.group(3)),
        (1, ()) if prerelease is None else (0, tuple(identifiers)),
    )


def version_from_tag(tag):
    prefix = "typol-cli-v"
    if not tag.startswith(prefix):
        fail("Expected a tag in the form typol-cli-v<workspace-version>.")
    version = tag[len(prefix) :]
    version_parts(version)
    return version


def workspace_version(path):
    with Path(path).open("rb") as cargo_file:
        manifest = tomllib.load(cargo_file)
    try:
        version = manifest["workspace"]["package"]["version"]
    except (KeyError, TypeError) as error:
        fail(f"Could not read workspace.package.version from {path}: {error}")
    version_parts(version)
    return version


def checksum_entries(path):
    entries = {}
    for line_number, line in enumerate(Path(path).read_text().splitlines(), start=1):
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](.+)", line)
        if match is None:
            fail(f"Malformed SHA256SUMS line {line_number}.")
        digest, filename = match.groups()
        if "/" in filename or "\\" in filename:
            fail(f"SHA256SUMS entry must use a basename: {filename}")
        if filename in entries:
            fail(f"Duplicate SHA256SUMS entry: {filename}")
        entries[filename] = digest.lower()
    return entries


def create_index(tag, checksums_path):
    version = version_from_tag(tag)
    checksums = checksum_entries(checksums_path)
    assets = {}
    for system, target in SYSTEM_TARGETS.items():
        archive = f"typol-cli-{target}-v{version}.tar.gz"
        try:
            digest = checksums[archive]
        except KeyError:
            fail(f"SHA256SUMS is missing the Nix archive {archive}.")
        sri = "sha256-" + base64.b64encode(bytes.fromhex(digest)).decode("ascii")
        assets[system] = {"archive": archive, "hash": sri}
    return {"version": version, "tag": tag, "assets": assets}


def reject_duplicate_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            fail(f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def is_valid_sri(value):
    if not isinstance(value, str) or not SRI_PATTERN.fullmatch(value):
        return False
    encoded_digest = value.removeprefix("sha256-")
    try:
        digest = base64.b64decode(encoded_digest, validate=True)
    except (binascii.Error, ValueError):
        return False
    canonical_digest = base64.b64encode(digest).decode("ascii")
    return len(digest) == hashlib.sha256().digest_size and canonical_digest == encoded_digest


def validate_index(path):
    try:
        index = json.loads(
            Path(path).read_text(), object_pairs_hook=reject_duplicate_keys
        )
    except json.JSONDecodeError as error:
        fail(f"Invalid release index JSON: {error}")
    if not isinstance(index, dict) or set(index) != {"version", "tag", "assets"}:
        fail("Release index must contain exactly version, tag, and assets.")
    version = index["version"]
    if not isinstance(version, str):
        fail("Release index version must be a string.")
    version_parts(version)
    if index["tag"] != f"typol-cli-v{version}":
        fail("Release index tag must match its version.")
    assets = index["assets"]
    if not isinstance(assets, dict) or set(assets) != set(SYSTEM_TARGETS):
        fail("Release index must map exactly the three supported Nix systems.")
    for system, target in SYSTEM_TARGETS.items():
        asset = assets[system]
        if not isinstance(asset, dict) or set(asset) != {"archive", "hash"}:
            fail(f"Release index asset for {system} must contain archive and hash.")
        expected_archive = f"typol-cli-{target}-v{version}.tar.gz"
        if asset["archive"] != expected_archive:
            fail(f"Release index archive for {system} must be {expected_archive}.")
        if not is_valid_sri(asset["hash"]):
            fail(f"Release index hash for {system} is not a valid SRI SHA-256.")
    return index


def validate_checksums(index, checksums_path):
    checksums = checksum_entries(checksums_path)
    for system, asset in index["assets"].items():
        try:
            digest = checksums[asset["archive"]]
        except KeyError:
            fail(f"SHA256SUMS is missing {asset['archive']}.")
        actual = "sha256-" + base64.b64encode(bytes.fromhex(digest)).decode("ascii")
        if actual != asset["hash"]:
            fail(f"Release index hash for {system} does not match SHA256SUMS.")


def ensure_not_older(candidate_path, existing_path):
    candidate = validate_index(candidate_path)
    existing = validate_index(existing_path)
    if version_parts(candidate["version"]) < version_parts(existing["version"]):
        fail(
            f"Refusing to replace Nix Release {existing['version']} with older "
            f"Release {candidate['version']}."
        )


def main():
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    check_tag = subparsers.add_parser("check-tag")
    check_tag.add_argument("--tag", required=True)
    check_tag.add_argument("--workspace", default="Cargo.toml")

    generate = subparsers.add_parser("generate")
    generate.add_argument("--tag", required=True)
    generate.add_argument("--checksums", required=True)
    generate.add_argument("--output", required=True)

    validate = subparsers.add_parser("validate")
    validate.add_argument("--index", default="nix/release-assets.json")
    validate.add_argument("--checksums")

    current = subparsers.add_parser("has-current-release")
    current.add_argument("--index", default="nix/release-assets.json")
    current.add_argument("--workspace", default="Cargo.toml")

    compare = subparsers.add_parser("ensure-not-older")
    compare.add_argument("--candidate", required=True)
    compare.add_argument("--existing", required=True)

    arguments = parser.parse_args()
    try:
        if arguments.command == "check-tag":
            tag_version = version_from_tag(arguments.tag)
            source_version = workspace_version(arguments.workspace)
            if tag_version != source_version:
                fail(
                    f"Release tag version {tag_version} does not match workspace "
                    f"version {source_version}."
                )
        elif arguments.command == "generate":
            index = create_index(arguments.tag, arguments.checksums)
            Path(arguments.output).write_text(
                json.dumps(index, indent=2, sort_keys=True) + "\n"
            )
        elif arguments.command == "validate":
            index = validate_index(arguments.index)
            if arguments.checksums:
                validate_checksums(index, arguments.checksums)
        elif arguments.command == "has-current-release":
            index = validate_index(arguments.index)
            print(str(index["version"] == workspace_version(arguments.workspace)).lower())
        else:
            ensure_not_older(arguments.candidate, arguments.existing)
    except (OSError, ValueError) as error:
        print(f"release-assets: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
