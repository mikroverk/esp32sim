.option rvc
.text
start:
.org 48
init:
 addi sp,sp,-16
 sw ra,12(sp)
 nop
 nop
 .org 56
1: auipc t0,0
 lw t1,-52(t0)
 lw a0,-48(t0)
 lw a1,-44(t0)
 lw a2,-24(t0)
 li a3,0
 li a4,23
 li a5,0
 li a6,0
 jalr t1
 nop
 nop
 nop
 nop
 nop
 .org 96
2: auipc t0,0
 lw t1,-72(t0)
 jalr t1
 lw ra,12(sp)
 addi sp,sp,16
 ret
.org 112
task:
3: auipc t0,0
 lw t1,-92(t0)
 jalr t1
 beqz a0,sleep
 mv t1,a0
 nop
 .org 128
4: auipc t0,0
 lw a0,-100(t0)
 lw a1,0(a0)
 addi a0,a0,16
 jalr t1
 j task
sleep:
 li a0,1
 nop
 nop
 nop
 .org 152
5: auipc t0,0
 lw t1,-136(t0)
 jalr t1
 j task
.org 176
 ret
.org 192
 ret
