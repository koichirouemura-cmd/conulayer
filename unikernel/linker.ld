ENTRY(_start)

SECTIONS {
    /* 1MB から配置（BIOS領域を避ける） */
    . = 0x100000;

    /* Multiboot2 ヘッダを最初に配置（ELF先頭32KB以内に収める） */
    .multiboot2 ALIGN(8) : {
        KEEP(*(.multiboot2))
    }

    /* コードセクション */
    .text ALIGN(4096) : {
        *(.text .text.*)
    }

    /* 読み取り専用データ（GDTなど） */
    .rodata ALIGN(4096) : {
        *(.rodata .rodata.*)
    }

    /* 読み書きデータ */
    .data ALIGN(4096) : {
        *(.data .data.*)
    }

    /* 初期化なし領域（ページテーブル・スタックなど） */
    .bss ALIGN(4096) : {
        _bss_start = .;
        *(COMMON)
        *(.bss .bss.*)
        . = ALIGN(4096);
        _bss_end = .;
    }

    /* デバッグ情報などは除外 */
    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.note .note.*)
    }
}
