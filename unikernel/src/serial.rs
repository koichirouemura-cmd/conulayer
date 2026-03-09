// serial.rs — COM1 (0x3F8) へのシリアル出力
//
// UART 16550 を直接操作する。外部クレートなし。

const COM1: u16 = 0x3F8;

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack, preserves_flags)
    );
}

unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    core::arch::asm!(
        "in al, dx",
        out("al") val,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    val
}

/// シリアルポートを初期化する（38400 baud, 8N1）
pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00); // 割り込み無効
        outb(COM1 + 3, 0x80); // DLAB 有効（ボーレート設定モード）
        outb(COM1 + 0, 0x03); // ボーレート除数 下位 = 3 → 38400 baud
        outb(COM1 + 1, 0x00); // ボーレート除数 上位 = 0
        outb(COM1 + 3, 0x03); // 8bit, パリティなし, ストップビット1（DLAB オフ）
        outb(COM1 + 2, 0xC7); // FIFO 有効, クリア, 14バイト閾値
        outb(COM1 + 4, 0x0B); // RTS/DSR セット
    }
}

fn write_byte(byte: u8) {
    unsafe {
        // 送信レジスタが空くまで待つ
        while inb(COM1 + 5) & 0x20 == 0 {}
        outb(COM1, byte);
    }
}

/// 文字列をシリアルポートに出力する
pub fn print(s: &str) {
    for byte in s.bytes() {
        if byte == b'\n' {
            write_byte(b'\r'); // Windows 互換のために CR も送る
        }
        write_byte(byte);
    }
}
