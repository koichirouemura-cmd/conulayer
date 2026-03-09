// virtio_net.rs — VirtIO-net レガシー PCI ドライバ（Phase 3/4）
//
// 重要: VirtIO legacy では QUEUE_SIZE はデバイスが報告する値を使わなければならない。
// デバイスが Used ring を書くアドレスは (QueuePFN * 4096 + used_offset) で、
// used_offset = align(desc_table + avail_ring, 4096) となる。
// QUEUE_SIZE を誤った値にするとデバイスが別アドレスに書いてしまいパケットが届かない。

use alloc::alloc::{alloc_zeroed, Layout};
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

use crate::pci;

const VIRTIO_VENDOR: u16 = 0x1AF4;
const VIRTIO_NET_DEVICE: u16 = 0x1000;

// VirtIO レガシー PCI I/O レジスタオフセット
const REG_GUEST_FEATURES: u16 = 0x04;
const REG_QUEUE_PFN:      u16 = 0x08;
const REG_QUEUE_SIZE:     u16 = 0x0C; // デバイスが報告するキューサイズ（読み取り専用）
const REG_QUEUE_SEL:      u16 = 0x0E;
const REG_QUEUE_NOTIFY:   u16 = 0x10;
const REG_STATUS:         u16 = 0x12;
const REG_MAC_BASE:       u16 = 0x14;

const STATUS_RESET:       u8 = 0;
const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER:      u8 = 2;
const STATUS_DRIVER_OK:   u8 = 4;

const QUEUE_RX: u16 = 0;
const QUEUE_TX: u16 = 1;

const PAGE_SIZE: usize = 4096;
const BUF_SIZE:  usize = 2048; // パケットバッファ1枚のサイズ
const VNET_HDR:  usize = 10;   // VirtioNetHdr サイズ

// 実際に使う RX/TX バッファ数（デバイスの QUEUE_SIZE より少なくてよい）
const QUEUE_BUFS: usize = 16;

// ─── Virtqueue オフセット計算（デバイス報告の queue_size に依存）──────
//
// VirtIO legacy の仕様上のレイアウト:
//   offset 0                       : Descriptor table (16 × queue_size bytes)
//   offset 16 × queue_size         : Available ring (6 + 2 × queue_size bytes)
//   offset page_align(上記終端)    : Used ring (6 + 8 × queue_size bytes)
//
// デバイスはこのレイアウトを QueuePFN と queue_size から自分で計算する。
// ドライバも同じ計算を使わなければならない。

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

// ─── ロー・ポインタアクセサ ──────────────────────────────────────────

unsafe fn desc_write(mem: *mut u8, qs: usize, i: usize, addr: u64, len: u32, flags: u16) {
    let p = mem.add(i * 16); // desc offset 0 からの相対
    let _ = qs;              // 将来の検証用
    (p        as *mut u64).write_volatile(addr);
    (p.add(8) as *mut u32).write_volatile(len);
    (p.add(12) as *mut u16).write_volatile(flags);
    (p.add(14) as *mut u16).write_volatile(0); // next
}

unsafe fn avail_idx_ptr(mem: *mut u8, qs: usize) -> *mut u16 {
    mem.add(avail_offset(qs) + 2) as *mut u16
}
unsafe fn avail_ring_ptr(mem: *mut u8, qs: usize, i: usize) -> *mut u16 {
    mem.add(avail_offset(qs) + 4 + i * 2) as *mut u16
}
unsafe fn used_idx_ptr(mem: *mut u8, qs: usize) -> *mut u16 {
    mem.add(used_offset(qs) + 2) as *mut u16
}
unsafe fn used_id_ptr(mem: *mut u8, qs: usize, i: usize) -> *mut u32 {
    mem.add(used_offset(qs) + 4 + i * 8) as *mut u32
}
unsafe fn used_len_ptr(mem: *mut u8, qs: usize, i: usize) -> *mut u32 {
    mem.add(used_offset(qs) + 4 + i * 8 + 4) as *mut u32
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
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    core::arch::asm!("in al, dx",  out("al")  v, in("dx") port, options(nomem, nostack, preserves_flags));
    v
}
unsafe fn inw(port: u16) -> u16 {
    let v: u16;
    core::arch::asm!("in ax, dx",  out("ax")  v, in("dx") port, options(nomem, nostack, preserves_flags));
    v
}

// ─── VirtIO-net デバイス ─────────────────────────────────────────────

pub struct VirtioNetDev {
    io_base:    u16,
    pub mac:    [u8; 6],
    queue_size: usize,   // デバイスが報告した QUEUE_SIZE（リング計算に使う）

    // RX キュー
    rx_mem:        *mut u8,  // virtqueue 共有メモリ（ページ整列）
    rx_bufs:       *mut u8,  // RX パケットバッファ群（QUEUE_BUFS × BUF_SIZE）
    rx_next_avail: u16,
    rx_last_used:  u16,

    // TX キュー
    tx_mem:        *mut u8,  // virtqueue 共有メモリ（ページ整列）
    tx_bufs:       *mut u8,  // TX パケットバッファ群（QUEUE_BUFS × BUF_SIZE）
    tx_next_avail: u16,
}

unsafe impl Send for VirtioNetDev {}
unsafe impl Sync for VirtioNetDev {}

impl VirtioNetDev {
    pub fn init() -> Result<Self, &'static str> {
        let dev = pci::find(VIRTIO_VENDOR, VIRTIO_NET_DEVICE)
            .ok_or("VirtIO-net not found")?;
        if dev.bar0 & 0x1 == 0 {
            return Err("BAR0 is not I/O BAR");
        }
        let io_base = (dev.bar0 & !0x3) as u16;

        unsafe {
            // RESET → ACKNOWLEDGE → DRIVER
            outb(io_base + REG_STATUS, STATUS_RESET);
            outb(io_base + REG_STATUS, STATUS_ACKNOWLEDGE);
            outb(io_base + REG_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
            outl(io_base + REG_GUEST_FEATURES, 0);

            // MAC を読む
            let mut mac = [0u8; 6];
            for (i, b) in mac.iter_mut().enumerate() {
                *b = inb(io_base + REG_MAC_BASE + i as u16);
            }

            // ── RX キューのセットアップ ──
            outw(io_base + REG_QUEUE_SEL, QUEUE_RX);
            // デバイスが報告する QUEUE_SIZE を読む（重要！）
            let queue_size = inw(io_base + REG_QUEUE_SIZE) as usize;

            // デバイスの QUEUE_SIZE に基づいてメモリを確保
            let qmem_size = queue_mem_size(queue_size);
            let q_layout = Layout::from_size_align(qmem_size, PAGE_SIZE).unwrap();

            let rx_mem = alloc_zeroed(q_layout);
            assert!(!rx_mem.is_null(), "rx queue alloc failed");

            // RX バッファ（実際に使う QUEUE_BUFS 枚分）
            let bufs_layout = Layout::from_size_align(QUEUE_BUFS * BUF_SIZE, PAGE_SIZE).unwrap();
            let rx_bufs = alloc_zeroed(bufs_layout);
            assert!(!rx_bufs.is_null(), "rx bufs alloc failed");

            // デスクリプタと avail ring を QUEUE_BUFS 分セットアップ
            for i in 0..QUEUE_BUFS {
                let buf = rx_bufs.add(i * BUF_SIZE) as u64;
                desc_write(rx_mem, queue_size, i, buf, BUF_SIZE as u32, 0x2); // WRITE flag
                avail_ring_ptr(rx_mem, queue_size, i).write_volatile(i as u16);
            }
            fence(Ordering::SeqCst);
            avail_idx_ptr(rx_mem, queue_size).write_volatile(QUEUE_BUFS as u16);
            fence(Ordering::SeqCst);

            let rx_pfn = rx_mem as u32 / PAGE_SIZE as u32;
            outl(io_base + REG_QUEUE_PFN, rx_pfn);

            // ── TX キューのセットアップ ──
            outw(io_base + REG_QUEUE_SEL, QUEUE_TX);
            // TX の QUEUE_SIZE も読む（RX と同じはずだが念のため）
            let _tx_qs = inw(io_base + REG_QUEUE_SIZE) as usize;

            let tx_mem = alloc_zeroed(q_layout); // RX と同じサイズ
            assert!(!tx_mem.is_null(), "tx queue alloc failed");

            let tx_bufs = alloc_zeroed(bufs_layout);
            assert!(!tx_bufs.is_null(), "tx bufs alloc failed");

            let tx_pfn = tx_mem as u32 / PAGE_SIZE as u32;
            outl(io_base + REG_QUEUE_PFN, tx_pfn);

            // DRIVER_OK をセット
            outb(io_base + REG_STATUS,
                STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK);

            // DRIVER_OK 後に RX キューのバッファをデバイスに通知
            outw(io_base + REG_QUEUE_NOTIFY, QUEUE_RX);

            Ok(VirtioNetDev {
                io_base, mac, queue_size,
                rx_mem, rx_bufs,
                rx_next_avail: QUEUE_BUFS as u16,
                rx_last_used: 0,
                tx_mem, tx_bufs,
                tx_next_avail: 0,
            })
        }
    }

    /// 受信パケットを 1 つ取り出す（VirtIO ヘッダを除いた Ethernet フレーム）。
    pub fn try_recv(&mut self) -> Option<Vec<u8>> {
        let qs = self.queue_size;
        unsafe {
            fence(Ordering::SeqCst);
            let used = used_idx_ptr(self.rx_mem, qs).read_volatile();
            if used == self.rx_last_used {
                return None;
            }

            let ring_i = (self.rx_last_used % qs as u16) as usize;
            let desc_id = used_id_ptr(self.rx_mem, qs, ring_i).read_volatile() as usize;
            let pkt_len = used_len_ptr(self.rx_mem, qs, ring_i).read_volatile() as usize;
            self.rx_last_used += 1;

            let buf = self.rx_bufs.add(desc_id * BUF_SIZE);
            let data_len = pkt_len.saturating_sub(VNET_HDR);
            let mut pkt = Vec::with_capacity(data_len);
            pkt.extend_from_slice(core::slice::from_raw_parts(buf.add(VNET_HDR), data_len));

            // バッファをデバイスに返す
            let avail_slot = (self.rx_next_avail % qs as u16) as usize;
            avail_ring_ptr(self.rx_mem, qs, avail_slot).write_volatile(desc_id as u16);
            fence(Ordering::SeqCst);
            self.rx_next_avail += 1;
            avail_idx_ptr(self.rx_mem, qs).write_volatile(self.rx_next_avail);
            fence(Ordering::SeqCst);
            outw(self.io_base + REG_QUEUE_NOTIFY, QUEUE_RX);

            Some(pkt)
        }
    }

    /// Ethernet フレームを送信する（VirtIO ヘッダは自動付与）。
    pub fn send_packet(&mut self, frame: &[u8]) {
        let total = VNET_HDR + frame.len();
        assert!(total <= BUF_SIZE, "packet too large");

        let qs = self.queue_size;
        unsafe {
            let desc_i = (self.tx_next_avail % qs as u16) as usize % QUEUE_BUFS;
            let buf = self.tx_bufs.add(desc_i * BUF_SIZE);

            core::ptr::write_bytes(buf, 0, VNET_HDR);
            core::ptr::copy_nonoverlapping(frame.as_ptr(), buf.add(VNET_HDR), frame.len());

            let desc_slot = (self.tx_next_avail % qs as u16) as usize;
            desc_write(self.tx_mem, qs, desc_slot, buf as u64, total as u32, 0);

            avail_ring_ptr(self.tx_mem, qs, desc_slot).write_volatile(desc_slot as u16);
            fence(Ordering::SeqCst);
            self.tx_next_avail += 1;
            avail_idx_ptr(self.tx_mem, qs).write_volatile(self.tx_next_avail);
            fence(Ordering::SeqCst);
            outw(self.io_base + REG_QUEUE_NOTIFY, QUEUE_TX);
        }
    }
}
