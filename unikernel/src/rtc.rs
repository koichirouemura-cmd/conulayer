// rtc.rs — CMOS RTC から Unix タイムスタンプを読む
//
// CMOS ポート:
//   0x70: インデックスレジスタ（書き込み）
//   0x71: データレジスタ（読み取り）
//
// RTC レジスタ:
//   0x00: 秒, 0x02: 分, 0x04: 時, 0x07: 日, 0x08: 月, 0x09: 年（2桁）
//   0x0A: ステータスA（bit7=更新中フラグ）
//   0x0B: ステータスB（bit2=バイナリモード, bit1=24時間制）

const CMOS_ADDR: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

fn cmos_read(reg: u8) -> u8 {
    unsafe {
        core::arch::asm!(
            "out dx, al",
            in("dx") CMOS_ADDR,
            in("al") reg,
            options(nostack, preserves_flags)
        );
        let mut val: u8;
        core::arch::asm!(
            "in al, dx",
            out("al") val,
            in("dx") CMOS_DATA,
            options(nostack, preserves_flags)
        );
        val
    }
}

fn bcd_to_bin(v: u8) -> u8 {
    (v & 0x0F) + ((v >> 4) * 10)
}

/// CMOS RTC から Unix タイムスタンプ（秒）を返す。
/// QEMU の RTC は UTC。
pub fn read_unix_timestamp() -> i64 {
    // 更新中フラグが立っている間は待つ（読み取り中に値が変わるのを防ぐ）
    while cmos_read(0x0A) & 0x80 != 0 {}

    let sec  = cmos_read(0x00);
    let min  = cmos_read(0x02);
    let hour = cmos_read(0x04);
    let day  = cmos_read(0x07);
    let mon  = cmos_read(0x08);
    let year = cmos_read(0x09);
    let reg_b = cmos_read(0x0B);

    // BCD か バイナリか（bit2 が 1 ならバイナリ）
    let (s, mi, h, d, mo, y) = if reg_b & 0x04 == 0 {
        (
            bcd_to_bin(sec)  as i64,
            bcd_to_bin(min)  as i64,
            bcd_to_bin(hour) as i64,
            bcd_to_bin(day)  as i64,
            bcd_to_bin(mon)  as i64,
            bcd_to_bin(year) as i64,
        )
    } else {
        (sec as i64, min as i64, hour as i64, day as i64, mon as i64, year as i64)
    };

    let full_year = 2000 + y;

    // 1970-01-01 からの日数を計算
    let yy = full_year - 1970;
    // うるう年を考慮した日数（グレゴリオ暦）
    let mut days = yy * 365 + yy / 4 - yy / 100 + yy / 400;

    // 各月の日数（非うるう年）
    const DAYS_IN_MONTH: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 0..(mo - 1) as usize {
        days += DAYS_IN_MONTH[m];
    }

    // うるう年かつ2月より後なら1日追加
    let is_leap = (full_year % 4 == 0 && full_year % 100 != 0) || full_year % 400 == 0;
    if is_leap && mo > 2 {
        days += 1;
    }

    days += d - 1;

    days * 86400 + h * 3600 + mi * 60 + s
}
