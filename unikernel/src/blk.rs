// blk.rs — VirtIO-blk レガシー PCI ドライバ
//
// virtio_net.rs と同構造（legacy PCI I/O, BAR0）。
// 単一 virtqueue（queue 0）を使い、3-descriptor チェーンで同期 I/O を行う。
//
// リクエストヘッダ（16バイト）:
//   [0..4]  type_:    u32  (0=read, 1=write)
//   [4..8]  reserved: u32
//   [8..16] sector:   u64

use alloc::alloc::{alloc_zeroed, Layout};
use core::sync::atomic::{fence, Ordering};

use crate::pci;
use crate::serial;

const VIRTIO_VENDOR: u16 = 0x1AF4;
const VIRTIO_BLK_DEVICE: u16 = 0x1001;

// VirtIO レガシー PCI I/O レジスタオフセット（virtio_net.rs と共通）
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_PFN:      u16 = 0x08;
const REG_QUEUE_SIZE:     u16 = 0x0C;
const REG_QUEUE_SEL:      u16 = 0x0E;
const REG_QUEUE_NOTIFY:   u16 = 0x10;
const REG_STATUS:         u16 = 0x12;

const STATUS_RESET:       u8 = 0;
const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER:      u8 = 2;
const STATUS_DRIVER_OK:   u8 = 4;

// VirtIO デスクリプタフラグ
const VRING_DESC_F_NEXT:  u16 = 1; // 次のデスクリプタに連結
const VRING_DESC_F_WRITE: u16 = 2; // デバイスが書き込む（カーネルは読み取り専用）

// VirtIO-blk リクエストタイプ
const VIRTIO_BLK_T_IN:  u32 = 0; // read
const VIRTIO_BLK_T_OUT: u32 = 1; // write

const PAGE_SIZE:   usize = 4096;
const POLL_LIMIT:  usize = 10_000_000;

// 固定デスクリプタインデックス（同期ドライバなので常に同じ3つを使う）
const HEADER_DESC: usize = 0;
const DATA_DESC:   usize = 1;
const STATUS_DESC: usize = 2;

// ─── Virtqueue レイアウト計算（virtio_net.rs と同じ） ────────────────

fn avail_offset(qs: usize) -> usize {
    qs * 16
}

fn used_offset(qs: usize) -> usize {
    let avail_end = avail_offset(qs) + 6 + 2 * qs;
    (avail_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn queue_mem_size(qs: usize) -> usize {
    let used_end = used_offset(qs) + 6 + 8 * qs;
    (used_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

// ─── デスクリプタ書き込みヘルパー ────────────────────────────────────

unsafe fn desc_write(mem: *mut u8, i: usize, addr: u64, len: u32, flags: u16, next: u16) {
    let p = mem.add(i * 16);
    (p         as *mut u64).write_volatile(addr);
    (p.add(8)  as *mut u32).write_volatile(len);
    (p.add(12) as *mut u16).write_volatile(flags);
    (p.add(14) as *mut u16).write_volatile(next);
}

// ─── VirtIO-blk デバイス ─────────────────────────────────────────────

pub struct VirtioBlkDev {
    io_base:    u16,
    queue_size: usize,
    q_mem:      *mut u8,
    next_avail: u16,
    last_used:  u16,
}

unsafe impl Send for VirtioBlkDev {}
unsafe impl Sync for VirtioBlkDev {}

impl VirtioBlkDev {
    /// PCI デバイスを検索して初期化する。見つからなければ None。
    pub fn init() -> Option<Self> {
        let dev = pci::find(VIRTIO_VENDOR, VIRTIO_BLK_DEVICE)?;
        if dev.bar0 & 0x1 == 0 {
            serial::print("[BLK] BAR0 is not I/O BAR\n");
            return None;
        }
        let io_base = (dev.bar0 & !0x3) as u16;

        unsafe {
            // RESET → ACKNOWLEDGE → DRIVER
            outb(io_base + REG_STATUS, STATUS_RESET);
            outb(io_base + REG_STATUS, STATUS_ACKNOWLEDGE);
            outb(io_base + REG_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
            outl(io_base + REG_GUEST_FEATURES, 0);

            // Queue 0 を選択してサイズを取得
            outw(io_base + REG_QUEUE_SEL, 0);
            let queue_size = inw(io_base + REG_QUEUE_SIZE) as usize;
            if queue_size == 0 {
                serial::print("[BLK] queue size = 0\n");
                return None;
            }

            // Virtqueue メモリを確保してデバイスに通知
            let qmem_size = queue_mem_size(queue_size);
            let layout = Layout::from_size_align(qmem_size, PAGE_SIZE).unwrap();
            let q_mem = alloc_zeroed(layout);
            if q_mem.is_null() {
                serial::print("[BLK] queue alloc failed\n");
                return None;
            }

            let pfn = q_mem as u32 / PAGE_SIZE as u32;
            outl(io_base + REG_QUEUE_PFN, pfn);

            outb(io_base + REG_STATUS,
                STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK);

            Some(VirtioBlkDev { io_base, queue_size, q_mem, next_avail: 0, last_used: 0 })
        }
    }

    /// セクタ読み込み（buf は 512 の倍数バイト）
    pub fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> bool {
        self.do_io(VIRTIO_BLK_T_IN, lba, buf.as_mut_ptr(), buf.len())
    }

    /// セクタ書き込み（data は 512 の倍数バイト）
    pub fn write_sectors(&mut self, lba: u64, data: &[u8]) -> bool {
        // 書き込みは device が読むだけなので const ptr をキャストして使う
        self.do_io(VIRTIO_BLK_T_OUT, lba, data.as_ptr() as *mut u8, data.len())
    }

    /// 3-descriptor チェーンで同期 I/O を実行する
    fn do_io(&mut self, req_type: u32, lba: u64, data_ptr: *mut u8, data_len: usize) -> bool {
        // スタック上にヘッダ・ステータスを確保（do_io が返るまで生存保証あり）
        let mut hdr = [0u8; 16];
        let mut sts = [0u8; 1];

        // ヘッダを埋める
        unsafe {
            (hdr.as_mut_ptr()       as *mut u32).write_unaligned(req_type);
            (hdr.as_mut_ptr().add(4) as *mut u32).write_unaligned(0);
            (hdr.as_mut_ptr().add(8) as *mut u64).write_unaligned(lba);
        }

        let qs = self.queue_size;

        unsafe {
            // Desc 0: ヘッダ（デバイスが読む）
            desc_write(self.q_mem, HEADER_DESC,
                hdr.as_ptr() as u64, 16,
                VRING_DESC_F_NEXT, DATA_DESC as u16);

            // Desc 1: データ（read=デバイスが書く WRITE フラグ、write=デバイスが読む）
            let data_flags = VRING_DESC_F_NEXT
                | if req_type == VIRTIO_BLK_T_IN { VRING_DESC_F_WRITE } else { 0 };
            desc_write(self.q_mem, DATA_DESC,
                data_ptr as u64, data_len as u32,
                data_flags, STATUS_DESC as u16);

            // Desc 2: ステータス（デバイスが書く）
            desc_write(self.q_mem, STATUS_DESC,
                sts.as_mut_ptr() as u64, 1,
                VRING_DESC_F_WRITE, 0);

            // Avail ring にヘッダデスクリプタ（desc 0）を追加
            let avail_slot = (self.next_avail % qs as u16) as usize;
            let avail_ring = self.q_mem.add(avail_offset(qs) + 4 + avail_slot * 2) as *mut u16;
            avail_ring.write_volatile(HEADER_DESC as u16);
            fence(Ordering::SeqCst);
            self.next_avail = self.next_avail.wrapping_add(1);
            let avail_idx = self.q_mem.add(avail_offset(qs) + 2) as *mut u16;
            avail_idx.write_volatile(self.next_avail);
            fence(Ordering::SeqCst);

            // デバイスに通知
            outw(self.io_base + REG_QUEUE_NOTIFY, 0);

            // Used ring が更新されるまでポーリング
            let used_idx = self.q_mem.add(used_offset(qs) + 2) as *mut u16;
            let mut done = false;
            for _ in 0..POLL_LIMIT {
                fence(Ordering::SeqCst);
                if used_idx.read_volatile() != self.last_used {
                    done = true;
                    break;
                }
            }
            if !done {
                serial::print("[BLK] I/O timeout\n");
                return false;
            }
            self.last_used = self.last_used.wrapping_add(1);
        }

        sts[0] == 0 // 0 = VIRTIO_BLK_S_OK
    }
}

// ─── I/O ポートヘルパー ──────────────────────────────────────────────

unsafe fn outb(port: u16, val: u8) {
    core::arch::asm!("out dx, al",  in("dx") port, in("al")  val, options(nomem, nostack, preserves_flags));
}
unsafe fn outw(port: u16, val: u16) {
    core::arch::asm!("out dx, ax",  in("dx") port, in("ax")  val, options(nomem, nostack, preserves_flags));
}
unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack, preserves_flags));
}
unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!("in ax, dx",   out("ax") v, in("dx") port, options(nomem, nostack, preserves_flags));
    v
}
