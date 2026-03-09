// idt.rs — IDT（割り込み記述子テーブル）+ 8259A PIC 初期化
//
// 全256ベクタにヌルハンドラを設定し、PIT タイマー割り込み（IRQ 0）を有効化する。
// STI 後に HLT を使ってアイドル時の CPU 消費を削減する。

use core::arch::asm;

// ─── IDT エントリ（64bit モード: 16 バイト） ─────────────────────
#[repr(C)]
struct IdtEntry {
    offset_lo:  u16, // ハンドラアドレス [15:0]
    selector:   u16, // コードセグメント (0x08)
    attrs:      u16, // IST=0, type=0xE (64bit interrupt gate), DPL=0, P=1
    offset_mid: u16, // ハンドラアドレス [31:16]
    offset_hi:  u32, // ハンドラアドレス [63:32]
    _reserved:  u32,
}

impl IdtEntry {
    const fn new(handler: u64) -> Self {
        IdtEntry {
            offset_lo:  handler as u16,
            selector:   0x08,
            attrs:      0x8E00, // P=1, DPL=0, type=0xE
            offset_mid: (handler >> 16) as u16,
            offset_hi:  (handler >> 32) as u32,
            _reserved:  0,
        }
    }
    const fn zero() -> Self {
        IdtEntry { offset_lo: 0, selector: 0, attrs: 0, offset_mid: 0, offset_hi: 0, _reserved: 0 }
    }
}

// ─── IDT テーブル（静的配置） ─────────────────────────────────────
static mut IDT: [IdtEntry; 256] = [const { IdtEntry::zero() }; 256];

#[repr(C, packed)]
struct IdtPtr { limit: u16, base: u64 }

// boot.s で定義したハンドラ
extern "C" {
    fn irq_exception();
    fn irq_exception_err();
    fn irq_pic_master();
    fn irq_pic_slave();
}

// エラーコードをスタックに積む例外ベクタ
const ERR_VECS: &[usize] = &[8, 10, 11, 12, 13, 14, 17, 30];

// ─── 初期化エントリポイント ───────────────────────────────────────
pub fn init() {
    unsafe {
        let exc     = irq_exception     as *const () as u64;
        let exc_err = irq_exception_err as *const () as u64;
        let master  = irq_pic_master    as *const () as u64;
        let slave   = irq_pic_slave     as *const () as u64;

        for i in 0..256usize {
            IDT[i] = if ERR_VECS.contains(&i) {
                IdtEntry::new(exc_err)
            } else if (0x20..0x28).contains(&i) {
                IdtEntry::new(master)
            } else if (0x28..0x30).contains(&i) {
                IdtEntry::new(slave)
            } else {
                IdtEntry::new(exc)
            };
        }

        // 8259A PIC を 0x20-0x2F に再マップ
        pic_init();

        // IDT をロード
        let idt_ptr = &raw const IDT as *const u8;
        let ptr = IdtPtr {
            limit: (core::mem::size_of::<[IdtEntry; 256]>() - 1) as u16,
            base:  idt_ptr as u64,
        };
        asm!("lidt [{ptr}]", ptr = in(reg) &ptr, options(nostack, readonly));

        // 割り込み有効化
        asm!("sti", options(nostack));
    }

    // PIT を 1000Hz に再プログラム（STI 後でも IRQ0 ハンドラは既に設定済み）
    crate::timer::init();
}

// ─── 8259A PIC 初期化 ────────────────────────────────────────────
// マスター → INT 0x20-0x27, スレーブ → INT 0x28-0x2F に再マップ
// IRQ 0（PIT タイマー ~18Hz）のみアンマスク → アイドル時に HLT から復帰
unsafe fn pic_init() {
    // ICW1: カスケード + ICW4 要求
    outb(0x20, 0x11);
    outb(0xA0, 0x11);
    // ICW2: ベクタオフセット
    outb(0x21, 0x20); // マスター: IRQ0 → INT 0x20
    outb(0xA1, 0x28); // スレーブ: IRQ8 → INT 0x28
    // ICW3: カスケード設定
    outb(0x21, 0x04); // マスター: スレーブは IRQ2
    outb(0xA1, 0x02); // スレーブ: カスケード ID = 2
    // ICW4: 8086 モード
    outb(0x21, 0x01);
    outb(0xA1, 0x01);
    // マスク: マスターは IRQ0 のみ有効（0xFE）, スレーブは全マスク（0xFF）
    outb(0x21, 0xFE);
    outb(0xA1, 0xFF);
}

unsafe fn outb(port: u16, val: u8) {
    asm!("out dx, al", in("dx") port, in("al") val, options(nostack));
}
