/* Deliberately absent LX106 options. Assemble with an ESP32-S3 assembler,
 * then wrap its .text bytes in LX106 .byte directives for the negative corpus.
 * Repository-owned fixture, MIT (repository LICENSE).
 */
.text
.begin no-transform
entry a1, 32
retw
retw.n
call4 3f
call8 3f
call12 3f
callx4 a2
callx8 a2
callx12 a2
movsp a2, a3
rotw 1
l32e a2, a3, -4
s32e a2, a3, -4
rfwo
rfwu
add.s f0, f1, f2
lsi f0, a2, 0
rfr a2, f0
wfr f0, a2
quos a4, a2, a3
quou a4, a2, a3
rems a4, a2, a3
remu a4, a2, a3
mulsh a4, a2, a3
muluh a4, a2, a3
loop a2, 3f
sext a2, a3, 7
clamps a2, a3, 7
min a4, a2, a3
max a4, a2, a3
minu a4, a2, a3
maxu a4, a2, a3
andb b0, b1, b2
bf b0, 2f
2:
movf a2, a3, b0
s32c1i a2, a3, 0
l32ai a2, a3, 0
s32ri a2, a3, 0
rur a2, threadptr
wsr a2, cpenable
rsr a2, ccompare1
rsr a2, ccompare2
.align 4
3:
.end no-transform
