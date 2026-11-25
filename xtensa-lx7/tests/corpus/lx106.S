/* Repository-owned CPU fixture, MIT (repository LICENSE). No SDK or ROM linked.
 * Assemble with toolchain-xtensa 2.100300.220621, as 2.32, GCC 10.3.0.
 * Link .text at 0x40100000; no relaxation, no automatic density conversion.
 */
    .text
    .begin no-transform
    .global _start, done, leaf, indirect, arithmetic, literal, literal_insn
indirect_address:
    .word indirect
_start:
    addi a1, a1, -32
    s32i a0, a1, 28
    movi.n a2, 7
    movi.n a3, 9
    movi a4, 11
    s32i a4, a1, 0
    call0 leaf
    s32i a2, a1, 4
    l32r a4, indirect_address
    callx0 a4
    l32i a5, a1, 4
    add a2, a2, a5
    l32i a0, a1, 28
    addi a1, a1, 32
done:
    j done
    .align 4
leaf:
    l32i a4, a1, 0
    add a2, a2, a3
    add a2, a2, a4
    ret
    .align 4
indirect:
    movi.n a2, 5
    ret.n
    .align 4
arithmetic:
    mull a4, a2, a3
    mul16s a5, a2, a3
    mul16u a6, a2, a3
    ret
    .align 4
literal:
    .word 0x12345678
literal_insn:
    l32r a2, literal
    ret
    .global software_divide, divide_done, wait, return_nmi, return_exception, debug_break
software_divide:
    addi a1, a1, -16
    s32i a0, a1, 12
    call0 divide_unsigned
    l32i a0, a1, 12
    addi a1, a1, 16
divide_done:
    j divide_done
    .align 4
divide_unsigned:
    movi.n a4, 0
divide_again:
    bltu a2, a3, divide_end
    sub a2, a2, a3
    addi.n a4, a4, 1
    j divide_again
divide_end:
    mov.n a2, a4
    ret
wait:
    waiti 0
return_nmi:
    rfi 3
return_exception:
    rfe
debug_break:
    break 0, 0
    .global special_registers
special_registers:
    wsr a2, litbase
    rsr a3, litbase
    xsr a4, litbase
    wsr a5, ccompare0
    rsr a6, ccompare0
    ret
    .end no-transform
