/*
 * boot.s — Multiboot2 ヘッダ + 32bit→64bit 移行コード
 *
 * 処理の流れ:
 *   GRUB が Multiboot2 ヘッダを検出
 *   → 32bitプロテクトモードで _start へジャンプ
 *   → ページテーブル設定（0-4GB を 1GB ヒュージページで恒等写像）
 *   → 64bitロングモードへ移行
 *   → kernel_main() を呼び出す
 */

/* ─── Multiboot2 定数 ─────────────────────────────── */
.set MB2_MAGIC,    0xE85250D6
.set MB2_ARCH,     0            /* i386 protected mode */
.set MB2_LENGTH,   24           /* ヘッダサイズ（バイト） */
.set MB2_CHECKSUM, (0x100000000 - (MB2_MAGIC + MB2_ARCH + MB2_LENGTH))

/* ─── Multiboot2 ヘッダ ──────────────────────────────
 * ELF の先頭 32KB 以内に配置する必要がある。
 * linker.ld で .multiboot2 セクションを先頭に置いている。
 */
.section .multiboot2, "a"
.align 8
mb2_header_start:
    .long MB2_MAGIC
    .long MB2_ARCH
    .long MB2_LENGTH
    .long MB2_CHECKSUM
    /* End tag */
    .align 8
    .short 0    /* type  = 0 (end) */
    .short 0    /* flags = 0       */
    .long  8    /* size  = 8       */
mb2_header_end:

/* ─── BSS: ページテーブル + スタック ────────────────── */
.section .bss
.align 4096
p4_table:   .skip 4096    /* PML4 */
p3_table:   .skip 4096    /* PDPT（1GB ヒュージページを使用） */
/* p2_table は不要：P3 エントリに PS ビット=1 で直接 1GB ページを指定 */

.align 16
stack_bottom:
    .skip 65536           /* 64KB スタック */
stack_top:

/* ─── GDT（64bit 用） ────────────────────────────────
 *   0x00: null descriptor
 *   0x08: code (P=1, DPL=0, S=1, Type=0xA, L=1)
 *   0x10: data (P=1, DPL=0, S=1, Type=0x2, W=1)
 */
.section .rodata
.align 16
gdt64:
    .quad 0x0000000000000000    /* null  */
    .quad 0x00AF9A000000FFFF    /* code: 64-bit */
    .quad 0x00CF92000000FFFF    /* data */
gdt64_end:

.align 4
gdt64_ptr:
    .short gdt64_end - gdt64 - 1
    .long  gdt64

/* ─── 32bit エントリポイント ──────────────────────────
 * QEMU から以下の状態で呼ばれる:
 *   EAX = 0x36D76289 (Multiboot2 magic)
 *   EBX = Multiboot2 情報構造体の物理アドレス
 */
.section .text
.code32
.global _start
_start:
    cli
    movl $stack_top, %esp
    movl %ebx, %edi          /* Multiboot2 ポインタを保存 */

    /* ── ページテーブル設定（0-4GB を 1GB ヒュージページで恒等写像） ──
     *
     * 1GB ヒュージページ（PDPT エントリに PS ビット=1）を使うことで
     * p2_table が不要になる。x86_64 は pdpe1gb フラグで対応確認できる。
     * QEMU の -cpu host では必ずサポートされる。
     *
     * PDPT エントリ形式（1GB ページ）:
     *   bit  0  : P (present)
     *   bit  1  : R/W (writable)
     *   bit  7  : PS = 1 (1GB ヒュージページ)
     *   bit 31:30: 物理アドレス上位ビット（1GB アライン）
     */

    /* P4[0] → p3_table (present + writable) */
    movl $p3_table, %eax
    orl  $0x3, %eax
    movl %eax, (p4_table)

    /* P3[0] → 1GB 恒等写像 @ 0x00000000 (0-1GB) */
    movl $0x00000083, %eax   /* phys=0x000000000, PS|R/W|P */
    movl %eax,  (p3_table)
    movl $0,    p3_table+4

    /* P3[1] → 1GB 恒等写像 @ 0x40000000 (1-2GB) */
    movl $0x40000083, %eax   /* phys=0x040000000 */
    movl %eax,  p3_table+8
    movl $0,    p3_table+12

    /* P3[2] → 1GB 恒等写像 @ 0x80000000 (2-3GB) */
    movl $0x80000083, %eax   /* phys=0x080000000 */
    movl %eax,  p3_table+16
    movl $0,    p3_table+20

    /* P3[3] → 1GB 恒等写像 @ 0xC0000000 (3-4GB)
     * MMIO バー（例: 0xFE004000）はここに収まる */
    movl $0xC0000083, %eax   /* phys=0x0C0000000 */
    movl %eax,  p3_table+24
    movl $0,    p3_table+28

    /* ── CR3 に P4 をセット ── */
    movl $p4_table, %eax
    movl %eax, %cr3

    /* ── PAE を有効化 ── */
    movl %cr4, %eax
    orl  $0x20, %eax        /* CR4.PAE */
    movl %eax, %cr4

    /* ── EFER.LME: ロングモード有効化 ── */
    movl $0xC0000080, %ecx
    rdmsr
    orl  $0x100, %eax
    wrmsr

    /* ── ページングを有効化（CR0.PG） ── */
    movl %cr0, %eax
    orl  $0x80000001, %eax
    movl %eax, %cr0

    /* ── GDT をロードして 64bit セグメントへジャンプ ── */
    lgdt gdt64_ptr
    ljmp $0x08, $long_mode_entry

/* ─── 割り込みハンドラ（64bit） ──────────────────────
 * CPU例外（エラーコードなし）: iretq のみ
 * CPU例外（エラーコードあり）: エラーコードを捨てて iretq
 * PIC マスター (IRQ 0-7 → INT 0x20-0x27): EOI → iretq
 * PIC スレーブ (IRQ 8-15 → INT 0x28-0x2F): EOI ×2 → iretq
 */
.code64
.global irq_exception
irq_exception:
    iretq

.global irq_exception_err
irq_exception_err:
    addq $8, %rsp        /* エラーコードをスキップ */
    iretq

.global irq_pic_master
irq_pic_master:
    /* caller-saved レジスタを全て保存（System V AMD64 ABI） */
    push %rax
    push %rcx
    push %rdx
    push %rsi
    push %rdi
    push %r8
    push %r9
    push %r10
    push %r11
    /* EOI を先に送る（マスター PIC） */
    movb $0x20, %al
    outb %al, $0x20
    /* Rust tick カウンタをインクリメント */
    call timer_irq_handler
    /* レジスタを逆順に復元 */
    pop %r11
    pop %r10
    pop %r9
    pop %r8
    pop %rdi
    pop %rsi
    pop %rdx
    pop %rcx
    pop %rax
    iretq

.global irq_pic_slave
irq_pic_slave:
    push %rax
    movb $0x20, %al
    outb %al, $0xA0      /* スレーブ PIC に EOI */
    outb %al, $0x20      /* マスター PIC に EOI */
    pop %rax
    iretq

/* ─── 64bit コード ──────────────────────────────── */
.code64
long_mode_entry:
    /* セグメントレジスタを 64bit データセグメントに設定 */
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %fs
    movw %ax, %gs
    movw %ax, %ss

    /* Rust の kernel_main を呼び出す */
    call kernel_main

    /* ここには来ないはず */
.halt:
    hlt
    jmp .halt
