import base64
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

import release_assets


class ReleaseAssetsTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.checksums = self.root / "SHA256SUMS"
        lines = []
        for target in release_assets.SYSTEM_TARGETS.values():
            archive = f"typol-cli-{target}-v1.2.3.tar.gz"
            lines.append(f"{hashlib.sha256(archive.encode()).hexdigest()} *{archive}")
        self.checksums.write_text("\n".join(lines) + "\n")

    def tearDown(self):
        self.directory.cleanup()

    def test_generated_index_uses_only_supported_systems_and_sri_hashes(self):
        index = release_assets.create_index("typol-cli-v1.2.3", self.checksums)
        self.assertEqual(set(index["assets"]), set(release_assets.SYSTEM_TARGETS))
        self.assertNotIn("x86_64-darwin", index["assets"])
        path = self.root / "release-assets.json"
        path.write_text(json.dumps(index))
        self.assertEqual(release_assets.validate_index(path), index)

    def test_wrong_archive_mapping_is_rejected(self):
        index = release_assets.create_index("typol-cli-v1.2.3", self.checksums)
        index["assets"]["x86_64-linux"]["archive"] = (
            "typol-cli-aarch64-unknown-linux-musl-v1.2.3.tar.gz"
        )
        path = self.root / "release-assets.json"
        path.write_text(json.dumps(index))
        with self.assertRaisesRegex(ValueError, "archive for x86_64-linux"):
            release_assets.validate_index(path)

    def test_missing_hash_is_rejected(self):
        index = release_assets.create_index("typol-cli-v1.2.3", self.checksums)
        del index["assets"]["aarch64-linux"]["hash"]
        path = self.root / "release-assets.json"
        path.write_text(json.dumps(index))
        with self.assertRaisesRegex(ValueError, "archive and hash"):
            release_assets.validate_index(path)

    def test_malformed_hash_is_rejected(self):
        index = release_assets.create_index("typol-cli-v1.2.3", self.checksums)
        index["assets"]["aarch64-darwin"]["hash"] = ""
        path = self.root / "release-assets.json"
        path.write_text(json.dumps(index))
        with self.assertRaisesRegex(ValueError, "valid SRI SHA-256"):
            release_assets.validate_index(path)

    def test_noncanonical_base64_hash_is_rejected(self):
        index = release_assets.create_index("typol-cli-v1.2.3", self.checksums)
        encoded = index["assets"]["x86_64-linux"]["hash"]
        index["assets"]["x86_64-linux"]["hash"] = encoded[:-2] + "B="
        path = self.root / "release-assets.json"
        path.write_text(json.dumps(index))
        with self.assertRaisesRegex(ValueError, "valid SRI SHA-256"):
            release_assets.validate_index(path)

    def test_generated_hashes_match_release_checksums(self):
        index = release_assets.create_index("typol-cli-v1.2.3", self.checksums)
        release_assets.validate_checksums(index, self.checksums)

    def test_checksum_mismatch_is_rejected(self):
        index = release_assets.create_index("typol-cli-v1.2.3", self.checksums)
        wrong_digest = hashlib.sha256(b"different archive bytes").digest()
        index["assets"]["x86_64-linux"]["hash"] = (
            "sha256-" + base64.b64encode(wrong_digest).decode("ascii")
        )
        with self.assertRaisesRegex(ValueError, "does not match SHA256SUMS"):
            release_assets.validate_checksums(index, self.checksums)

    def test_older_release_cannot_replace_newer_index(self):
        older = self.root / "older.json"
        newer = self.root / "newer.json"
        older.write_text(
            json.dumps(release_assets.create_index("typol-cli-v1.2.3", self.checksums))
        )
        newer_checksums = self.root / "newer-checksums"
        lines = []
        for target in release_assets.SYSTEM_TARGETS.values():
            archive = f"typol-cli-{target}-v1.3.0.tar.gz"
            lines.append(f"{hashlib.sha256(archive.encode()).hexdigest()} *{archive}")
        newer_checksums.write_text("\n".join(lines) + "\n")
        newer.write_text(
            json.dumps(release_assets.create_index("typol-cli-v1.3.0", newer_checksums))
        )
        with self.assertRaisesRegex(ValueError, "Refusing to replace"):
            release_assets.ensure_not_older(older, newer)


if __name__ == "__main__":
    unittest.main()
