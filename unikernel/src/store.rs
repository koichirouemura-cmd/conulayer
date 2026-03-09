// store.rs — フラット KV ストア（VirtIO-blk 上）
//
// ディスクレイアウト（全値リトルエンディアン、セクタ = 512バイト）:
//   Sector 0:      スーパーブロック
//     [0..4]       magic: 0x554E494B ("UNIK")
//     [4..8]       version: 1
//     [8..12]      entry_count: u32
//   Sectors 1-4:   ディレクトリ（64エントリ × 32バイト = 2048バイト）
//     [0..20]      name: [u8; 20]（null終端）
//     [20..24]     offset_sectors: u32（sector 5 からのオフセット）
//     [24..28]     size: u32（バイト）
//     [28..32]     flags: u32（0=有効, 1=削除済み）
//   Sector 5+:     データ領域（シーケンシャル割り当て）

use alloc::string::String;
use alloc::vec::Vec;

use crate::blk::VirtioBlkDev;
use crate::serial;

const MAGIC: u32 = 0x4B494E55; // "UNIK" LE
const VERSION: u32 = 1;
const DIR_ENTRIES: usize = 64;
const ENTRY_SIZE: usize = 32;
const DIR_SECTORS: u64 = 4;      // sectors 1-4
const DATA_START: u64 = 5;       // sector 5 以降
const SECTOR_SIZE: usize = 512;
const NAME_MAX: usize = 20;

const FLAG_VALID:   u32 = 0;
const FLAG_DELETED: u32 = 1;

pub struct KvStore {
    blk: VirtioBlkDev,
}

impl KvStore {
    /// ブロックデバイスを受け取り、KV ストアを開く。
    /// magic が不一致の場合はディスクをフォーマットする。
    pub fn open(mut blk: VirtioBlkDev) -> Self {
        let mut sb = [0u8; SECTOR_SIZE];
        blk.read_sectors(0, &mut sb);
        let magic = u32::from_le_bytes(sb[0..4].try_into().unwrap_or([0; 4]));
        if magic != MAGIC {
            serial::print("[STORE] formatting disk...\n");
            let mut sb2 = [0u8; SECTOR_SIZE];
            sb2[0..4].copy_from_slice(&MAGIC.to_le_bytes());
            sb2[4..8].copy_from_slice(&VERSION.to_le_bytes());
            sb2[8..12].copy_from_slice(&0u32.to_le_bytes());
            blk.write_sectors(0, &sb2);
            // ディレクトリセクタをゼロで初期化
            let zeros = [0u8; SECTOR_SIZE];
            for i in 0..DIR_SECTORS {
                blk.write_sectors(1 + i, &zeros);
            }
        }
        KvStore { blk }
    }

    /// 指定名のデータを読み込む。見つからなければ None。
    pub fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        let dir = self.read_dir();
        // 最後に書かれた有効エントリを使う（上書き時は古いものが DELETED）
        let mut best: Option<(u32, u32)> = None; // (offset_sectors, size)
        for i in 0..DIR_ENTRIES {
            let e = &dir[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
            let flags = u32::from_le_bytes(e[28..32].try_into().unwrap_or([0; 4]));
            if flags != FLAG_VALID { continue; }
            let entry_name = entry_name_str(e);
            if entry_name == name {
                let offset = u32::from_le_bytes(e[20..24].try_into().unwrap_or([0; 4]));
                let size   = u32::from_le_bytes(e[24..28].try_into().unwrap_or([0; 4]));
                best = Some((offset, size));
            }
        }
        let (offset, size) = best?;

        // データをセクタ単位で読む
        let sectors = (size as usize + SECTOR_SIZE - 1) / SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; sectors * SECTOR_SIZE];
        for i in 0..sectors {
            let lba = DATA_START + offset as u64 + i as u64;
            self.blk.read_sectors(lba, &mut buf[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE]);
        }
        buf.truncate(size as usize);
        Some(buf)
    }

    /// 指定名でデータを書き込む。同名エントリがあれば古いものを DELETED にする。
    pub fn write(&mut self, name: &str, data: &[u8]) -> bool {
        let mut dir = self.read_dir();

        // 既存エントリを DELETED に
        for i in 0..DIR_ENTRIES {
            let e = &mut dir[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
            let flags = u32::from_le_bytes(e[28..32].try_into().unwrap_or([0; 4]));
            if flags == FLAG_VALID && entry_name_str(e) == name {
                e[28..32].copy_from_slice(&FLAG_DELETED.to_le_bytes());
            }
        }

        // 空きエントリを探す
        let mut slot: Option<usize> = None;
        for i in 0..DIR_ENTRIES {
            let e = &dir[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
            let flags = u32::from_le_bytes(e[28..32].try_into().unwrap_or([0; 4]));
            if flags == FLAG_DELETED || e[0] == 0 {
                slot = Some(i);
                break;
            }
        }
        let slot = match slot {
            Some(s) => s,
            None => { serial::print("[STORE] directory full\n"); return false; }
        };

        // データ領域の末尾を計算
        let next_sector = self.find_next_free_sector(&dir);

        // データをセクタ単位で書く
        let sectors = (data.len() + SECTOR_SIZE - 1) / SECTOR_SIZE;
        let mut buf = alloc::vec![0u8; sectors * SECTOR_SIZE];
        buf[..data.len()].copy_from_slice(data);
        for i in 0..sectors {
            let lba = DATA_START + next_sector as u64 + i as u64;
            if !self.blk.write_sectors(lba, &buf[i * SECTOR_SIZE..(i + 1) * SECTOR_SIZE]) {
                serial::print("[STORE] write failed\n");
                return false;
            }
        }

        // ディレクトリエントリを更新
        let e = &mut dir[slot * ENTRY_SIZE..(slot + 1) * ENTRY_SIZE];
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len().min(NAME_MAX - 1);
        e[..NAME_MAX].fill(0);
        e[..name_len].copy_from_slice(&name_bytes[..name_len]);
        e[20..24].copy_from_slice(&next_sector.to_le_bytes());
        e[24..28].copy_from_slice(&(data.len() as u32).to_le_bytes());
        e[28..32].copy_from_slice(&FLAG_VALID.to_le_bytes());

        // ディレクトリを書き戻す
        self.write_dir(&dir);

        // スーパーブロックの entry_count を更新
        self.update_superblock(&dir);

        true
    }

    /// 有効エントリ名の一覧を返す
    pub fn list(&mut self) -> Vec<String> {
        let dir = self.read_dir();
        let mut names = Vec::new();
        for i in 0..DIR_ENTRIES {
            let e = &dir[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
            let flags = u32::from_le_bytes(e[28..32].try_into().unwrap_or([0; 4]));
            if flags == FLAG_VALID && e[0] != 0 {
                names.push(String::from(entry_name_str(e)));
            }
        }
        names
    }

    /// エントリを削除済みとしてマークする
    pub fn delete(&mut self, name: &str) -> bool {
        let mut dir = self.read_dir();
        let mut found = false;
        for i in 0..DIR_ENTRIES {
            let e = &mut dir[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
            let flags = u32::from_le_bytes(e[28..32].try_into().unwrap_or([0; 4]));
            if flags == FLAG_VALID && entry_name_str(e) == name {
                e[28..32].copy_from_slice(&FLAG_DELETED.to_le_bytes());
                found = true;
            }
        }
        if found {
            self.write_dir(&dir);
            self.update_superblock(&dir);
        }
        found
    }

    // ─── 内部ヘルパー ────────────────────────────────────────────────

    fn read_dir(&mut self) -> Vec<u8> {
        let total = DIR_ENTRIES * ENTRY_SIZE; // 2048 bytes = 4 sectors
        let mut buf = alloc::vec![0u8; total];
        for i in 0..DIR_SECTORS {
            let off = (i as usize) * SECTOR_SIZE;
            self.blk.read_sectors(1 + i, &mut buf[off..off + SECTOR_SIZE]);
        }
        buf
    }

    fn write_dir(&mut self, dir: &[u8]) {
        for i in 0..DIR_SECTORS {
            let off = (i as usize) * SECTOR_SIZE;
            self.blk.write_sectors(1 + i, &dir[off..off + SECTOR_SIZE]);
        }
    }

    fn update_superblock(&mut self, dir: &[u8]) {
        let count = (0..DIR_ENTRIES).filter(|&i| {
            let e = &dir[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
            let flags = u32::from_le_bytes(e[28..32].try_into().unwrap_or([0; 4]));
            flags == FLAG_VALID && e[0] != 0
        }).count() as u32;

        let mut sb = [0u8; SECTOR_SIZE];
        self.blk.read_sectors(0, &mut sb);
        sb[8..12].copy_from_slice(&count.to_le_bytes());
        self.blk.write_sectors(0, &sb);
    }

    /// 全有効エントリの末尾から次の空きセクタオフセット（sector 5 起点）を返す
    fn find_next_free_sector(&self, dir: &[u8]) -> u32 {
        let mut max = 0u32;
        for i in 0..DIR_ENTRIES {
            let e = &dir[i * ENTRY_SIZE..(i + 1) * ENTRY_SIZE];
            let flags = u32::from_le_bytes(e[28..32].try_into().unwrap_or([0; 4]));
            if flags == FLAG_VALID {
                let offset = u32::from_le_bytes(e[20..24].try_into().unwrap_or([0; 4]));
                let size   = u32::from_le_bytes(e[24..28].try_into().unwrap_or([0; 4]));
                let end = offset + ((size + SECTOR_SIZE as u32 - 1) / SECTOR_SIZE as u32);
                if end > max { max = end; }
            }
        }
        max
    }
}

/// ディレクトリエントリのバイト列から名前文字列を取り出す
fn entry_name_str(e: &[u8]) -> &str {
    let nul = e[..NAME_MAX].iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
    core::str::from_utf8(&e[..nul]).unwrap_or("")
}
