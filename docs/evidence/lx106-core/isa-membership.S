/* One instance of every LX106 opcode the profile executes (tests/lx106.rs LX106_OPS).
 * Assembled with the LX106 configuration of Espressif's multi-config toolchain; see
 * isa-membership.json. rer/wer take no operands in that configuration.
 * Repository-owned fixture, MIT (repository LICENSE).
 */
.text
.begin no-transform
.literal_position
.literal .Llit, 0x12345678
nop
nop.n
break 1, 2
break.n 1
syscall
waiti 0
rsil a2, 1
isync
rsync
esync
dsync
excw
memw
extw
j 1f
1:
jx a3
call0 2f
.align 4
2:
callx0 a3
ret
ret.n
rfe
rfde
rfi 2
rfi 3
beqz a2, 1f
1:
bnez a2, 1f
1:
bltz a2, 1f
1:
bgez a2, 1f
1:
beqz.n a2, 1f
nop.n
1:
bnez.n a2, 1f
nop.n
1:
beqi a2, 1, 1f
1:
bnei a2, 1, 1f
1:
blti a2, 1, 1f
1:
bgei a2, 1, 1f
1:
bltui a2, 2, 1f
1:
bgeui a2, 2, 1f
1:
bnone a2, a3, 1f
1:
beq a2, a3, 1f
1:
blt a2, a3, 1f
1:
bltu a2, a3, 1f
1:
ball a2, a3, 1f
1:
bbc a2, a3, 1f
1:
bbci a2, 5, 1f
1:
bany a2, a3, 1f
1:
bne a2, a3, 1f
1:
bge a2, a3, 1f
1:
bgeu a2, a3, 1f
1:
bnall a2, a3, 1f
1:
bbs a2, a3, 1f
1:
bbsi a2, 5, 1f
1:
l8ui a2, a3, 1
l16ui a2, a3, 2
l16si a2, a3, 2
l32i a2, a3, 4
l32i.n a2, a3, 4
l32r a2, .Llit
s8i a2, a3, 1
s16i a2, a3, 2
s32i a2, a3, 4
s32i.n a2, a3, 4
movi a2, 100
movi.n a2, 10
mov a2, a3
mov.n a2, a3
add a2, a3, a4
add.n a2, a3, a4
addi a2, a3, 5
addi.n a2, a3, 5
addmi a2, a3, 256
sub a2, a3, a4
addx2 a2, a3, a4
addx4 a2, a3, a4
addx8 a2, a3, a4
subx2 a2, a3, a4
subx4 a2, a3, a4
subx8 a2, a3, a4
and a2, a3, a4
or a2, a3, a4
xor a2, a3, a4
neg a2, a3
abs a2, a3
extui a2, a3, 4, 8
moveqz a2, a3, a4
movnez a2, a3, a4
movltz a2, a3, a4
movgez a2, a3, a4
slli a2, a3, 5
srai a2, a3, 5
srli a2, a3, 5
sll a2, a3
srl a2, a3
sra a2, a3
src a2, a3, a4
ssr a3
ssl a3
ssa8l a3
ssa8b a3
ssai 5
nsa a2, a3
nsau a2, a3
mull a2, a3, a4
mul16u a2, a3, a4
mul16s a2, a3, a4
rer
wer
ritlb0 a2, a3
iitlb a3
pitlb a2, a3
witlb a2, a3
ritlb1 a2, a3
rdtlb0 a2, a3
idtlb a3
pdtlb a2, a3
wdtlb a2, a3
rdtlb1 a2, a3
rsr a2, sar
wsr a2, sar
xsr a2, sar
.end no-transform
