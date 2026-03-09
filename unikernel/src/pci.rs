// pci.rs — PCI コンフィグレーション空間アクセス（ポート I/O 方式）
//
// アドレスポート 0xCF8 にアドレスを書き、データポート 0xCFC から読む。
// x86 のレガシー PCI アクセス方法。PCIe でも後方互換として動作する。

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

fn make_address(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    0x8000_0000
        | ((bus as u32) << 16)
        | ((slot as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC)
}

unsafe fn outl(port: u16, val: u32) {
    core::arch::asm!(
        "out dx, eax",
        in("dx") port,
        in("eax") val,
        options(nomem, nostack, preserves_flags)
    );
}

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

pub fn read32(bus: u8, slot: u8, func: u8, offset: u8) -> u32 {
    unsafe {
        outl(CONFIG_ADDRESS, make_address(bus, slot, func, offset));
        inl(CONFIG_DATA)
    }
}

pub fn read16(bus: u8, slot: u8, func: u8, offset: u8) -> u16 {
    let val = read32(bus, slot, func, offset & !3);
    ((val >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

pub fn read8(bus: u8, slot: u8, func: u8, offset: u8) -> u8 {
    let val = read32(bus, slot, func, offset & !3);
    ((val >> ((offset & 3) * 8)) & 0xFF) as u8
}

/// vendor_id と device_id が一致する最初の PCI デバイスを返す
#[allow(dead_code)]
pub struct PciDevice {
    pub bus: u8,
    pub slot: u8,
    pub func: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    /// BAR0 の生の値（I/O BAR なら bit0=1、ベースは & !3）
    pub bar0: u32,
}

pub fn find(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    for bus in 0u8..=255 {
        for slot in 0u8..32 {
            let v = read16(bus, slot, 0, 0x00);
            if v == 0xFFFF {
                continue;
            }
            let d = read16(bus, slot, 0, 0x02);
            if v == vendor_id && d == device_id {
                let bar0 = read32(bus, slot, 0, 0x10);
                return Some(PciDevice { bus, slot, func: 0, vendor_id: v, device_id: d, bar0 });
            }
            // マルチファンクション確認
            let header = read8(bus, slot, 0, 0x0E);
            if header & 0x80 != 0 {
                for func in 1u8..8 {
                    let v = read16(bus, slot, func, 0x00);
                    if v == 0xFFFF {
                        continue;
                    }
                    let d = read16(bus, slot, func, 0x02);
                    if v == vendor_id && d == device_id {
                        let bar0 = read32(bus, slot, func, 0x10);
                        return Some(PciDevice { bus, slot, func, vendor_id: v, device_id: d, bar0 });
                    }
                }
            }
        }
    }
    None
}
