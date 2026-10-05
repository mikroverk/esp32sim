.option norvc
.option norelax
.text
.global init
.org 16
init:
 addi sp,sp,-16
 sw ra,12(sp)
init_loop:
 jal ra,init_next
 beqz t0,done
 jalr ra,t0,0
 j init_loop
done:
 lw ra,12(sp)
 addi sp,sp,16
 ret
.org 64
task:
 jal ra,task_next
 jalr ra,t0,0
 j task
.org 80
free_cmd:
 addi sp,sp,-16
 sw ra,12(sp)
 # Host sets t0 to the matching free function and a1 to the transport status.
 sw a1,8(sp)
 jalr ra,t0,0
 lw a0,8(sp)
 lw ra,12(sp)
 addi sp,sp,16
 ret
.org 112
init_next:
 ret
.org 116
task_next:
 ret
