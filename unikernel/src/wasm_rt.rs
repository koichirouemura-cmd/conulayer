// wasm_rt.rs — Layer B: wasmi WASM ランタイム
//
// WASMモジュールをロードし、リクエストごとに handle_request() を呼び出す。
//
// メモリレイアウト（2ページ = 128KB）:
//   Page 0 (0    - 65535): レスポンス領域（WASMが書き込む）
//   Page 1 (65536- ...  ): リクエスト領域（カーネルが書き込む）
//     65536: method  (最大256バイト)
//     65792: path    (最大1024バイト)
//     66816: body    (最大32KBバイト)
//
// WASM export API:
//   handle_request(method_ptr, method_len, path_ptr, path_len, body_ptr, body_len) -> i32
//   get_response_ptr() -> i32

use alloc::vec::Vec;
use wasmi::{Config, Engine, ExternType, Instance, Linker, Memory, Module, Store, TypedFunc};
use crate::serial;
use crate::rtc;

// ─── ホストステート ──────────────────────────────────────────────

/// WASM ホスト関数から参照できるカーネル側の状態
pub struct HostState {
    /// host.get_feed() で返す最新フィードデータ
    pub feed_cache: Vec<u8>,
}

impl HostState {
    fn new() -> Self { Self { feed_cache: Vec::new() } }
}

static FALLBACK_WASM: &[u8] = include_bytes!("../wasm/app.wasm");

const MAX_WASM_BYTES: usize = 1024 * 1024;         // 1MB
const MAX_WASM_MEMORY_PAGES: u64 = 16;             // 16 pages = 1MB
const FUEL_PER_REQUEST: u64 = 100_000_000;         // 100M instructions

// リクエスト領域のオフセット（Page 1）
const METHOD_OFF: usize = 65536;
const METHOD_MAX: usize = 256;
const PATH_OFF:   usize = METHOD_OFF + METHOD_MAX; // 65792
const PATH_MAX:   usize = 1024;
const BODY_OFF:   usize = PATH_OFF + PATH_MAX;     // 66816
const BODY_MAX:   usize = 32768;

pub struct WasmApp {
    store:               Store<HostState>,
    handle_request_fn:   TypedFunc<(i32, i32, i32, i32, i32, i32), i32>,
    get_response_ptr_fn: TypedFunc<(), i32>,
    memory:              Memory,
}

impl WasmApp {
    /// フィードキャッシュを更新する（net.rs のポーリングループから呼ぶ）
    pub fn update_feed(&mut self, data: Vec<u8>) {
        self.store.data_mut().feed_cache = data;
    }
}

impl WasmApp {
    pub fn init() -> Option<Self> {
        Self::from_bytes(FALLBACK_WASM)
    }

    pub fn from_bytes(wasm_bytes: &[u8]) -> Option<Self> {
        if wasm_bytes.len() > MAX_WASM_BYTES {
            serial::print("[WASM] too large, rejected\n");
            return None;
        }
        serial::print("[WASM] loading module...\n");
        Self::load(wasm_bytes)
    }

    fn load(wasm_bytes: &[u8]) -> Option<Self> {
        let mut config = Config::default();
        config.consume_fuel(true);
        let engine = Engine::new(&config);
        let module = match Module::new(&engine, wasm_bytes) {
            Ok(m) => m,
            Err(_) => { serial::print("[WASM] module parse failed\n"); return None; }
        };

        // メモリページ上限チェック
        let mem_ok = module.exports()
            .filter_map(|e| if let ExternType::Memory(mt) = e.ty() { Some(mt.minimum()) } else { None })
            .all(|pages| pages as u64 <= MAX_WASM_MEMORY_PAGES);
        if !mem_ok {
            serial::print("[WASM] memory limit exceeded, rejected\n");
            return None;
        }

        // ─── ホスト関数を登録 ────────────────────────────────────────
        let mut linker: Linker<HostState> = Linker::new(&engine);

        // host.log(ptr, len) — デバッグ用シリアル出力
        linker.func_wrap("host", "log", |caller: wasmi::Caller<'_, HostState>, ptr: i32, len: i32| {
            let mem = caller.get_export("memory")
                .and_then(|e| e.into_memory());
            if let Some(mem) = mem {
                let data = mem.data(&caller);
                let start = ptr as usize;
                let end = (start + len as usize).min(data.len());
                if let Ok(s) = core::str::from_utf8(&data[start..end]) {
                    crate::serial::print("[WASM LOG] ");
                    crate::serial::print(s);
                    crate::serial::print("\n");
                }
            }
        }).ok()?;

        // host.now() -> i64 — Unix タイムスタンプ（秒）
        linker.func_wrap("host", "now", |_: wasmi::Caller<'_, HostState>| -> i64 {
            rtc::read_unix_timestamp()
        }).ok()?;

        // host.random() -> i64 — ハードウェア乱数（RDRAND）
        linker.func_wrap("host", "random", |_: wasmi::Caller<'_, HostState>| -> i64 {
            let mut val: u64 = 0xdeadbeef_cafebabe;
            unsafe {
                core::arch::asm!(
                    "2: rdrand {0}; jnc 2b",
                    out(reg) val,
                    options(nostack)
                );
            }
            val as i64
        }).ok()?;

        // host.get_feed(out_ptr, out_max) -> i32
        // カーネルが保持する最新フィードデータを WASM メモリにコピーする
        // 戻り値: コピーしたバイト数（0=データなし）
        linker.func_wrap("host", "get_feed", |mut caller: wasmi::Caller<'_, HostState>, out_ptr: i32, out_max: i32| -> i32 {
            let feed = caller.data().feed_cache.clone();
            if feed.is_empty() { return 0; }
            let copy_len = feed.len().min(out_max as usize);
            let mem = caller.get_export("memory").and_then(|e| e.into_memory());
            if let Some(mem) = mem {
                let data = mem.data_mut(&mut caller);
                let start = out_ptr as usize;
                let end = start + copy_len;
                if end <= data.len() {
                    data[start..end].copy_from_slice(&feed[..copy_len]);
                    return copy_len as i32;
                }
            }
            -1
        }).ok()?;

        // host.file_read(path_ptr, path_len, out_ptr, out_max) -> i32
        linker.func_wrap("host", "file_read", |mut caller: wasmi::Caller<'_, HostState>, path_ptr: i32, path_len: i32, out_ptr: i32, out_max: i32| -> i32 {
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let path = {
                let data = mem.data(&caller);
                let start = path_ptr as usize;
                let end = (start + path_len as usize).min(data.len());
                match core::str::from_utf8(&data[start..end]) {
                    Ok(s) => alloc::string::String::from(s),
                    Err(_) => return -1,
                }
            };
            let result = match crate::vsock::file_read(&path) {
                Some(v) => v,
                None => return -1,
            };
            let copy_len = result.len().min(out_max as usize);
            let data = mem.data_mut(&mut caller);
            let start = out_ptr as usize;
            if start + copy_len <= data.len() {
                data[start..start + copy_len].copy_from_slice(&result[..copy_len]);
                copy_len as i32
            } else {
                -1
            }
        }).ok()?;

        // host.file_write(path_ptr, path_len, data_ptr, data_len) -> i32
        linker.func_wrap("host", "file_write", |caller: wasmi::Caller<'_, HostState>, path_ptr: i32, path_len: i32, data_ptr: i32, data_len: i32| -> i32 {
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let (path, write_data) = {
                let raw = mem.data(&caller);
                let ps = path_ptr as usize;
                let pe = (ps + path_len as usize).min(raw.len());
                let path = match core::str::from_utf8(&raw[ps..pe]) {
                    Ok(s) => alloc::string::String::from(s),
                    Err(_) => return -1,
                };
                let ds = data_ptr as usize;
                let de = (ds + data_len as usize).min(raw.len());
                let write_data = raw[ds..de].to_vec();
                (path, write_data)
            };
            if crate::vsock::file_write(&path, &write_data) { 0 } else { -1 }
        }).ok()?;

        // host.file_list(path_ptr, path_len, out_ptr, out_max) -> i32
        linker.func_wrap("host", "file_list", |mut caller: wasmi::Caller<'_, HostState>, path_ptr: i32, path_len: i32, out_ptr: i32, out_max: i32| -> i32 {
            let mem = match caller.get_export("memory").and_then(|e| e.into_memory()) {
                Some(m) => m,
                None => return -1,
            };
            let path = {
                let data = mem.data(&caller);
                let start = path_ptr as usize;
                let end = (start + path_len as usize).min(data.len());
                match core::str::from_utf8(&data[start..end]) {
                    Ok(s) => alloc::string::String::from(s),
                    Err(_) => return -1,
                }
            };
            let result = match crate::vsock::file_list(&path) {
                Some(v) => v,
                None => return -1,
            };
            let copy_len = result.len().min(out_max as usize);
            let data = mem.data_mut(&mut caller);
            let start = out_ptr as usize;
            if start + copy_len <= data.len() {
                data[start..start + copy_len].copy_from_slice(&result[..copy_len]);
                copy_len as i32
            } else {
                -1
            }
        }).ok()?;

        let mut store: Store<HostState> = Store::new(&engine, HostState::new());

        let instance: Instance = match linker.instantiate_and_start(&mut store, &module) {
            Ok(i) => i,
            Err(_) => { serial::print("[WASM] instantiate failed\n"); return None; }
        };

        let handle_request_fn: TypedFunc<(i32, i32, i32, i32, i32, i32), i32> =
            match instance.get_typed_func(&store, "handle_request") {
                Ok(f) => f,
                Err(_) => { serial::print("[WASM] handle_request not found\n"); return None; }
            };

        let get_response_ptr_fn: TypedFunc<(), i32> =
            match instance.get_typed_func(&store, "get_response_ptr") {
                Ok(f) => f,
                Err(_) => { serial::print("[WASM] get_response_ptr not found\n"); return None; }
            };

        let memory: Memory = match instance.get_memory(&store, "memory") {
            Some(m) => m,
            None => { serial::print("[WASM] memory not found\n"); return None; }
        };

        serial::print("[WASM] Layer B ready\n");
        Some(WasmApp { store, handle_request_fn, get_response_ptr_fn, memory })
    }

    /// HTTPリクエストを受け取ってWASMに渡し、レスポンスバイト列を返す。
    pub fn handle_request(&mut self, method: &[u8], path: &[u8], body: &[u8]) -> Option<Vec<u8>> {
        // リクエストデータをWASMリニアメモリに書き込む
        {
            let mem = self.memory.data_mut(&mut self.store);
            let ml = method.len().min(METHOD_MAX);
            mem[METHOD_OFF..METHOD_OFF + ml].copy_from_slice(&method[..ml]);
            let pl = path.len().min(PATH_MAX);
            mem[PATH_OFF..PATH_OFF + pl].copy_from_slice(&path[..pl]);
            let bl = body.len().min(BODY_MAX);
            mem[BODY_OFF..BODY_OFF + bl].copy_from_slice(&body[..bl]);
        }

        // リクエストごとに燃料をリセット
        self.store.set_fuel(FUEL_PER_REQUEST).ok()?;

        // handle_request() を呼び出す
        let resp_len = self.handle_request_fn.call(
            &mut self.store,
            (
                METHOD_OFF as i32, method.len().min(METHOD_MAX) as i32,
                PATH_OFF   as i32, path.len().min(PATH_MAX)     as i32,
                BODY_OFF   as i32, body.len().min(BODY_MAX)     as i32,
            ),
        ).ok()?;

        if resp_len <= 0 { return None; }

        // レスポンスポインタを取得してコピー
        let resp_ptr = self.get_response_ptr_fn.call(&mut self.store, ()).ok()? as usize;
        let mem = self.memory.data(&self.store);
        let end = resp_ptr + resp_len as usize;
        if end > mem.len() { return None; }
        Some(mem[resp_ptr..end].to_vec())
    }
}
