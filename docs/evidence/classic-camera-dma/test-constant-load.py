#!/usr/bin/env python3
"""Unit fixtures for the narrow Mach-O constant-load verifier; no emulator evidence."""
import hashlib
import os
from pathlib import Path
import runpy
import struct
import tempfile
import unittest

load = runpy.run_path(str(Path(__file__).with_name('constant-load.py')))['constant_load']


class ConstantLoadTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=os.environ['TMPDIR'])
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def fixture(self, offset, value=b'0123456789abcdef', opcode=None):
        # Deliberately small synthetic thin Mach-O: one file-backed segment,
        # adjacent ADRP x8 / LDR q0,[x8,#offset], and a known 16-byte constant.
        data = bytearray(0x2000)
        struct.pack_into('<8I', data, 0, 0xfeedfacf, 0x100000c, 0, 2, 1, 72, 0, 0)
        struct.pack_into('<II16s4Q', data, 32, 0x19, 72, b'__TEXT',
                         0x100000000, len(data), 0, len(data))
        struct.pack_into('<II', data, 0x100, 0xb0000008,
                         0x3dc00000 | ((offset // 16) << 10) | (8 << 5) if opcode is None else opcode)
        data[0x1000 + offset:0x1010 + offset] = value
        binary = self.root / ('binary-' + str(offset))
        binary.write_bytes(data)
        dis = self.root / ('dis-' + str(offset))
        dis.write_text('100000100 <test>:\n100000100: adrp\tx8, 1\n100000104: ldr\tq0, [x8]\n')
        return binary, dis, 'test', 1, hashlib.sha256(data).hexdigest()

    def test_zero_and_nonzero_offsets_resolve_equal_actual_constants(self):
        zero = load(*self.fixture(0))
        relocated = load(*self.fixture(0x480))
        self.assertEqual(zero['bytes_hex'], relocated['bytes_hex'])
        self.assertEqual(zero['address'], '0x100001000')
        self.assertEqual(relocated['address'], '0x100001480')

    def test_changed_constant_is_not_equal(self):
        zero = load(*self.fixture(0))
        changed = load(*self.fixture(0x480, b'changed constant'))
        self.assertNotEqual(zero['bytes_hex'], changed['bytes_hex'])

    def test_binary_hash_mismatch_rejected(self):
        args = list(self.fixture(0))
        args[-1] = 'wrong hash'
        with self.assertRaises(AssertionError):
            load(*args)

    def test_unsupported_instruction_rejected(self):
        with self.assertRaises(AssertionError):
            load(*self.fixture(0, opcode=0xd503201f))


if __name__ == '__main__':
    unittest.main()
