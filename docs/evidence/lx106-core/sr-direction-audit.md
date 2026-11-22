LX106 special-register direction audit

Source: QEMU v10.1.0 target/xtensa/translate.c (sources.json pins the fetched file hash); Xtensa ISA manual https://0x04.net/~mwk/doc/xtensa.pdf tables 5-135, 5-137, 5-139, 5-148..159, 5-160..162, 5-164, 5-166..167, 5-169..181, 5-183. This is a source-based access-direction audit, not hardware validation or a QEMU execution differential.

All 31 supported SR numbers checked in all three directions:
  Read/write/exchange: SAR(3), LITBASE(5), IBREAKENABLE(96), DDR(104), IBREAKA0(128), DBREAKA0(144), DBREAKC0(160), EPC1..3(177..179), DEPC(192), EPS2..3(194..195), EXCSAVE1..3(209..211), INTENABLE(228), PS(230), VECBASE(231), EXCCAUSE(232), CCOUNT(234), ICOUNT(236), ICOUNTLEVEL(237), EXCVADDR(238), CCOMPARE0(240).
  Read only: CONFIGID0(176), CONFIGID1(208), DEBUGCAUSE(233), PRID(235).
  Read/write, not exchange: INTERRUPT read / INTSET write (226).
  Write only in this profile: INTCLEAR(227). Manual table 5-171 specifies an undefined RSR value, not an illegal read; QEMU translates it. The existing LX106 profile already rejects this undefined read. Retain that deliberate strict policy and make supports() consistent with execution. Do not claim hardware mandates the RSR rejection.

CONFIGID write/exchange: QEMU lines 4740..4744, 5505..5509.
DEBUGCAUSE write/exchange: QEMU lines 4801..4802, 5566..5567; manual table 5-159 specifies WSR reserved and XSR No.
PRID write/exchange: QEMU lines 5319..5320, 6048..6049; manual table 5-181 (printed p235) specifies WSR reserved and XSR No.
Interrupt exchanges: QEMU lines 5841..5842, 5856..5860; manual tables 5-169..171 specify XSR No.
All other supported SR entries are translated by QEMU in each direction; direction legality does not assert full modeled debug/TLB/timing semantics.

The pinned LX106 assembler is corroborating evidence only. It rejects all named CONFIGID forms (including RSR aliases), WSR/XSR PRID and XSR INTSET/INTCLEAR, but accepts WSR/XSR DEBUGCAUSE despite the manual's reserved/No entries. Assembler acceptance or EXCW fallback disassembly is not a hardware legality oracle.

Tests: independent 256-register x 3-direction matrix (768 checks), accessor availability checks, explicit positive RSR for each read-only register, and 11 raw negative encodings at kernel/user/double-exception PS states. The six reported CONFIGID/PRID encodings use a2=0xdeadbeef and PC=0x40100000; expect IllegalInstruction, fault EPC1 (DEPC for double), vector base+0x30/+0x50/+0x70, all 16 ARs and every unrelated readable SR preserved. Normal exception entry and cycle charging are not claimed absent.
