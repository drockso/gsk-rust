"""Explicit opt-in hardware test for the user's authorized CNC and test names.

Writes ONLY O3996.CNC and O3997.CNC, refusing existing names before any upload.
Programs contain comments and M30 only; no execution command is sent.
python -X utf8 research/hardware_test.py --ip 192.168.1.100 --run
"""
import argparse
import hashlib
import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXE = ROOT / 'target/release/gsk.exe'


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ip', required=True, help='CNC IPv4 address; writes O3996 and O3997')
    parser.add_argument('--binary', type=Path, default=EXE)
    parser.add_argument('--run', action='store_true', required=True)
    args = parser.parse_args()
    IP = args.ip
    executable = args.binary.resolve()
    OUTPUT = ROOT / 'artifacts' / ('hardware-' + datetime.now().strftime('%Y%m%d-%H%M%S'))
    OUTPUT.mkdir(parents=True, exist_ok=True)
    results = []

    def run(label, *arguments, expected=0):
        command = [str(executable), *map(str, arguments)]
        process = subprocess.run(command, capture_output=True, text=True, encoding='utf-8', timeout=120)
        record = dict(label=label, time=datetime.now(timezone.utc).isoformat(), command=command,
                      exit=process.returncode, stdout=process.stdout, stderr=process.stderr)
        results.append(record)
        (OUTPUT / 'results.json').write_text(json.dumps(results, indent=2, ensure_ascii=False), encoding='utf-8')
        (OUTPUT / f'{label}.log').write_text(process.stdout + process.stderr, encoding='utf-8')
        print(label, 'exit', process.returncode, process.stdout.strip(), process.stderr.strip())
        assert process.returncode == expected, record
        return process

    before = run('01-list-before', 'list', IP).stdout
    for name in ('O3996.CNC', 'O3997.CNC'):
        assert name not in before, f'{name} already exists; refusing hardware test'
        result = run(f'02-absent-{name}', 'stat', IP, name, expected=4)
        assert '0x8016' in result.stderr

    small = OUTPUT / 'O3996-first-send.cnc'
    small.write_bytes(b'%\r\nO3996\r\n(GSK RUST TRANSFER TEST 2026-11-07)\r\n(COMMENTS AND M30 ONLY)\r\nM30\r\n%\r\n')
    large = OUTPUT / 'O3997-send.cnc'
    lines = ['%', 'O3997', '(GSK RUST MULTIBLOCK TEST 2026-11-07)']
    lines += [f'(BLOCK {n:05} ABCDEFGHIJKLMNOPQRSTUVWXYZ 0123456789)' for n in range(2500)]
    lines += ['M30', '%', '']
    large.write_bytes('\r\n'.join(lines).encode('ascii'))

    for number, path in ((3996, small), (3997, large)):
        name = f'O{number}.CNC'
        run(f'03-send-{number}', 'send', IP, path, name, '--no-clobber', '--verify')
        received = OUTPUT / f'O{number}-received.cnc'
        run(f'04-receive-{number}', 'receive', IP, name, received)
        assert path.read_bytes() == received.read_bytes()
        print(name, len(path.read_bytes()), sha(path))

    run('05-no-clobber', 'send', IP, small, 'O3996.CNC', '--no-clobber', expected=2)
    run('06-unchanged-after-refusal', 'verify', IP, small, 'O3996.CNC')
    shorter = OUTPUT / 'O3996-shorter-send.cnc'
    shorter.write_bytes(b'%\r\nO3996\r\n(RUST SHORT REWRITE)\r\nM30\r\n%\r\n')
    run('07-shorten-own-file', 'send', IP, shorter, 'O3996.CNC', '--verify')
    received = OUTPUT / 'O3996-shorter-received.cnc'
    run('08-receive-shorter', 'receive', IP, 'O3996.CNC', received)
    assert shorter.read_bytes() == received.read_bytes()
    run('09-list-after', 'list', IP)
    hashes = {p.name: dict(bytes=p.stat().st_size, sha256=sha(p)) for p in OUTPUT.glob('*.cnc')}
    (OUTPUT / 'hashes.json').write_text(json.dumps(hashes, indent=2), encoding='utf-8')
    print('ALL HARDWARE CHECKS PASSED')


if __name__ == '__main__':
    main()
