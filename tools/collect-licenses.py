"""Collect release notices from locked Cargo sources, including nested native code.

Intentionally includes build/test dependencies in the target's resolved graph:
this is an attribution superset, not a claim that all crates are linked.
No network access is performed beyond Cargo's locked metadata resolution.
"""

import json
from pathlib import Path
import shutil
import subprocess
import sys


def notice_files(root):
    for path in sorted(root.rglob('*')):
        relative = path.relative_to(root)
        if any(part in ('target', '.git') for part in relative.parts):
            continue
        if path.is_file() and path.name.upper().startswith(
            ('LICENSE', 'LICENCE', 'COPYING', 'COPYRIGHT', 'NOTICE')
        ):
            if not path.resolve().is_relative_to(root.resolve()):
                raise RuntimeError(f'License symlink escapes source: {path}')
            yield path


def collect(output):
    root = Path(__file__).resolve().parent.parent
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--format-version', '1',
         '--filter-platform', 'x86_64-unknown-linux-gnu'], cwd=root
    ))
    output.mkdir(parents=True, exist_ok=False)
    index = ['# Third-party notices', '',
             'Locked x86_64 Linux dependency sources, including build/test dependencies.',
             'Each component retains its own license; Fjern itself is MIT licensed.',
             'Nested native-library notices are included. System shared libraries are',
             'distributed separately by the operating system.', '']
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        source = Path(package['manifest_path']).parent
        key = f"{package['name']}-{package['version']}"
        files = list(notice_files(source))
        if package.get('license_file'):
            declared = (source / package['license_file']).resolve()
            if not declared.is_relative_to(source.resolve()):
                raise RuntimeError(f'License file escapes source: {key}')
            if declared not in files:
                files.append(declared)
        supplement = root / 'tools/license-supplements' / key
        extras = list(supplement.rglob('*')) if supplement.exists() else []
        if package['id'] in metadata['workspace_members']:
            files.append(root / 'LICENSE')
        if not files and not extras:
            raise RuntimeError(f'Missing license text: {key}')
        for path in files:
            relative = path.relative_to(source) if path.is_relative_to(source) else Path('LICENSE')
            target = output / key / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
        for path in extras:
            if path.is_file():
                target = output / key / 'supplement' / path.relative_to(supplement)
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, target)
        index += [f"## {key}", '', f"Declared license: {package['license'] or 'see files'}",
                  f"Source: {package.get('repository') or 'workspace source'}", '',
                  f"Notices: `{key}/`", '']
    # Rust's standard library is not represented in Cargo's dependency graph.
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], text=True).strip())
    rust_docs = sysroot / 'share/doc/rust'
    shutil.copytree(rust_docs / 'licenses', output / 'rust-standard-library/licenses')
    shutil.copyfile(rust_docs / 'COPYRIGHT-library.html', output / 'rust-standard-library/COPYRIGHT-library.html')
    index += ['## Rust standard library', '', subprocess.check_output(
        ['rustc', '--version'], text=True).strip(), '', 'Notices: `rust-standard-library/`', '']
    (output / 'README.md').write_text('\n'.join(index))


if __name__ == '__main__':
    collect(Path(sys.argv[1]))
