"""CLI integration against a UDP fixture bound ONLY to 127.0.0.1.

Python 3 standard library; no DLL, CNC or network configuration needed.
python -X utf8 tests/integration.py [path/to/gsk.exe-or-gsk]
"""
import os
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EXE = Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / 'target/release/gsk.exe'
sys.argv = sys.argv[:1]
REMOTE = '/user/NCPROG/O3997.CNC'


def crc8(data):
    value = 0
    for byte in data:
        value ^= byte
        for _ in range(8):
            value = ((value << 1) ^ (7 if value & 128 else 0)) & 255
    return value


class Cnc:
    def __init__(self, **faults):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.bind(('127.0.0.1', 0))
        self.sock.settimeout(.05)
        self.port = self.sock.getsockname()[1]
        self.files, self.handles, self.requests, self.errors = {}, {}, [], []
        self.next_handle, self.cursor = 1, 0
        self.faults, self.dropped = faults, set()
        self.stop, self.writing = threading.Event(), threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *args):
        self.stop.set()
        self.thread.join(2)
        self.sock.close()
        assert not self.thread.is_alive()
        assert not self.errors, self.errors

    def run(self):
        try:
            while not self.stop.is_set():
                try:
                    packet, peer = self.sock.recvfrom(65536)
                except TimeoutError:
                    continue
                assert peer[0] == '127.0.0.1'
                assert packet[0] == crc8(packet[1:5])
                assert packet[2:5] == bytes([12, 100, 12])
                p = packet[5:]
                op = p[3]
                self.requests.append(bytes(p))
                r = bytearray(p[:4])
                r[3] |= 128
                if op == 63:
                    assert p[4:] == bytes([3, 0, 1, 0, 0])
                    r += p[4:]
                elif op == 6:
                    name = p[6:].split(b'\0')[0].decode('ascii')
                    if name not in self.files:
                        r[3] |= 64
                        r += struct.pack('<H', self.faults.get('stat_error', 0x8016))
                    else:
                        data = bytearray(52)
                        struct.pack_into('<I', data, 8, 0x100600)
                        struct.pack_into('<I', data, 28, len(self.files[name]))
                        r += struct.pack('<H', len(data)) + data
                elif op in (1, 8):
                    mode = struct.unpack_from('<H', p, 4)[0]
                    name = p[8:].split(b'\0')[0].decode('ascii')
                    r += struct.pack('<H', mode)
                    if op == 1:
                        r += b'\0\0'
                    if self.faults.get('open_error'):
                        r[3] |= 64
                        r += struct.pack('<H', 0x8006)
                    else:
                        handle = self.next_handle
                        self.next_handle += 1
                        self.handles[handle] = name
                        if mode == 0x1a:
                            self.files[name] = b''
                        elif mode == 0x11:
                            assert name in self.files
                        else:
                            assert mode == 0x100 and op == 8
                            self.cursor = 0
                        r += struct.pack('<H', handle)
                elif op in (2, 3):
                    handle, count, offset = struct.unpack_from('<HHI', p, 4)
                    name = self.handles[handle]
                    if op == 3:
                        self.writing.set()
                        assert count == len(p[12:]) and 0 < count <= 1024
                        old = self.files[name]
                        self.files[name] = old[:offset] + p[12:] + old[offset+count:]
                        r += p[4:12]
                        if self.faults.get('write_error'):
                            r[3] |= 64
                            r += struct.pack('<H', 0x8013)
                        if self.faults.get('bad_write_ack'):
                            struct.pack_into('<I', r, 8, offset + 1)
                    else:
                        data = self.files[name][offset:offset+count]
                        if self.faults.get('early_eof'):
                            data = b''
                        if not data:
                            r[3] |= 64
                            # Captured CNC EOF has stale count/offset. Deliberately reproduce it.
                            r += struct.pack('<HHIH', handle, count, 0x3d3d28, 0x8011)
                        else:
                            if self.faults.get('corrupt'):
                                data = bytes([data[0] ^ 1]) + data[1:]
                            r += struct.pack('<HHI', handle, len(data), offset) + data
                            if self.faults.get('truncate_read'):
                                r = r[:-1]
                elif op in (4, 9):
                    handle = struct.unpack_from('<H', p, 4)[0]
                    del self.handles[handle]
                    r += p[4:6]
                    if self.faults.get('close_error'):
                        r[3] |= 64  # Native close error may have no error-code field.
                elif op == 10:
                    handle, count = struct.unpack_from('<HH', p, 4)
                    assert handle in self.handles and count == 1024
                    names = sorted(self.files)[self.cursor:self.cursor+15]
                    self.cursor += len(names)
                    records = bytearray()
                    for path in names:
                        name = path.rsplit('/', 1)[-1].encode() + b'\0'
                        size = (24 + len(name) + 3) // 4 * 4
                        record = bytearray(size)
                        struct.pack_into('<III', record, 0, size, 0x100600, len(self.files[path]))
                        record[24:24+len(name)] = name
                        records += record
                    if self.faults.get('bad_dir') and records:
                        records[:4] = b'\xff' * 4
                    r += struct.pack('<HHH', handle, len(names), len(records)) + records
                else:
                    raise AssertionError(f'unexpected command {op}')

                if self.faults.get('drop_all') or self.faults.get('hold_write') and op == 3:
                    continue
                if op == self.faults.get('drop_once') and op not in self.dropped:
                    self.dropped.add(op)
                    continue
                response = bytes([1, 100, 12, packet[1], 12]) + r
                if self.faults.get('noise'):
                    stale = bytearray(response)
                    stale[5] ^= 1
                    self.sock.sendto(stale, peer)
                    wrong = bytearray(response)
                    wrong[3] ^= 1
                    self.sock.sendto(wrong, peer)
                    wrong = bytearray(response)
                    wrong[8] ^= 1
                    self.sock.sendto(wrong, peer)
                    self.sock.sendto(b'\x01\x64', peer)
                self.sock.sendto(response, peer)
        except BaseException as error:
            self.errors.append(repr(error))


class Integration(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='gsk-')
        self.root = Path(self.temp.name) / 'программы с пробелами'
        self.root.mkdir()
        self.source = self.root / 'пример.cnc'
        self.source.write_bytes(b'%\r\nO3997\r\n(RUST TEST)\r\nM30\r\n%\r\n')
        self.destination = self.root / 'принято.cnc'

    def tearDown(self):
        self.temp.cleanup()

    def args(self, cnc, *args):
        return [str(EXE), *map(str, args), '--port', str(cnc.port), '--timeout-ms', '150', '--retries', '2']

    def call(self, cnc, *args, code=0):
        result = subprocess.run(self.args(cnc, *args), capture_output=True, text=True, encoding='utf-8', timeout=15)
        self.assertEqual(result.returncode, code, result.stdout + result.stderr)
        return result

    def test_upload_boundaries_readback_and_unicode_local_paths(self):
        for length in (1, 1024, 1025, 15656, 131073):
            with self.subTest(length=length), Cnc(noise=True) as cnc:
                data = bytes(n % 251 for n in range(length))
                self.source.write_bytes(data)
                self.call(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC', '--verify')
                self.assertEqual(cnc.files[REMOTE], data)
                self.call(cnc, 'receive', '127.0.0.1', 'O3997.CNC', self.destination, '--force')
                self.assertEqual(self.destination.read_bytes(), data)
                self.assertFalse(cnc.handles)

    def test_compatibility_arguments_and_shorter_overwrite(self):
        with Cnc() as cnc:
            cnc.files[REMOTE] = b'old longer content' * 100
            self.call(cnc, '127.0.0.1', self.source, 'o3997.cnc', '--verify')
            self.assertEqual(cnc.files[REMOTE], self.source.read_bytes())

    def test_safe_retries_reuse_sequence_and_offset(self):
        for op in (63, 6, 3, 2):
            with self.subTest(op=op), Cnc(drop_once=op) as cnc:
                self.call(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC', '--verify')
                requests = [p for p in cnc.requests if p[3] == op]
                self.assertEqual(requests[0], requests[1])
                self.assertEqual(cnc.files[REMOTE], self.source.read_bytes())

    def test_lost_open_reply_is_never_replayed(self):
        with Cnc(drop_once=1) as cnc:
            result = self.call(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC', code=4)
            self.assertEqual(sum(p[3] == 1 for p in cnc.requests), 1)
            self.assertIn('may have completed', result.stderr)

    def test_no_clobber_never_opens_existing_file(self):
        with Cnc() as cnc:
            cnc.files[REMOTE] = b'preserve'
            self.call(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC', '--no-clobber', code=2)
            self.assertEqual(cnc.files[REMOTE], b'preserve')
            self.assertNotIn(1, [p[3] for p in cnc.requests])

    def test_unknown_stat_error_does_not_create(self):
        with Cnc(stat_error=0x8005) as cnc:
            self.call(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC', code=4)
            self.assertNotIn(1, [p[3] for p in cnc.requests])

    def test_rejected_open(self):
        with Cnc(open_error=True) as cnc:
            self.call(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC', code=4)
            self.assertNotIn(3, [p[3] for p in cnc.requests])

    def test_write_errors_close_handle_and_fail(self):
        for fault in ('write_error', 'bad_write_ack', 'close_error'):
            with self.subTest(fault=fault), Cnc(**{fault: True}) as cnc:
                self.call(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC', code=4)
                self.assertFalse(cnc.handles)

    def test_verification_detects_changed_bytes(self):
        with Cnc(corrupt=True) as cnc:
            self.call(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC', '--verify', code=5)
            self.assertFalse(cnc.handles)

    def test_failed_receive_preserves_local_file(self):
        for fault in ('early_eof', 'truncate_read', 'close_error'):
            with self.subTest(fault=fault), Cnc(**{fault: True}) as cnc:
                cnc.files[REMOTE] = self.source.read_bytes()
                self.destination.write_bytes(b'preserve local')
                self.call(cnc, 'receive', '127.0.0.1', 'O3997.CNC', self.destination, '--force', code=4)
                self.assertEqual(self.destination.read_bytes(), b'preserve local')
                self.assertEqual(len(list(self.root.iterdir())), 2)
                self.assertFalse(cnc.handles)

    def test_receive_existing_destination_requires_force_before_network(self):
        with Cnc() as cnc:
            self.destination.write_bytes(b'preserve')
            self.call(cnc, 'receive', '127.0.0.1', 'O3997.CNC', self.destination, code=2)
            self.assertFalse(cnc.requests)

    def test_directory_pagination_and_stat(self):
        with Cnc() as cnc:
            cnc.files = {f'/user/NCPROG/O{n:04}.CNC': b'x' * n for n in range(3000, 3035)}
            result = self.call(cnc, 'list', '127.0.0.1')
            self.assertEqual(len(result.stdout.splitlines()), 35)
            self.assertIn('O3034.CNC', result.stdout)
            self.assertEqual(sum(p[3] == 10 for p in cnc.requests), 4)
            self.call(cnc, 'stat', '127.0.0.1', 'O3000.CNC')
            self.assertFalse(cnc.handles)

    def test_directory_failure_closes_and_never_replays_cursor_read(self):
        for fault in ({'drop_once': 10}, {'bad_dir': True}):
            with self.subTest(fault=fault), Cnc(**fault) as cnc:
                cnc.files[REMOTE] = b'x'
                self.call(cnc, 'list', '127.0.0.1', code=4)
                self.assertEqual(sum(p[3] == 10 for p in cnc.requests), 1)
                self.assertFalse(cnc.handles)

    def test_missing_receive_does_not_publish_file(self):
        with Cnc() as cnc:
            self.call(cnc, 'receive', '127.0.0.1', 'O3997.CNC', self.destination, code=4)
            self.assertFalse(self.destination.exists())

    def test_invalid_name_and_dry_run_send_nothing(self):
        with Cnc() as cnc:
            self.call(cnc, 'send', '127.0.0.1', self.source, '../O3997.CNC', code=2)
            self.call(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC', '--dry-run')
            self.assertFalse(cnc.requests)

    def test_timeout_is_bounded(self):
        with Cnc(drop_all=True) as cnc:
            self.call(cnc, 'stat', '127.0.0.1', 'O3997.CNC', code=4)
            self.assertEqual(len(cnc.requests), 3)

    @unittest.skipIf(os.name == 'nt', 'POSIX SIGINT integration; Windows uses the same cancellation flag')
    def test_interrupt_closes_open_handle(self):
        with Cnc(hold_write=True) as cnc:
            process = subprocess.Popen(self.args(cnc, 'send', '127.0.0.1', self.source, 'O3997.CNC'), stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            try:
                self.assertTrue(cnc.writing.wait(5))
                process.send_signal(signal.SIGINT)
                out, err = process.communicate(timeout=5)
                self.assertEqual(process.returncode, 130, (out, err))
                self.assertFalse(cnc.handles)
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait()


if __name__ == '__main__':
    unittest.main(verbosity=2)
