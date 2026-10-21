#!/usr/bin/env python3
"""Verify an adjacent AArch64 ADRP/LDR Q constant via actual thin Mach-O bytes.

This is an explanation helper, not a general disassembler or equality normalizer.
Unsupported encodings/layouts fail closed. No constant is accepted just from a
PAGEOFF placeholder: both 16-byte values must be read from the supplied binaries.
"""
import hashlib
import re
import struct


def constant_load(binary, disassembly, name, index, expected_sha256):
    data = binary.read_bytes()
    assert hashlib.sha256(data).hexdigest() == expected_sha256
    magic, _, _, _, ncmds = struct.unpack_from('<5I', data)
    assert magic == 0xfeedfacf, 'requires thin little-endian Mach-O 64'
    segments = []
    pos = 32
    for _ in range(ncmds):
        cmd, size = struct.unpack_from('<II', data, pos)
        assert size >= 8
        if cmd == 0x19:  # LC_SEGMENT_64
            vmaddr, _, fileoff, filesize = struct.unpack_from('<4Q', data, pos + 24)
            segments.append((vmaddr, fileoff, filesize))
        pos += size

    def read(address, size):
        for vmaddr, fileoff, filesize in segments:
            if vmaddr <= address and address + size <= vmaddr + filesize:
                offset = fileoff + address - vmaddr
                result = data[offset:offset + size]
                assert len(result) == size
                return result
        raise AssertionError(('address not file-backed', hex(address), size))

    # Match funcs()'s normalized instruction indexing (nop/udf omitted), without
    # discarding addresses needed to read and decode the actual instructions.
    instructions = []
    current = None
    for line in disassembly.read_text().splitlines():
        if line.endswith(':') and not line.startswith(' ') and '<' in line:
            current = line[line.index('<') + 1:line.rindex('>')]
            continue
        if current != name:
            continue
        match = re.match(r'^\s*([0-9a-f]+):\s*(\w+)\s*(.*)', line)
        if match and match[2] not in ('nop', 'udf'):
            instructions.append((int(match[1], 16), match[2], match[3]))
    assert 0 < index < len(instructions)
    pc, op, _ = instructions[index - 1]
    load_pc, load_op, _ = instructions[index]
    assert op == 'adrp' and load_op == 'ldr' and load_pc == pc + 4
    adrp = int.from_bytes(read(pc, 4), 'little')
    ldr = int.from_bytes(read(load_pc, 4), 'little')
    assert adrp & 0x9f000000 == 0x90000000
    assert ldr & 0xffc00000 == 0x3dc00000  # LDR Qt, [Xn, #imm12*16]
    assert adrp & 31 == (ldr >> 5) & 31
    immediate = ((adrp >> 5) & 0x7ffff) << 2 | ((adrp >> 29) & 3)
    if immediate & (1 << 20):
        immediate -= 1 << 21
    address = (pc & ~0xfff) + (immediate << 12) + (((ldr >> 10) & 0xfff) << 4)
    value = read(address, 16)
    return dict(address=hex(address), bytes_hex=value.hex(),
                sha256=hashlib.sha256(value).hexdigest(),
                instruction_addresses=[hex(pc), hex(load_pc)])
