# Repository-owned MIT raw negative corpus. Not an assembler/hardware oracle.
# QEMU v10.1.0 translate.c: wsr/xsr.configid*, .debugcause, .prid;
# xsr.intset/intclear. RSR INTCLEAR: profile rejects architecturally undefined read.
.text
.byte 0x20, 0xb0, 0x13 # wsr.configid0 a2
.byte 0x20, 0xd0, 0x13 # wsr.configid1 a2
.byte 0x20, 0xeb, 0x13 # wsr.prid a2
.byte 0x20, 0xe9, 0x13 # wsr.debugcause a2
.byte 0x20, 0xb0, 0x61 # xsr.configid0 a2
.byte 0x20, 0xd0, 0x61 # xsr.configid1 a2
.byte 0x20, 0xeb, 0x61 # xsr.prid a2
.byte 0x20, 0xe9, 0x61 # xsr.debugcause a2
.byte 0x20, 0xe2, 0x61 # xsr.intset a2
.byte 0x20, 0xe3, 0x61 # xsr.intclear a2
.byte 0x20, 0xe3, 0x03 # rsr.intclear a2 (undefined read policy)
