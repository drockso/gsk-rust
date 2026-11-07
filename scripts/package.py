"""Package a built binary with docs and dependency notices (Python 3.11+).

python scripts/package.py --platform windows-x64 --binary target/release/gsk.exe
python scripts/package.py --platform linux-x64 --binary target/x86_64-unknown-linux-musl/release/gsk

Requires the Cargo dependency cache and rust-docs for the build toolchain.
Archives and standalone binaries are written to ignored artifacts/releases/.
"""
import argparse
import hashlib
import json
import shutil
import subprocess
import tarfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def dependency_notices(target):
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--format-version', '1', '--filter-platform', target],
        cwd=ROOT, encoding='utf-8'))
    active = {node['id'] for node in metadata['resolve']['nodes']}
    parts = ['Third-party Cargo dependency notices\n'
             f'Generated from Cargo.lock for {target}; includes build-time dependencies.\n'
             'Original license texts and attribution follow; no vendor GSK binary is included.\n']
    for package in sorted(metadata['packages'], key=lambda p: p['name']):
        if package['source'] is None or package['id'] not in active:
            continue
        directory = Path(package['manifest_path']).parent
        candidates = sorted(p for p in directory.rglob('*') if p.is_file()
                            and p.name.upper().startswith(('LICENSE', 'COPYING', 'COPYRIGHT', 'NOTICE')))
        if not candidates:
            raise RuntimeError(f"Missing license files for {package['name']}")
        parts.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\n"
                     f"License expression: {package['license']}\n"
                     f"Source: https://crates.io/crates/{package['name']}/{package['version']}\n")
        for path in candidates:
            parts.append(f'\n--- {path.relative_to(directory).as_posix()} ---\n')
            parts.append(path.read_text(encoding='utf-8') + '\n')
    return ''.join(parts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--platform', choices=['windows-x64', 'linux-x64'], required=True)
    parser.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    assert binary.is_file(), binary
    version = tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding='utf-8'))['package']['version']
    output = ROOT / 'artifacts/releases'
    output.mkdir(parents=True, exist_ok=True)
    name = f'gsk-v{version}-{args.platform}'
    stage = output / name
    stage.mkdir(exist_ok=True)
    executable = 'gsk.exe' if args.platform == 'windows-x64' else 'gsk'
    shutil.copy2(binary, stage / executable)
    (stage / executable).chmod(0o755)
    for relative in ('README.md', 'README.ru.md', 'LICENSE', 'CHANGELOG.md',
                     'assets/banner.svg', 'docs/PROTOCOL.md', 'docs/HARDWARE-2026-11-07.md'):
        target = stage / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / relative, target)
    target = 'x86_64-pc-windows-msvc' if args.platform == 'windows-x64' else 'x86_64-unknown-linux-musl'
    (stage / 'THIRD-PARTY-NOTICES.txt').write_text(dependency_notices(target), encoding='utf-8')
    sysroot = Path(subprocess.check_output(['rustc', '--print', 'sysroot'], encoding='utf-8').strip())
    rust_notices = sysroot / 'share/doc/rust/COPYRIGHT-library.html'
    if not rust_notices.is_file():
        raise RuntimeError('Install rust-docs for this toolchain before packaging runtime notices')
    shutil.copy2(rust_notices, stage / 'RUST-STANDARD-LIBRARY-NOTICES.html')
    if args.platform == 'linux-x64':
        shutil.copy2(ROOT / 'licenses/MUSL-COPYRIGHT.txt', stage / 'MUSL-COPYRIGHT.txt')
        archive = output / f'{name}.tar.gz'
        def normalize(info):
            info.uid = info.gid = 0
            info.uname = info.gname = ''
            info.mode = 0o755 if info.isdir() or info.name == f'{name}/gsk' else 0o644
            return info
        with tarfile.open(archive, 'w:gz') as packed:
            packed.add(stage, arcname=name, filter=normalize)
    else:
        archive = output / f'{name}.zip'
        with zipfile.ZipFile(archive, 'w', zipfile.ZIP_DEFLATED) as packed:
            for path in sorted(stage.rglob('*')):
                if path.is_file():
                    packed.write(path, arcname=path.relative_to(output).as_posix())
    standalone = output / ('gsk-windows-x64.exe' if args.platform == 'windows-x64' else 'gsk-linux-x64')
    shutil.copy2(binary, standalone)
    print(archive.name, archive.stat().st_size, 'bytes')
    print(standalone.name, standalone.stat().st_size, 'bytes')
    release_files = sorted(p for p in output.iterdir() if p.is_file() and p.name != 'SHA256SUMS.txt')
    sums = ''.join(f'{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n' for p in release_files)
    (output / 'SHA256SUMS.txt').write_text(sums, encoding='utf-8')


if __name__ == '__main__':
    main()
