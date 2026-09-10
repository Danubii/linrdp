import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    'licenses', Path(__file__).with_name('collect-licenses.py'))
licenses = importlib.util.module_from_spec(spec)
spec.loader.exec_module(licenses)


class LicenseTests(unittest.TestCase):
    def test_nested_native_notices_and_case_variants(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('LICENSE', 'native/codec/COPYING.txt', 'nested/Notice.md',
                         'target/LICENSE', '.git/LICENSE', 'src/code.rs'):
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text('fixture')
            self.assertEqual(
                {str(p.relative_to(root)) for p in licenses.notice_files(root)},
                {'LICENSE', 'native/codec/COPYING.txt', 'nested/Notice.md'})

    def test_external_symlink_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'outside').write_text('fixture')
            (root / 'source').mkdir()
            (root / 'source/LICENSE').symlink_to(root / 'outside')
            with self.assertRaisesRegex(RuntimeError, 'escapes source'):
                list(licenses.notice_files(root / 'source'))

    def test_missing_notice_stops_release(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'source').mkdir()
            metadata = {'workspace_members': [], 'packages': [{
                'name': 'missing-license-fixture', 'version': '1.0.0',
                'manifest_path': str(root / 'source/Cargo.toml'),
                'id': 'fixture', 'license': 'MIT',
            }]}
            with patch.object(licenses.subprocess, 'check_output',
                              return_value=json.dumps(metadata).encode()):
                with self.assertRaisesRegex(RuntimeError, 'Missing license text'):
                    licenses.collect(root / 'output')


if __name__ == '__main__':
    unittest.main()
