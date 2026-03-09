#![no_std]
#![no_main]

extern crate alloc;

mod serial;
mod fw_cfg;
mod pci;
mod virtio_net;
mod vsock;
mod net;
mod wasm_rt;
mod rtc;
mod idt;
mod registry;
mod timer;
mod blk;
mod store;

use store::KvStore;

use alloc::vec::Vec;
use core::arch::global_asm;
use linked_list_allocator::LockedHeap;

// boot.s をこのコンパイル単位に組み込む
global_asm!(include_str!("boot.s"), options(att_syntax));

// ─── グローバルアロケータ ───────────────────────────────────────
const HEAP_SIZE: usize = 16 * 1024 * 1024; // 16MB（smoltcp バッファ用に拡張）

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

static mut HEAP: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

/// ブート後に呼ばれる Rust エントリポイント
#[no_mangle]
pub extern "C" fn kernel_main() -> ! {
    serial::init();
    serial::print("[BOOT OK]\n");

    // ─── Phase 1: ヒープ初期化 ────────────────────────────────────
    unsafe {
        ALLOCATOR
            .lock()
            .init(core::ptr::addr_of_mut!(HEAP) as *mut u8, HEAP_SIZE);
    }
    serial::print("[HEAP INIT]\n");

    let mut v: Vec<u32> = Vec::new();
    v.push(10); v.push(20); v.push(30);
    assert!(v.len() == 3 && v[0] == 10 && v[1] == 20 && v[2] == 30);
    serial::print("[ALLOC OK]\n");
    serial::print("[unikernel] Phase 1 complete\n");

    // ─── IDT + PIC 初期化（HLT によるアイドル CPU 削減） ─────────────
    idt::init();
    serial::print("[IDT] interrupts enabled\n");

    // ─── Phase 2: シークレット取得（vsock 優先 → fw_cfg フォールバック）
    serial::print("[SECRET] trying vsock...\n");
    serial::print("[VSOCK] init...\n");
    let got_secret = match vsock::fetch_secret("api_key") {
        Some(bytes) => {
            let s = core::str::from_utf8(&bytes)
                .unwrap_or("(invalid utf8)")
                .trim_end_matches(|c: char| c == '\n' || c == '\r' || c == '\0');
            serial::print("[VSOCK] secret: ");
            serial::print(s);
            serial::print("\n");
            serial::print("[VSOCK OK]\n");
            true
        }
        None => {
            serial::print("[VSOCK] not available, falling back to fw_cfg\n");
            if fw_cfg::is_present() {
                serial::print("[FW_CFG] found\n");
                match fw_cfg::find_file("opt/secret") {
                    Some((selector, size)) => {
                        let mut buf = [0u8; 256];
                        let len = (size as usize).min(buf.len());
                        fw_cfg::read(selector, &mut buf[..len]);
                        let s = core::str::from_utf8(&buf[..len])
                            .unwrap_or("(invalid utf8)")
                            .trim_end_matches(|c: char| c == '\n' || c == '\r' || c == '\0');
                        serial::print("[SECRET] ");
                        serial::print(s);
                        serial::print("\n");
                        serial::print("[SECRET OK]\n");
                        true
                    }
                    None => { serial::print("[FW_CFG] opt/secret not found\n"); false }
                }
            } else {
                serial::print("[FW_CFG] not present\n");
                false
            }
        }
    };
    let _ = got_secret;
    serial::print("[unikernel] Phase 2 complete\n");

    // ─── Phase 3/4: VirtIO-net + TCP/IP + HTTP サーバー ──────────
    match virtio_net::VirtioNetDev::init() {
        Ok(mut dev) => {
            serial::print("[NET] MAC: ");
            for (i, b) in dev.mac.iter().enumerate() {
                print_hex(*b);
                if i < 5 { serial::print(":"); }
            }
            serial::print("\n");
            serial::print("[NET OK]\n");
            serial::print("[unikernel] Phase 3 complete\n");

            // VirtIO-blk 初期化（オプション）
            let mut store: Option<KvStore> = match blk::VirtioBlkDev::init() {
                Some(blk) => {
                    serial::print("[BLK OK]\n");
                    Some(KvStore::open(blk))
                }
                None => {
                    serial::print("[BLK] not present\n");
                    None
                }
            };

            // Layer B: レジストリから全 WASM と静的ファイルを取得してルーターに登録
            let (fetched_list, static_files, dev_back) = registry::fetch_all(dev);
            dev = dev_back;

            // Vec<(route, WasmApp)> を組み立てる
            let mut wasm_routes: alloc::vec::Vec<(alloc::vec::Vec<u8>, wasm_rt::WasmApp)> =
                alloc::vec::Vec::new();
            for (route, bytes) in fetched_list {
                if let Some(app) = wasm_rt::WasmApp::from_bytes(&bytes) {
                    serial::print("[WASM OK] route=");
                    serial::print(core::str::from_utf8(&route).unwrap_or("?"));
                    serial::print("\n");
                    wasm_routes.push((route, app));
                }
            }
            if wasm_routes.is_empty() {
                serial::print("[WASM] no WASM loaded, using static response\n");
            }

            // Phase 4: HTTP サーバーとして動き続ける（返らない）
            net::run_http_server(dev, wasm_routes, static_files, store);
        }
        Err(e) => {
            serial::print("[NET ERR] ");
            serial::print(e);
            serial::print("\n");
            loop { unsafe { core::arch::asm!("hlt") }; }
        }
    }
}

/// 1バイトを "xx" 形式でシリアルに出力する
fn print_hex(b: u8) {
    const HEX: &[u8] = b"0123456789abcdef";
    let hi = HEX[(b >> 4) as usize] as char;
    let lo = HEX[(b & 0xF) as usize] as char;
    let s = [hi as u8, lo as u8];
    serial::print(core::str::from_utf8(&s).unwrap_or("??"));
}


/// パニック時のハンドラ（no_std では必須）
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    serial::print("[PANIC]\n");
    loop { unsafe { core::arch::asm!("hlt") }; }
}
