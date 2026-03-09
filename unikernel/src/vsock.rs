// vsock.rs — VirtIO-vsock モダン PCI ドライバ
//
// Alpine Linux (CID=2) の vsock サーバー (port=1234) に接続し、
// シークレット名を送ってシークレット値を受け取る。
//
// PCI: vendor=0x1AF4, device=0x1053 (non-transitional modern virtio)
// 全設定は BAR4 の MMIO 空間を通じて行う（legacy I/O ポートは使わない）。
//
// BAR4 MMIO レイアウト（QEMU デフォルト）:
//   +0x0000: COMMON_CFG  (type=1)
//   +0x1000: ISR_CFG     (type=3)
//   +0x2000: DEVICE_CFG  (type=4)  -- guest_cid u64
//   +0x3000: NOTIFY_CFG  (type=2)  -- queue kick アドレス

use alloc::alloc::{alloc_zeroed, Layout};
use alloc::vec::Vec;
use core::sync::atomic::{fence, Ordering};

use crate::pci;

// ─── PCI IDs ─────────────────────────────────────────────────────
const VIRTIO_VENDOR:       u16 = 0x1AF4;
const VIRTIO_VSOCK_DEVICE: u16 = 0x1053;

// ─── Virtio PCI Capability cfg_type ──────────────────────────────
const CAP_COMMON: u8 = 1;
const CAP_NOTIFY: u8 = 2;

// ─── COMMON_CFG レジスタオフセット ───────────────────────────────
const OFF_DRV_FEAT_SEL: u64 = 8;
const OFF_DRV_FEAT:     u64 = 12;
const OFF_MSIX_VEC:     u64 = 16;
const OFF_DEV_STATUS:   u64 = 20;
const OFF_Q_SELECT:     u64 = 22;
const OFF_Q_SIZE:       u64 = 24;
const OFF_Q_MSIX:       u64 = 26;
const OFF_Q_ENABLE:     u64 = 28;
const OFF_Q_NOTIFY_OFF: u64 = 30;
const OFF_Q_DESC:       u64 = 32;
const OFF_Q_DRIVER:     u64 = 40;
const OFF_Q_DEVICE:     u64 = 48;

// ─── Device status bits ───────────────────────────────────────────
const STATUS_RESET:       u8 = 0;
const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER:      u8 = 2;
const STATUS_DRIVER_OK:   u8 = 4;
const STATUS_FEATURES_OK: u8 = 8;

// ─── Feature bits ─────────────────────────────────────────────────
const VIRTIO_F_VERSION_1: u32 = 1; // bit 32（高 word、selector=1）

// ─── Queue indices ────────────────────────────────────────────────
const QUEUE_RX:    u16 = 0;
const QUEUE_TX:    u16 = 1;
const QUEUE_EVENT: u16 = 2;

// ─── メモリ定数 ──────────────────────────────────────────────────
const PAGE_SIZE:  usize = 4096;
const QUEUE_SIZE: usize = 8;   // 使用するキューサイズ（device_max 以下の2の冪）
const BUF_SIZE:   usize = 4096;

// ─── vsock プロトコル定数 ─────────────────────────────────────────
const VSOCK_TYPE_STREAM:      u16 = 1;
const VSOCK_OP_REQUEST:       u16 = 1;
const VSOCK_OP_RESPONSE:      u16 = 2;
const VSOCK_OP_RST:           u16 = 3;
const VSOCK_OP_RW:            u16 = 5;
const VSOCK_OP_CREDIT_UPDATE: u16 = 6;

pub const GUEST_CID: u64 = 3;
pub const HOST_CID:  u64 = 2;
pub const HOST_PORT: u32 = 1234;
const GUEST_PORT:    u32 = 40000;
const BUF_ALLOC:     u32 = 65536;
const HDR_SIZE:      usize = 44;

// ─── ファイルI/O用 vsock 定数 ──────────────────────────────────
const HOST_FILE_PORT:  u32 = 1235;
const GUEST_FILE_PORT: u32 = 40001;
const FILE_OP_READ:    u8 = 0x01;
const FILE_OP_WRITE:   u8 = 0x02;
const FILE_OP_LIST:    u8 = 0x03;

// ─── MMIO ヘルパー ───────────────────────────────────────────────
unsafe fn mm8r(addr: u64)       -> u8  { core::ptr::read_volatile(addr as *const u8) }
unsafe fn mm8w(addr: u64, v: u8)       { core::ptr::write_volatile(addr as *mut u8, v) }
unsafe fn mm16r(addr: u64)      -> u16 { core::ptr::read_volatile(addr as *const u16) }
unsafe fn mm16w(addr: u64, v: u16)     { core::ptr::write_volatile(addr as *mut u16, v) }
unsafe fn mm32w(addr: u64, v: u32)     { core::ptr::write_volatile(addr as *mut u32, v) }
unsafe fn mm64w(addr: u64, v: u64)     { core::ptr::write_volatile(addr as *mut u64, v) }

// ─── Virtqueue レイアウト計算 ─────────────────────────────────────
fn avail_offset(qs: usize) -> usize { qs * 16 }

fn used_offset(qs: usize) -> usize {
    let avail_end = avail_offset(qs) + 6 + 2 * qs;
    (avail_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

fn queue_mem_size(qs: usize) -> usize {
    let used_end = used_offset(qs) + 6 + 8 * qs;
    (used_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1)
}

// ─── Virtqueue ポインタアクセサ ───────────────────────────────────
unsafe fn desc_write(mem: *mut u8, i: usize, addr: u64, len: u32, flags: u16) {
    let p = mem.add(i * 16);
    (p        as *mut u64).write_volatile(addr);
    (p.add(8) as *mut u32).write_volatile(len);
    (p.add(12) as *mut u16).write_volatile(flags);
    (p.add(14) as *mut u16).write_volatile(0);
}

unsafe fn avail_idx(mem: *mut u8, qs: usize) -> *mut u16 {
    mem.add(avail_offset(qs) + 2) as *mut u16
}
unsafe fn avail_ring(mem: *mut u8, qs: usize, i: usize) -> *mut u16 {
    mem.add(avail_offset(qs) + 4 + i * 2) as *mut u16
}
unsafe fn used_idx(mem: *mut u8, qs: usize) -> *mut u16 {
    mem.add(used_offset(qs) + 2) as *mut u16
}
unsafe fn used_id(mem: *mut u8, qs: usize, i: usize) -> *mut u32 {
    mem.add(used_offset(qs) + 4 + i * 8) as *mut u32
}
unsafe fn used_len(mem: *mut u8, qs: usize, i: usize) -> *mut u32 {
    mem.add(used_offset(qs) + 4 + i * 8 + 4) as *mut u32
}

// ─── vsock パケットヘッダ操作 ────────────────────────────────────
// ヘッダフォーマット（44バイト）:
//   [0..8]  src_cid
//   [8..16] dst_cid
//   [16..20] src_port
//   [20..24] dst_port
//   [24..28] len（データ部分のバイト数）
//   [28..30] type（1=STREAM）
//   [30..32] op
//   [32..36] flags
//   [36..40] buf_alloc
//   [40..44] fwd_cnt

unsafe fn write_hdr(buf: *mut u8,
                    src_cid: u64, dst_cid: u64,
                    src_port: u32, dst_port: u32,
                    data_len: u32, op: u16,
                    buf_alloc: u32, fwd_cnt: u32) {
    (buf.add(0)  as *mut u64).write_unaligned(src_cid);
    (buf.add(8)  as *mut u64).write_unaligned(dst_cid);
    (buf.add(16) as *mut u32).write_unaligned(src_port);
    (buf.add(20) as *mut u32).write_unaligned(dst_port);
    (buf.add(24) as *mut u32).write_unaligned(data_len);
    (buf.add(28) as *mut u16).write_unaligned(VSOCK_TYPE_STREAM);
    (buf.add(30) as *mut u16).write_unaligned(op);
    (buf.add(32) as *mut u32).write_unaligned(0u32);
    (buf.add(36) as *mut u32).write_unaligned(buf_alloc);
    (buf.add(40) as *mut u32).write_unaligned(fwd_cnt);
}

unsafe fn read_op(buf: *const u8) -> u16 {
    (buf.add(30) as *const u16).read_unaligned()
}

// ─── PCI Capability スキャン ──────────────────────────────────────
struct CapInfo {
    bar:            u8,
    offset:         u32,
    cap_cfg_offset: u8, // PCI 設定空間内のこの Cap のオフセット（追加フィールド読み取り用）
}

fn find_cap(dev: &pci::PciDevice, cfg_type: u8) -> Option<CapInfo> {
    let status = pci::read16(dev.bus, dev.slot, dev.func, 0x06);
    if status & 0x10 == 0 { return None; }

    let mut ptr = pci::read8(dev.bus, dev.slot, dev.func, 0x34);
    let mut guard = 0u8;
    while ptr != 0 && guard < 32 {
        guard += 1;
        let cap_id  = pci::read8(dev.bus, dev.slot, dev.func, ptr);
        let cap_len = pci::read8(dev.bus, dev.slot, dev.func, ptr + 2);
        let next    = pci::read8(dev.bus, dev.slot, dev.func, ptr + 1);
        if cap_id == 0x09 && cap_len >= 16 {
            let ct = pci::read8(dev.bus, dev.slot, dev.func, ptr + 3);
            if ct == cfg_type {
                let bar    = pci::read8(dev.bus, dev.slot, dev.func, ptr + 4);
                let offset = pci::read32(dev.bus, dev.slot, dev.func, ptr + 8);
                return Some(CapInfo { bar, offset, cap_cfg_offset: ptr });
            }
        }
        ptr = next;
    }
    None
}

fn bar_addr(dev: &pci::PciDevice, bar_idx: u8) -> u64 {
    let off = 0x10 + bar_idx * 4;
    let val = pci::read32(dev.bus, dev.slot, dev.func, off);
    if val & 1 == 1 { return 0; } // I/O BAR は無視
    let addr32 = val & !0xF;
    if (val >> 1) & 0x3 == 2 { // 64-bit BAR
        let high = pci::read32(dev.bus, dev.slot, dev.func, off + 4);
        ((high as u64) << 32) | (addr32 as u64)
    } else {
        addr32 as u64
    }
}

// ─── VsockDev ────────────────────────────────────────────────────
pub struct VsockDev {
    common_base:   u64,
    notify_base:   u64,
    notify_mult:   u32,

    rx_mem:        *mut u8,
    rx_bufs:       *mut u8,
    rx_last_used:  u16,
    rx_next_avail: u16,
    rx_notify_off: u16,

    tx_mem:        *mut u8,
    tx_bufs:       *mut u8,
    tx_next_avail: u16,
    tx_notify_off: u16,

    fwd_cnt:       u32,
}

unsafe impl Send for VsockDev {}
unsafe impl Sync for VsockDev {}

impl VsockDev {
    pub fn init() -> Option<Self> {
        let dev = pci::find(VIRTIO_VENDOR, VIRTIO_VSOCK_DEVICE)?;

        let common_cap = find_cap(&dev, CAP_COMMON)?;
        let notify_cap = find_cap(&dev, CAP_NOTIFY)?;

        let cbar = bar_addr(&dev, common_cap.bar);
        let nbar = bar_addr(&dev, notify_cap.bar);
        if cbar == 0 || nbar == 0 { return None; }

        let common_base  = cbar + common_cap.offset as u64;
        let notify_base  = nbar + notify_cap.offset as u64;
        // notify_off_multiplier は Cap オフセット+16 の u32
        let notify_mult  = pci::read32(dev.bus, dev.slot, dev.func,
                                       notify_cap.cap_cfg_offset + 16);

        let q_layout    = Layout::from_size_align(queue_mem_size(QUEUE_SIZE), PAGE_SIZE).unwrap();
        let bufs_layout = Layout::from_size_align(QUEUE_SIZE * BUF_SIZE, PAGE_SIZE).unwrap();
        let ev_layout   = Layout::from_size_align(PAGE_SIZE, PAGE_SIZE).unwrap();

        unsafe {
            // ── 1. RESET ──────────────────────────────────────────
            mm8w(common_base + OFF_DEV_STATUS, STATUS_RESET);
            fence(Ordering::SeqCst);

            // ── 2. ACKNOWLEDGE → DRIVER ───────────────────────────
            mm8w(common_base + OFF_DEV_STATUS, STATUS_ACKNOWLEDGE);
            mm8w(common_base + OFF_DEV_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);

            // ── 3. Feature negotiation（VIRTIO_F_VERSION_1 必須） ──
            mm32w(common_base + OFF_DRV_FEAT_SEL, 0);
            mm32w(common_base + OFF_DRV_FEAT, 0);
            mm32w(common_base + OFF_DRV_FEAT_SEL, 1);
            mm32w(common_base + OFF_DRV_FEAT, VIRTIO_F_VERSION_1);

            // ── 4. FEATURES_OK ────────────────────────────────────
            mm8w(common_base + OFF_DEV_STATUS,
                STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_FEATURES_OK);
            fence(Ordering::SeqCst);
            if mm8r(common_base + OFF_DEV_STATUS) & STATUS_FEATURES_OK == 0 {
                return None;
            }

            // MSI-X 無効化（0xFFFF = ベクタなし）
            mm16w(common_base + OFF_MSIX_VEC, 0xFFFF);

            // ── 5. RX キューセットアップ ──────────────────────────
            mm16w(common_base + OFF_Q_SELECT, QUEUE_RX);
            mm16w(common_base + OFF_Q_SIZE, QUEUE_SIZE as u16);
            mm16w(common_base + OFF_Q_MSIX, 0xFFFF);
            let rx_notify_off = mm16r(common_base + OFF_Q_NOTIFY_OFF);

            let rx_mem  = alloc_zeroed(q_layout);
            let rx_bufs = alloc_zeroed(bufs_layout);
            assert!(!rx_mem.is_null() && !rx_bufs.is_null());

            for i in 0..QUEUE_SIZE {
                let buf = rx_bufs.add(i * BUF_SIZE) as u64;
                desc_write(rx_mem, i, buf, BUF_SIZE as u32, 0x2); // WRITE flag
                avail_ring(rx_mem, QUEUE_SIZE, i).write_volatile(i as u16);
            }
            fence(Ordering::SeqCst);
            avail_idx(rx_mem, QUEUE_SIZE).write_volatile(QUEUE_SIZE as u16);
            fence(Ordering::SeqCst);

            mm64w(common_base + OFF_Q_DESC,
                  rx_mem as u64);
            mm64w(common_base + OFF_Q_DRIVER,
                  rx_mem.add(avail_offset(QUEUE_SIZE)) as u64);
            mm64w(common_base + OFF_Q_DEVICE,
                  rx_mem.add(used_offset(QUEUE_SIZE)) as u64);
            mm16w(common_base + OFF_Q_ENABLE, 1);

            // ── 6. TX キューセットアップ ──────────────────────────
            mm16w(common_base + OFF_Q_SELECT, QUEUE_TX);
            mm16w(common_base + OFF_Q_SIZE, QUEUE_SIZE as u16);
            mm16w(common_base + OFF_Q_MSIX, 0xFFFF);
            let tx_notify_off = mm16r(common_base + OFF_Q_NOTIFY_OFF);

            let tx_mem  = alloc_zeroed(q_layout);
            let tx_bufs = alloc_zeroed(bufs_layout);
            assert!(!tx_mem.is_null() && !tx_bufs.is_null());

            mm64w(common_base + OFF_Q_DESC,   tx_mem as u64);
            mm64w(common_base + OFF_Q_DRIVER,
                  tx_mem.add(avail_offset(QUEUE_SIZE)) as u64);
            mm64w(common_base + OFF_Q_DEVICE,
                  tx_mem.add(used_offset(QUEUE_SIZE)) as u64);
            mm16w(common_base + OFF_Q_ENABLE, 1);

            // ── 7. EVENT キュー（最小限、バッファなし） ───────────
            mm16w(common_base + OFF_Q_SELECT, QUEUE_EVENT);
            mm16w(common_base + OFF_Q_SIZE, 2);
            mm16w(common_base + OFF_Q_MSIX, 0xFFFF);
            let ev_mem = alloc_zeroed(ev_layout);
            assert!(!ev_mem.is_null());
            mm64w(common_base + OFF_Q_DESC,   ev_mem as u64);
            mm64w(common_base + OFF_Q_DRIVER,
                  ev_mem.add(avail_offset(2)) as u64);
            mm64w(common_base + OFF_Q_DEVICE,
                  ev_mem.add(used_offset(2)) as u64);
            mm16w(common_base + OFF_Q_ENABLE, 1);

            // ── 8. DRIVER_OK ──────────────────────────────────────
            mm8w(common_base + OFF_DEV_STATUS,
                STATUS_ACKNOWLEDGE | STATUS_DRIVER |
                STATUS_FEATURES_OK | STATUS_DRIVER_OK);
            fence(Ordering::SeqCst);

            // RX キューを kick（バッファ準備完了を通知）
            let rx_addr = notify_base + rx_notify_off as u64 * notify_mult as u64;
            mm16w(rx_addr, QUEUE_RX);

            Some(VsockDev {
                common_base, notify_base, notify_mult,
                rx_mem, rx_bufs,
                rx_last_used: 0,
                rx_next_avail: QUEUE_SIZE as u16,
                rx_notify_off,
                tx_mem, tx_bufs,
                tx_next_avail: 0,
                tx_notify_off,
                fwd_cnt: 0,
            })
        }
    }

    // ─── TX ──────────────────────────────────────────────────────
    unsafe fn send_raw(&mut self, hdr: &[u8; HDR_SIZE], data: &[u8]) {
        let total = HDR_SIZE + data.len();
        assert!(total <= BUF_SIZE);

        let slot = (self.tx_next_avail as usize) % QUEUE_SIZE;
        let buf  = self.tx_bufs.add(slot * BUF_SIZE);

        core::ptr::copy_nonoverlapping(hdr.as_ptr(), buf, HDR_SIZE);
        if !data.is_empty() {
            core::ptr::copy_nonoverlapping(data.as_ptr(), buf.add(HDR_SIZE), data.len());
        }

        desc_write(self.tx_mem, slot, buf as u64, total as u32, 0);
        avail_ring(self.tx_mem, QUEUE_SIZE, slot).write_volatile(slot as u16);
        fence(Ordering::SeqCst);
        self.tx_next_avail = self.tx_next_avail.wrapping_add(1);
        avail_idx(self.tx_mem, QUEUE_SIZE).write_volatile(self.tx_next_avail);
        fence(Ordering::SeqCst);

        let addr = self.notify_base
            + self.tx_notify_off as u64 * self.notify_mult as u64;
        mm16w(addr, QUEUE_TX);
    }

    // ─── RX ──────────────────────────────────────────────────────
    fn try_recv(&mut self) -> Option<(u16, Vec<u8>)> {
        unsafe {
            fence(Ordering::SeqCst);
            let used = used_idx(self.rx_mem, QUEUE_SIZE).read_volatile();
            if used == self.rx_last_used { return None; }

            let ring_i  = (self.rx_last_used as usize) % QUEUE_SIZE;
            let desc_id = used_id(self.rx_mem, QUEUE_SIZE, ring_i).read_volatile() as usize;
            let pkt_len = used_len(self.rx_mem, QUEUE_SIZE, ring_i).read_volatile() as usize;
            self.rx_last_used = self.rx_last_used.wrapping_add(1);

            let buf      = self.rx_bufs.add(desc_id * BUF_SIZE);
            let op       = read_op(buf);
            let data_len = if pkt_len > HDR_SIZE { pkt_len - HDR_SIZE } else { 0 };

            let mut data = Vec::with_capacity(data_len);
            if data_len > 0 {
                data.extend_from_slice(
                    core::slice::from_raw_parts(buf.add(HDR_SIZE), data_len));
                self.fwd_cnt += data_len as u32;
            }

            // バッファをデバイスに返す
            let avail_slot = (self.rx_next_avail as usize) % QUEUE_SIZE;
            avail_ring(self.rx_mem, QUEUE_SIZE, avail_slot)
                .write_volatile(desc_id as u16);
            fence(Ordering::SeqCst);
            self.rx_next_avail = self.rx_next_avail.wrapping_add(1);
            avail_idx(self.rx_mem, QUEUE_SIZE).write_volatile(self.rx_next_avail);
            fence(Ordering::SeqCst);

            let rx_addr = self.notify_base
                + self.rx_notify_off as u64 * self.notify_mult as u64;
            mm16w(rx_addr, QUEUE_RX);

            Some((op, data))
        }
    }

    fn recv_op(&mut self, want_op: u16) -> Option<Vec<u8>> {
        for _ in 0..50_000_000u32 {
            if let Some((op, data)) = self.try_recv() {
                match op {
                    VSOCK_OP_CREDIT_UPDATE => { self.send_hdr(VSOCK_OP_CREDIT_UPDATE); }
                    VSOCK_OP_RST           => return None,
                    o if o == want_op      => return Some(data),
                    _                      => {}
                }
            }
            unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
        }
        None
    }

    // ─── パケット組み立て ─────────────────────────────────────────
    fn make_hdr(&self, data_len: u32, op: u16) -> [u8; HDR_SIZE] {
        let mut buf = [0u8; HDR_SIZE];
        unsafe {
            write_hdr(buf.as_mut_ptr(),
                GUEST_CID, HOST_CID, GUEST_PORT, HOST_PORT,
                data_len, op, BUF_ALLOC, self.fwd_cnt);
        }
        buf
    }

    fn send_hdr(&mut self, op: u16) {
        let hdr = self.make_hdr(0, op);
        unsafe { self.send_raw(&hdr, &[]); }
    }

    // ─── 公開 API ────────────────────────────────────────────────
    pub fn connect(&mut self) -> bool {
        self.send_hdr(VSOCK_OP_REQUEST);
        if self.recv_op(VSOCK_OP_RESPONSE).is_some() {
            // RESPONSE 受信後にクレジット更新を送信（ホストが受信バッファを知る）
            self.send_hdr(VSOCK_OP_CREDIT_UPDATE);
            true
        } else {
            false
        }
    }

    pub fn send_data(&mut self, data: &[u8]) {
        let hdr = self.make_hdr(data.len() as u32, VSOCK_OP_RW);
        unsafe { self.send_raw(&hdr, data); }
    }

    pub fn recv_data(&mut self) -> Option<Vec<u8>> {
        self.recv_op(VSOCK_OP_RW)
    }

    pub fn close(&mut self) {
        self.send_hdr(VSOCK_OP_RST);
    }

    // ─── ポート指定版メソッド（ファイルI/O用） ─────────────────────
    fn make_hdr_port(&self, data_len: u32, op: u16, src_port: u32, dst_port: u32) -> [u8; HDR_SIZE] {
        let mut buf = [0u8; HDR_SIZE];
        unsafe {
            write_hdr(buf.as_mut_ptr(),
                GUEST_CID, HOST_CID, src_port, dst_port,
                data_len, op, BUF_ALLOC, self.fwd_cnt);
        }
        buf
    }

    fn send_hdr_port(&mut self, src_port: u32, dst_port: u32, op: u16) {
        let hdr = self.make_hdr_port(0, op, src_port, dst_port);
        unsafe { self.send_raw(&hdr, &[]); }
    }

    fn drain_rx(&mut self) {
        while self.try_recv().is_some() {}
    }

    fn connect_port(&mut self, src_port: u32, dst_port: u32) -> bool {
        self.drain_rx(); // 前の接続からの残留パケットを破棄
        self.fwd_cnt = 0; // 各接続でフロー制御カウンタをリセット（プロトコル違反防止）
        self.send_hdr_port(src_port, dst_port, VSOCK_OP_REQUEST);
        for _ in 0..50_000_000u32 {
            if let Some((op, _)) = self.try_recv() {
                match op {
                    VSOCK_OP_RESPONSE => {
                        self.send_hdr_port(src_port, dst_port, VSOCK_OP_CREDIT_UPDATE);
                        return true;
                    }
                    VSOCK_OP_RST => { return false; }
                    _ => {}
                }
            }
            unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
        }
        false
    }

    fn send_file_req(&mut self, op: u8, path: &str, data: &[u8]) {
        let path_bytes = path.as_bytes();
        let mut payload = alloc::vec![0u8; 7 + path_bytes.len() + data.len()];
        payload[0] = op;
        payload[1..3].copy_from_slice(&(path_bytes.len() as u16).to_le_bytes());
        payload[3..7].copy_from_slice(&(data.len() as u32).to_le_bytes());
        payload[7..7 + path_bytes.len()].copy_from_slice(path_bytes);
        if !data.is_empty() {
            payload[7 + path_bytes.len()..].copy_from_slice(data);
        }
        let hdr = self.make_hdr_port(payload.len() as u32, VSOCK_OP_RW, GUEST_FILE_PORT, HOST_FILE_PORT);
        unsafe { self.send_raw(&hdr, &payload); }
    }

    fn recv_file_resp(&mut self) -> Option<(bool, Vec<u8>)> {
        let mut header_received = false;
        let mut expected_len: usize = 0;
        let mut status: u8 = 0xFF;
        let mut payload: Vec<u8> = Vec::new();

        for _ in 0..50_000_000u32 {
            if let Some((op, data)) = self.try_recv() {
                match op {
                    VSOCK_OP_CREDIT_UPDATE => {
                        self.send_hdr_port(GUEST_FILE_PORT, HOST_FILE_PORT, VSOCK_OP_CREDIT_UPDATE);
                    }
                    VSOCK_OP_RST => { return None; }
                    VSOCK_OP_RW => {
                        if !header_received {
                            if data.len() < 5 { return None; }
                            status = data[0];
                            expected_len = u32::from_le_bytes(
                                data[1..5].try_into().unwrap_or([0; 4])) as usize;
                            payload.extend_from_slice(&data[5..]);
                            header_received = true;
                        } else {
                            payload.extend_from_slice(&data);
                        }
                        if header_received && payload.len() >= expected_len {
                            payload.truncate(expected_len);
                            return Some((status == 0x00, payload));
                        }
                    }
                    _ => {}
                }
            }
            unsafe { core::arch::asm!("pause", options(nomem, nostack)); }
        }
        None
    }

    fn close_port(&mut self, src_port: u32, dst_port: u32) {
        self.send_hdr_port(src_port, dst_port, VSOCK_OP_RST);
    }
}

// ─── ファイルI/O用グローバルデバイス ────────────────────────────
static mut FILE_DEV: Option<VsockDev> = None;

fn ensure_file_dev() -> Option<&'static mut VsockDev> {
    unsafe {
        if FILE_DEV.is_none() {
            crate::serial::print("[VSOCK] init file device\n");
            FILE_DEV = VsockDev::init();
        }
        FILE_DEV.as_mut()
    }
}

pub fn file_read(path: &str) -> Option<Vec<u8>> {
    let dev = ensure_file_dev()?;
    if !dev.connect_port(GUEST_FILE_PORT, HOST_FILE_PORT) {
        crate::serial::print("[VSOCK FILE] connect failed\n");
        return None;
    }
    dev.send_file_req(FILE_OP_READ, path, &[]);
    let result = dev.recv_file_resp();
    dev.close_port(GUEST_FILE_PORT, HOST_FILE_PORT);
    result.and_then(|(ok, data)| if ok { Some(data) } else { None })
}

pub fn file_write(path: &str, data: &[u8]) -> bool {
    let dev = match ensure_file_dev() {
        Some(d) => d,
        None => return false,
    };
    if !dev.connect_port(GUEST_FILE_PORT, HOST_FILE_PORT) {
        crate::serial::print("[VSOCK FILE] connect failed\n");
        return false;
    }
    dev.send_file_req(FILE_OP_WRITE, path, data);
    let result = dev.recv_file_resp();
    dev.close_port(GUEST_FILE_PORT, HOST_FILE_PORT);
    result.map(|(ok, _)| ok).unwrap_or(false)
}

pub fn file_list(dir: &str) -> Option<Vec<u8>> {
    let dev = ensure_file_dev()?;
    if !dev.connect_port(GUEST_FILE_PORT, HOST_FILE_PORT) {
        crate::serial::print("[VSOCK FILE] connect failed\n");
        return None;
    }
    dev.send_file_req(FILE_OP_LIST, dir, &[]);
    let result = dev.recv_file_resp();
    dev.close_port(GUEST_FILE_PORT, HOST_FILE_PORT);
    result.and_then(|(ok, data)| if ok { Some(data) } else { None })
}

/// vsock 経由でシークレットを取得する。
/// CID=2（Alpine）の port=1234 に接続し、key_name を送って値を受け取る。
/// デバイスは使用後も FILE_DEV として保持し、再初期化を避ける。
pub fn fetch_secret(key_name: &str) -> Option<Vec<u8>> {
    use crate::serial;
    let mut dev = VsockDev::init()?;
    serial::print("[VSOCK] device init ok\n");

    serial::print("[VSOCK] connecting...\n");
    if !dev.connect() {
        serial::print("[VSOCK] connect failed (no RESPONSE)\n");
        // 失敗しても FILE_DEV としてデバイスを保持
        unsafe { FILE_DEV = Some(dev); }
        return None;
    }
    serial::print("[VSOCK] connected\n");

    let mut req = Vec::with_capacity(key_name.len() + 1);
    req.extend_from_slice(key_name.as_bytes());
    req.push(b'\n');
    dev.send_data(&req);
    serial::print("[VSOCK] key sent, waiting for data...\n");

    let result = dev.recv_data();
    dev.close();
    if result.is_some() {
        serial::print("[VSOCK] data received\n");
    } else {
        serial::print("[VSOCK] recv_data timeout\n");
    }
    // シークレット取得後もデバイスを FILE_DEV として再利用（再初期化を避ける）
    unsafe { FILE_DEV = Some(dev); }
    result
}
