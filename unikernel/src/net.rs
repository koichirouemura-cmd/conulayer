// net.rs — smoltcp TCP/IP スタック + HTTP サーバー + 管理エンドポイント
//
// port 80  : ユーザー向け HTTP サーバー（WASMに handle_request を呼び出す）
// port 8081: 管理エンドポイント（POST /update で WASM をホットスワップ）

use alloc::vec;
use alloc::vec::Vec;

use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};

use crate::rtc;
use crate::serial;
use crate::store::KvStore;
use crate::virtio_net::VirtioNetDev;
use crate::wasm_rt::WasmApp;

// ─── smoltcp Device ラッパー ─────────────────────────────────────

pub struct RxTokenImpl(Vec<u8>);
pub struct TxTokenImpl(*mut VirtioNetDev);

impl RxToken for RxTokenImpl {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(mut self, f: F) -> R { f(&mut self.0) }
}

impl TxToken for TxTokenImpl {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);
        unsafe { &mut *self.0 }.send_packet(&buf);
        result
    }
}

impl Device for VirtioNetDev {
    type RxToken<'a> = RxTokenImpl where Self: 'a;
    type TxToken<'a> = TxTokenImpl where Self: 'a;

    fn receive(&mut self, _: Instant) -> Option<(RxTokenImpl, TxTokenImpl)> {
        self.try_recv().map(|pkt| (RxTokenImpl(pkt), TxTokenImpl(self as *mut _)))
    }
    fn transmit(&mut self, _: Instant) -> Option<TxTokenImpl> {
        Some(TxTokenImpl(self as *mut _))
    }
    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps
    }
}

// ─── フォールバックレスポンス ─────────────────────────────────────

const HTTP_FALLBACK: &[u8] = b"\
HTTP/1.1 200 OK\r\n\
Content-Type: text/plain\r\n\
Connection: close\r\n\
\r\n\
Hello from unikernel (no WASM)\r\n";

// ─── HTTP リクエストパーサー ──────────────────────────────────────

struct HttpRequest {
    method: Vec<u8>,
    path:   Vec<u8>,
    body:   Vec<u8>,
}

fn parse_http(data: &[u8]) -> HttpRequest {
    let first_line_end = data.iter().position(|&b| b == b'\r').unwrap_or(data.len());
    let first_line = &data[..first_line_end];
    let mut parts = first_line.splitn(3, |&b| b == b' ');
    let method = parts.next().unwrap_or(b"GET").to_vec();
    let path   = parts.next().unwrap_or(b"/").to_vec();

    let body = data.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|pos| data[pos + 4..].to_vec())
        .unwrap_or_default();

    HttpRequest { method, path, body }
}

// ─── 管理ポートのステートマシン ──────────────────────────────────

const ADMIN_PORT: u16 = 8081;
const MAX_WASM_SIZE: usize = 512 * 1024;

enum AdminState {
    Idle,
    ReceivingHeaders(Vec<u8>),
    ReceivingBody { prefix: Vec<u8>, buf: Vec<u8>, total: usize },
}

// ─── バックグラウンドフェッチ ─────────────────────────────────────
//
// smoltcp の iface/dev は単一なので、既存の SocketSet に
// フェッチ用クライアントソケットを追加して同じメインループで管理する。

const FEED_HOST_IP: [u8; 4] = [10, 0, 2, 2];  // Alpine = QEMU ホスト
const FEED_HOST_PORT: u16 = 8889;
const FEED_PATH: &str = "/quake";
const FEED_INTERVAL_SECS: i64 = 60;
const FEED_MAX_SIZE: usize = 256 * 1024;

enum FetchState {
    /// 次回フェッチ時刻まで待機
    Idle { next_ts: i64 },
    /// TCP 接続中
    Connecting { handle: smoltcp::iface::SocketHandle, idle: u32 },
    /// HTTP リクエスト送信済み、レスポンス受信中
    Receiving { handle: smoltcp::iface::SocketHandle, buf: Vec<u8>, stale: u32 },
}

fn find_header_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &[u8]) -> Option<usize> {
    let text = core::str::from_utf8(headers).ok()?;
    for line in text.split("\r\n") {
        if line.len() > 16 && line[..16].eq_ignore_ascii_case("Content-Length: ") {
            return line[16..].trim().parse::<usize>().ok();
        }
    }
    None
}

// ─── HTTP サーバーメインループ ────────────────────────────────────

// ─── WASMルーター ────────────────────────────────────────────────

/// パスプレフィックスで引くルーター。最長一致で選択する。
struct WasmRouter {
    entries: Vec<(Vec<u8>, WasmApp)>, // (prefix, app)
}

impl WasmRouter {
    fn new() -> Self { Self { entries: Vec::new() } }

    /// アプリを登録する。同じprefixがあれば上書き。
    fn insert(&mut self, prefix: Vec<u8>, app: WasmApp) {
        for entry in &mut self.entries {
            if entry.0 == prefix { entry.1 = app; return; }
        }
        self.entries.push((prefix, app));
    }

    /// パスに最長一致するアプリとプレフィックス長を返す。
    /// プレフィックスを除いたパスをWASMに渡すためにprefixも返す。
    fn route_mut(&mut self, path: &[u8]) -> Option<(usize, &mut WasmApp)> {
        let mut best_len = 0usize;
        let mut best_idx: Option<usize> = None;
        for (i, (prefix, _)) in self.entries.iter().enumerate() {
            if path.starts_with(prefix) && prefix.len() >= best_len {
                best_len = prefix.len();
                best_idx = Some(i);
            }
        }
        best_idx.map(|i| {
            let plen = self.entries[i].0.len();
            (plen, &mut self.entries[i].1)
        })
    }
}

// ─── 管理エンドポイントのパス解析 ──────────────────────────────

/// "/update/bbs" → b"/bbs", "/update" または "/update/" → b"/"
fn parse_route_prefix(path: &[u8]) -> Vec<u8> {
    let prefix_key = b"/update";
    if path.starts_with(prefix_key) {
        let rest = &path[prefix_key.len()..];
        if rest.is_empty() || rest == b"/" {
            return b"/".to_vec();
        }
        return rest.to_vec();
    }
    b"/".to_vec()
}

pub fn run_http_server(mut dev: VirtioNetDev, wasm_routes: alloc::vec::Vec<(alloc::vec::Vec<u8>, WasmApp)>, static_files: alloc::vec::Vec<(alloc::vec::Vec<u8>, alloc::vec::Vec<u8>, alloc::vec::Vec<u8>)>, mut store: Option<KvStore>) -> ! {
    let mut router = WasmRouter::new();
    for (route, app) in wasm_routes { router.insert(route, app); }
    let mac = EthernetAddress(dev.mac);

    let config = Config::new(mac.into());
    let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));
    iface.update_ip_addrs(|a| { a.push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24)).ok(); });
    iface.routes_mut().add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2)).ok();

    let h80 = {
        let s = tcp::Socket::new(
            tcp::SocketBuffer::new(vec![0u8; 4096]),
            tcp::SocketBuffer::new(vec![0u8; 4096]),
        );
        let mut ss = SocketSet::new(vec![]);
        let h = ss.add(s);
        drop(ss);
        h
    };
    // SocketSet を再作成して両方追加
    let mut sockets = SocketSet::new(vec![]);
    let tcp80 = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0u8; 4096]),
        tcp::SocketBuffer::new(vec![0u8; 262144]),  // 256KB TX for large JSON (JMA ~173KB)
    );
    let tcp81 = tcp::Socket::new(
        tcp::SocketBuffer::new(vec![0u8; 65536]),
        tcp::SocketBuffer::new(vec![0u8; 4096]),
    );
    let _ = h80; // 使わない
    let h80 = sockets.add(tcp80);
    let h81 = sockets.add(tcp81);

    serial::print("[HTTP] listening on 10.0.2.15:80\n");
    serial::print("[ADMIN] listening on 10.0.2.15:8081\n");
    serial::print("[HTTP READY]\n");

    let mut sent80 = false;
    let mut req_buf: Vec<u8> = Vec::new();
    let mut admin_state = AdminState::Idle;
    let mut pending_entry: Option<(Vec<u8>, Vec<u8>)> = None; // (prefix, wasm_bytes)
    let mut fetch_state = FetchState::Idle { next_ts: 0 }; // 起動直後に即フェッチ
    let mut pending_feed: Option<Vec<u8>> = None;
    let mut last_feed: Vec<u8> = Vec::new(); // 最後に取得したフィードを保持

    loop {
        // フェッチ完了データを全WASMに配布（スワップより先に実行してlast_feedを更新）
        if let Some(feed) = pending_feed.take() {
            last_feed = feed.clone();
            for (_, app) in &mut router.entries {
                app.update_feed(feed.clone());
            }
        }

        // pending WASM をループ先頭で適用（last_feed更新後に実行）
        if let Some((prefix, bytes)) = pending_entry.take() {
            match WasmApp::from_bytes(&bytes) {
                Some(mut app) => {
                    // ホットスワップ直後に最新フィードを渡す（60秒待ちを防ぐ）
                    if !last_feed.is_empty() { app.update_feed(last_feed.clone()); }
                    router.insert(prefix, app);
                    serial::print("[SWAP] OK\n");
                }
                None => { serial::print("[SWAP] parse failed\n"); }
            }
        }

        iface.poll(Instant::from_millis(crate::timer::now_ms() as i64), &mut dev, &mut sockets);

        // ─── バックグラウンドフェッチ ─────────────────────────────
        fetch_state = match fetch_state {
            FetchState::Idle { next_ts } => {
                let now = rtc::read_unix_timestamp();
                if now >= next_ts {
                    serial::print("[FEED] connecting...\n");
                    let rx = tcp::SocketBuffer::new(vec![0u8; 65536]);
                    let tx = tcp::SocketBuffer::new(vec![0u8; 1024]);
                    let mut sock = tcp::Socket::new(rx, tx);
                    let remote = IpEndpoint::new(
                        IpAddress::v4(FEED_HOST_IP[0], FEED_HOST_IP[1], FEED_HOST_IP[2], FEED_HOST_IP[3]),
                        FEED_HOST_PORT,
                    );
                    if sock.connect(iface.context(), remote, 49200).is_ok() {
                        let h = sockets.add(sock);
                        FetchState::Connecting { handle: h, idle: 0 }
                    } else {
                        FetchState::Idle { next_ts: now + FEED_INTERVAL_SECS }
                    }
                } else {
                    FetchState::Idle { next_ts }
                }
            }
            FetchState::Connecting { handle: h, idle } => {
                let s = sockets.get::<tcp::Socket>(h);
                if s.may_send() {
                    serial::print("[FEED] connected, sending request\n");
                    drop(s);
                    let req = alloc::format!(
                        "GET {} HTTP/1.0\r\nHost: {}.{}.{}.{}\r\nConnection: close\r\n\r\n",
                        FEED_PATH,
                        FEED_HOST_IP[0], FEED_HOST_IP[1], FEED_HOST_IP[2], FEED_HOST_IP[3]
                    );
                    sockets.get_mut::<tcp::Socket>(h).send_slice(req.as_bytes()).ok();
                    FetchState::Receiving { handle: h, buf: Vec::new(), stale: 0 }
                } else if !s.is_open() || idle > 200000 {
                    drop(s);
                    sockets.remove(h);
                    let now = rtc::read_unix_timestamp();
                    FetchState::Idle { next_ts: now + FEED_INTERVAL_SECS }
                } else {
                    FetchState::Connecting { handle: h, idle: idle + 1 }
                }
            }
            FetchState::Receiving { handle: h, mut buf, mut stale } => {
                let prev_len = buf.len();
                {
                    let s = sockets.get_mut::<tcp::Socket>(h);
                    // 利用可能なデータを全部ドレインする
                    while s.can_recv() && buf.len() < FEED_MAX_SIZE {
                        let mut tmp = [0u8; 4096];
                        let n = s.recv_slice(&mut tmp).unwrap_or(0);
                        if n == 0 { break; }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                }
                if buf.len() > prev_len { stale = 0; } else { stale += 1; }
                if !buf.is_empty() && buf.len() % 4096 < 32 {
                    serial::print("[FEED] receiving...\n");
                }
                // FIN受信 OR データが止まった（50000ループ） OR バッファ上限
                let done = !sockets.get::<tcp::Socket>(h).may_recv()
                    || stale > 500_000
                    || buf.len() >= FEED_MAX_SIZE;
                if done {
                    sockets.get_mut::<tcp::Socket>(h).close();
                    sockets.remove(h);
                    serial::print("[FEED] done, parsing\n");
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let body = buf[pos + 4..].to_vec();
                        if !body.is_empty() {
                            serial::print("[FEED] updated\n");
                            pending_feed = Some(body);
                        }
                    }
                    let now = rtc::read_unix_timestamp();
                    FetchState::Idle { next_ts: now + FEED_INTERVAL_SECS }
                } else {
                    FetchState::Receiving { handle: h, buf, stale }
                }
            }
        };

        // ─── port 80 ─────────────────────────────────────────────
        {
            let s = sockets.get_mut::<tcp::Socket>(h80);
            if !s.is_open() {
                s.listen(80).ok();
                sent80 = false;
                req_buf.clear();
            }
            if s.may_recv() && !sent80 {
                let mut tmp = [0u8; 1024];
                let n = s.recv_slice(&mut tmp).unwrap_or(0);
                if n > 0 { req_buf.extend_from_slice(&tmp[..n]); }

                // ヘッダ受信完了（\r\n\r\n を検出）したらWASMを呼ぶ
                if req_buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let req = parse_http(&req_buf);
                    // 静的ファイルを完全一致で優先チェック
                    let resp = if let Some((_, content, ctype)) = static_files.iter()
                        .find(|(path, _, _)| path.as_slice() == req.path.as_slice())
                    {
                        serial::print("[STATIC] serving static file\n");
                        let header = alloc::format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            core::str::from_utf8(ctype).unwrap_or("text/html"),
                            content.len()
                        );
                        let mut r = header.into_bytes();
                        r.extend_from_slice(content);
                        r
                    } else {
                        match router.route_mut(&req.path) {
                            Some((plen, app)) => {
                                // プレフィックスを除いたパスをWASMに渡す
                                // /bbs/foo → /foo、/bbs → /
                                let stripped = &req.path[plen..];
                                let wasm_path = if stripped.is_empty() { b"/".as_slice() } else { stripped };
                                app.handle_request(&req.method, wasm_path, &req.body)
                                   .unwrap_or_else(|| HTTP_FALLBACK.to_vec())
                            }
                            None => HTTP_FALLBACK.to_vec(),
                        }
                    };
                    s.send_slice(&resp).ok();
                    s.close();
                    sent80 = true;
                }
            }
        }

        // ─── port 8081 ───────────────────────────────────────────
        {
            let s = sockets.get_mut::<tcp::Socket>(h81);
            if !s.is_open() { s.listen(ADMIN_PORT).ok(); admin_state = AdminState::Idle; }

            match &mut admin_state {
                AdminState::Idle => {
                    if s.may_recv() { admin_state = AdminState::ReceivingHeaders(Vec::new()); }
                }
                AdminState::ReceivingHeaders(hdr_buf) => {
                    if s.can_recv() {
                        let mut tmp = [0u8; 1024];
                        let n = s.recv_slice(&mut tmp).unwrap_or(0);
                        hdr_buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = find_header_end(hdr_buf) {
                            let content_len = parse_content_length(&hdr_buf[..pos]);
                            let already = hdr_buf[pos + 4..].to_vec();
                            // リクエストパスからルートプレフィックスを取得
                            let req_path = parse_http(&hdr_buf[..pos]).path;
                            let prefix = parse_route_prefix(&req_path);
                            match content_len {
                                Some(total) => { admin_state = AdminState::ReceivingBody { prefix, buf: already, total }; }
                                None => { s.send_slice(b"HTTP/1.0 400 Bad Request\r\n\r\n").ok(); s.close(); admin_state = AdminState::Idle; }
                            }
                        }
                    }
                    if !s.is_open() { admin_state = AdminState::Idle; }
                }
                AdminState::ReceivingBody { prefix, buf, total } => {
                    if s.can_recv() {
                        let mut tmp = [0u8; 4096];
                        let n = s.recv_slice(&mut tmp).unwrap_or(0);
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let total = *total;
                    if buf.len() >= total {
                        if total > MAX_WASM_SIZE {
                            s.send_slice(b"HTTP/1.0 413 Payload Too Large\r\n\r\n").ok();
                        } else {
                            let wasm_data = buf[..total].to_vec();
                            // ディスクに永続化
                            let store_key = if prefix == b"/" {
                                alloc::string::String::from("app.wasm")
                            } else {
                                let n = core::str::from_utf8(&prefix[1..]).unwrap_or("app");
                                alloc::format!("{}.wasm", n)
                            };
                            if let Some(ref mut st) = store {
                                if st.write(&store_key, &wasm_data) {
                                    serial::print("[STORE] WASM persisted\n");
                                }
                            }
                            pending_entry = Some((prefix.clone(), wasm_data));
                            s.send_slice(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\nOK").ok();
                        }
                        s.close();
                        admin_state = AdminState::Idle;
                    }
                    if !s.is_open() { admin_state = AdminState::Idle; }
                }
            }
        }

        unsafe { core::arch::asm!("hlt", options(nostack)) };
    }
}
