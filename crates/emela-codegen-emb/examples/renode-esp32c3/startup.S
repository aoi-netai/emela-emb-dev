/* ELF loading initializes .data; reset code initializes .bss and the stack. */
.section .text.startup, "ax", @progbits
.balign 64
.global _start
.type _start, @function
_start:
    csrci mstatus, 8
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop

    la t0, _bss_start
    la t1, _bss_end
1:
    bgeu t0, t1, 2f
    sw zero, 0(t0)
    addi t0, t0, 4
    j 1b
2:
    la t0, _stack_guard_start
    la t1, _stack_bottom
    li t2, 0xdeadc0de
3:
    bgeu t0, t1, 4f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 3b
4:
    la t1, _stack_top
    li t2, 0xa5a5a5a5
5:
    bgeu t0, t1, 6f
    sw t2, 0(t0)
    addi t0, t0, 4
    j 5b
6:
    la sp, _stack_top
    call probe_main
7:
    wfi
    j 7b
.size _start, .-_start

/* The runner selects this non-vectored CLIC entry through MTVEC. */
.section .text.trap, "ax", @progbits
.balign 64
.global trap_entry
.type trap_entry, @function
trap_entry:
    addi sp, sp, -128
    sw ra, 0(sp)
    sw t0, 4(sp)
    sw t1, 8(sp)
    sw t2, 12(sp)
    sw a0, 16(sp)
    sw a1, 20(sp)
    sw a2, 24(sp)
    sw a3, 28(sp)
    sw a4, 32(sp)
    sw a5, 36(sp)
    sw a6, 40(sp)
    sw a7, 44(sp)
    sw t3, 48(sp)
    sw t4, 52(sp)
    sw t5, 56(sp)
    sw t6, 60(sp)
    csrr a0, mcause
    csrr a1, mepc
    call probe_trap
    lw ra, 0(sp)
    lw t0, 4(sp)
    lw t1, 8(sp)
    lw t2, 12(sp)
    lw a0, 16(sp)
    lw a1, 20(sp)
    lw a2, 24(sp)
    lw a3, 28(sp)
    lw a4, 32(sp)
    lw a5, 36(sp)
    lw a6, 40(sp)
    lw a7, 44(sp)
    lw t3, 48(sp)
    lw t4, 52(sp)
    lw t5, 56(sp)
    lw t6, 60(sp)
    addi sp, sp, 128
    mret
.size trap_entry, .-trap_entry
