"""Capture read-only packet evidence for a test program on an authorized CNC.

python research/probe_readonly.py --ip 192.168.1.100 --program O3998.CNC
Writes local artifacts/read-only-packets.json. Does not enumerate production programs.
"""
import argparse
import json
import socket
import struct
from pathlib import Path


def crc8(data):
    value = 0
    for byte in data:
        value ^= byte
        for _ in range(8):
            value = ((value << 1) ^ (7 if value & 128 else 0)) & 255
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ip', required=True)
    parser.add_argument('--program', default='O3998.CNC')
    args = parser.parse_args()
    name = args.program.upper()
    assert len(name) == 9 and name[0] == 'O' and name[1:5].isascii() and name[1:5].isdigit() and name[5:] == '.CNC'
    records, sequence = [], 0
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.connect((args.ip, 6000))
        sock.settimeout(3)
        local_id = int(sock.getsockname()[0].split('.')[-1])
        if local_id in (0, 100, 254, 255):
            local_id = 25
        header = bytes([local_id, 12, 100, 12])

        def request(op, body, label):
            nonlocal sequence
            seq = sequence
            sequence += 1
            payload = struct.pack('<HBB', seq, 0, op) + body
            packet = bytes([crc8(header)]) + header + payload
            sock.send(packet)
            reply = sock.recv(4096)
            assert len(reply) >= 9 and reply[5:7] == struct.pack('<H', seq) and reply[8] & 63 == op
            records.append(dict(label=label, request=packet.hex(), response=reply.hex()))
            print(label, 'bytes', len(reply))
            return reply[5:]

        path = ('/user/NCPROG/' + name).encode('ascii') + b'\0'
        path_body = struct.pack('<H', len(path)) + path
        request(63, struct.pack('<HH', 3, 1) + b'\0', 'mount')
        stat = request(6, path_body + b'\0\0', 'stat_test')
        assert not stat[3] & 64
        size = struct.unpack_from('<I', stat, 6 + 28)[0]
        reply = request(1, struct.pack('<H', 0x11) + path_body, 'open_read')
        assert not reply[3] & 64
        handle = struct.unpack_from('<H', reply, 8)[0]
        try:
            request(2, struct.pack('<HHI', handle, 64, 0), 'read_first_64')
            request(2, struct.pack('<HHI', handle, 64, size), 'read_eof')
        finally:
            request(4, struct.pack('<H', handle), 'close_file')
    output = Path(__file__).resolve().parents[1] / 'artifacts/read-only-packets.json'
    output.parent.mkdir(exist_ok=True)
    output.write_text(json.dumps(records, indent=2) + '\n', encoding='utf-8')
    print(output)


if __name__ == '__main__':
    main()
