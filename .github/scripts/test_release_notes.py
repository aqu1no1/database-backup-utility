import tempfile
import unittest
from pathlib import Path

from release_notes import extract


class ReleaseNotesTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / 'Cargo.toml').write_text('[workspace.package]\nversion = "0.1.0"\n')
        for path in ('apps/cli', 'crates/core', 'crates/database-adapters'):
            dest = self.root / path
            dest.mkdir(parents=True)
            (dest / 'Cargo.toml').write_text('[package]\nversion = { workspace = true }\n')

    def tearDown(self):
        self.temp.cleanup()

    def test_extracts_release(self):
        (self.root / 'CHANGELOG.md').write_text(
            '## [Unreleased]\n\n## [0.1.0] - 2026-10-20\n\n### Added\n- CLI inicial.\n'
        )
        self.assertIn('CLI inicial.', extract('v0.1.0', self.root))

    def test_rejects_unreleased_only(self):
        (self.root / 'CHANGELOG.md').write_text('## [Unreleased]\n### Added\n- Nada.\n')
        with self.assertRaises(ValueError):
            extract('v0.1.0', self.root)

    def test_rejects_other_version(self):
        (self.root / 'CHANGELOG.md').write_text(
            '## [0.1.0] - 2026-10-20\n### Added\n- Base.\n'
        )
        with self.assertRaises(ValueError):
            extract('v0.2.0', self.root)


if __name__ == '__main__':
    unittest.main()
