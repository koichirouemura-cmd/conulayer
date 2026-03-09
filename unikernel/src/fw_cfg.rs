// fw_cfg.rs — QEMU fw_cfg インターフェース（ポート 0x510/0x511）
//
// QEMU が提供するシークレット注入機構。
// AIはシークレットをハードコードしない — fw_cfg 経由で実行時に受け取る。
//
// プロトコル:
//   0x510 (16bit 書き込み): セレクタ（どのエントリを読むか）
//   0x511 (8bit 読み込み):  データ（1バイトずつ）
//   0x514 (32bit 書き込み): DMA アドレス（今回は使わない）

const FW_CFG_PORT_SEL: u16 = 0x510;
const FW_CFG_PORT_DATA: u16 = 0x511;

// 標準セレクタ
const FW_CFG_SIGNATURE: u16 = 0x0000;
const FW_CFG_FILE_DIR: u16 = 0x0019; // ファイルディレクトリ

unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!(
        "out dx, ax",
        in("dx") port,
        in("ax") val,
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

#[allow(dead_code)]
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    core::arch::asm!(
        "in eax, dx",
        out("eax") val,
        in("dx") port,
        options(nomem, nostack, preserves_flags)
    );
    val
}

/// fw_cfg が存在するか確認（シグネチャ "QEMU" を確認）
pub fn is_present() -> bool {
    unsafe {
        outw(FW_CFG_PORT_SEL, FW_CFG_SIGNATURE);
        let b0 = inb(FW_CFG_PORT_DATA);
        let b1 = inb(FW_CFG_PORT_DATA);
        let b2 = inb(FW_CFG_PORT_DATA);
        let b3 = inb(FW_CFG_PORT_DATA);
        b0 == b'Q' && b1 == b'E' && b2 == b'M' && b3 == b'U'
    }
}

/// ファイルディレクトリを検索して、名前が一致するエントリのセレクタを返す
///
/// fw_cfg ファイルディレクトリの構造:
///   [4バイト: エントリ数 (big-endian)]
///   エントリ × N:
///     [4バイト: サイズ (big-endian)]
///     [2バイト: セレクタ (big-endian)]
///     [2バイト: 予約]
///     [56バイト: 名前 (NUL終端)]
pub fn find_file(name: &str) -> Option<(u16, u32)> {
    unsafe {
        outw(FW_CFG_PORT_SEL, FW_CFG_FILE_DIR);

        // エントリ数（big-endian 32bit）
        let count = u32::from_be_bytes([
            inb(FW_CFG_PORT_DATA),
            inb(FW_CFG_PORT_DATA),
            inb(FW_CFG_PORT_DATA),
            inb(FW_CFG_PORT_DATA),
        ]);

        for _ in 0..count {
            // サイズ（big-endian 32bit）
            let size = u32::from_be_bytes([
                inb(FW_CFG_PORT_DATA),
                inb(FW_CFG_PORT_DATA),
                inb(FW_CFG_PORT_DATA),
                inb(FW_CFG_PORT_DATA),
            ]);
            // セレクタ（big-endian 16bit）
            let selector = u16::from_be_bytes([
                inb(FW_CFG_PORT_DATA),
                inb(FW_CFG_PORT_DATA),
            ]);
            // 予約（2バイト読み捨て）
            let _ = inb(FW_CFG_PORT_DATA);
            let _ = inb(FW_CFG_PORT_DATA);
            // 名前（56バイト固定）
            let mut entry_name = [0u8; 56];
            for b in entry_name.iter_mut() {
                *b = inb(FW_CFG_PORT_DATA);
            }

            // NUL終端の文字列として比較
            let entry_str = core::str::from_utf8(&entry_name)
                .unwrap_or("")
                .trim_end_matches('\0');

            if entry_str == name {
                return Some((selector, size));
            }
        }
        None
    }
}

/// セレクタを指定してデータを読み込む（最大 buf.len() バイト）
pub fn read(selector: u16, buf: &mut [u8]) {
    unsafe {
        outw(FW_CFG_PORT_SEL, selector);
        for b in buf.iter_mut() {
            *b = inb(FW_CFG_PORT_DATA);
        }
    }
}
