// timer.rs — PIT 1000Hz タイマー + ms カウンタ
//
// PIT Channel 0 を 1000Hz に再プログラムし、IRQ0 ハンドラから
// Rust の tick カウンタをインクリメントする。

use core::sync::atomic::{AtomicU64, Ordering};

static TICK: AtomicU64 = AtomicU64::new(0);

/// IRQ0 ハンドラから呼ばれる（boot.s の irq_pic_master から call）
#[no_mangle]
pub extern "C" fn timer_irq_handler() {
    TICK.fetch_add(1, Ordering::Relaxed);
}

/// PIT を 1000Hz に初期化する（idt::init() 末尾から呼ぶ）
pub fn init() {
    unsafe { pit_set_1000hz(); }
}

/// PIT Channel 0 を 1000Hz に再プログラム
/// divisor = 1193182 / 1000 ≈ 1193 = 0x04A9
unsafe fn pit_set_1000hz() {
    outb(0x43, 0x36); // ch0, lo/hi, mode3 (square wave), binary
    outb(0x40, 0xA9); // divisor low byte  (0xA9 = 169)
    outb(0x40, 0x04); // divisor high byte (0x04 = 4) → 0x04A9 = 1193
}

/// 起動からの経過ミリ秒を返す（1000Hz なので 1tick = 1ms）
pub fn now_ms() -> u64 {
    TICK.load(Ordering::Relaxed)
}

/// 指定ミリ秒だけ HLT でスリープする（割り込みが必要）
pub fn sleep_ms(ms: u64) {
    let target = now_ms().saturating_add(ms);
    while now_ms() < target {
        unsafe { core::arch::asm!("hlt", options(nostack)) };
    }
}

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!(
        "out dx, al",
        in("dx") port,
        in("al") val,
        options(nomem, nostack, preserves_flags)
    );
}
