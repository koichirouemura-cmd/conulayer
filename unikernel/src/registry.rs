// registry.rs — WASMレジストリクライアント
//
// 起動時に HTTP でレジストリサーバーから *.wasm を取得する。
// QEMU user network では 10.0.2.2 がホスト（Alpine VM）に相当する。
// レジストリURL: http://10.0.2.2:8888/<name>.wasm
//
// /var/registry/ に置いたファイル名がそのままルートになる。
//   app.wasm  → ルート "/"
//   bbs.wasm  → ルート "/bbs"

use alloc::vec;
use alloc::vec::Vec;
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};

use crate::serial;
use crate::virtio_net::VirtioNetDev;

const REGISTRY_IP: [u8; 4] = [10, 0, 2, 2];
const REGISTRY_PORT: u16 = 8888;

/// レジストリに登録するエントリ: (HTTPパス, ルートプレフィックス)
/// app.wasm    → ("/app.wasm",    "/")
/// bbs.wasm    → ("/bbs.wasm",    "/bbs")
/// editor.wasm → ("/editor.wasm", "/editor")
const WASM_ENTRIES: &[(&str, &str)] = &[
    ("/app.wasm",    "/"),
    ("/bbs.wasm",    "/bbs"),
    ("/editor.wasm", "/editor"),
];

/// 静的ファイルエントリ: (HTTPパス, ルートパス, Content-Type)
const STATIC_ENTRIES: &[(&str, &str, &str)] = &[
    ("/eq.html",     "/",       "text/html; charset=utf-8"),
    ("/bbs.html",    "/bbs",    "text/html; charset=utf-8"),
    ("/editor.html", "/editor", "text/html; charset=utf-8"),
];

/// VirtioNetDev を受け取り、レジストリから全 WASM と静的ファイルを取得して返す。
/// 返り値: (wasm_routes, static_files, dev)
///   wasm_routes:  Vec<(route_prefix, wasm_bytes)>
///   static_files: Vec<(route_path, content, content_type)>
pub fn fetch_all(mut dev: VirtioNetDev) -> (Vec<(Vec<u8>, Vec<u8>)>, Vec<(Vec<u8>, Vec<u8>, Vec<u8>)>, VirtioNetDev) {
    let mac = EthernetAddress(dev.mac);
    let config = Config::new(mac.into());
    let mut iface = Interface::new(config, &mut dev, Instant::from_millis(0));
    iface.update_ip_addrs(|a| {
        a.push(IpCidr::new(IpAddress::v4(10, 0, 2, 15), 24)).ok();
    });
    iface.routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2))
        .ok();

    let mut sockets = SocketSet::new(vec![]);
    let mut wasm_results: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut static_results: Vec<(Vec<u8>, Vec<u8>, Vec<u8>)> = Vec::new();

    for (http_path, route) in WASM_ENTRIES {
        serial::print("[REG] fetching ");
        serial::print(http_path);
        serial::print("...\n");
        match do_fetch(&mut iface, &mut dev, &mut sockets, http_path) {
            Some(bytes) => {
                serial::print("[REG] ");
                serial::print(http_path);
                serial::print(" fetched: ");
                print_usize(bytes.len());
                serial::print(" bytes\n");
                wasm_results.push((route.as_bytes().to_vec(), bytes));
            }
            None => {
                serial::print("[REG] ");
                serial::print(http_path);
                serial::print(" not found, skip\n");
            }
        }
        // 前の接続の TIME_WAIT が落ち着くまで drain する
        for _ in 0..50000 {
            let ts = Instant::from_millis(crate::timer::now_ms() as i64);
            iface.poll(ts, &mut dev, &mut sockets);
        }
    }

    for (http_path, route_path, content_type) in STATIC_ENTRIES {
        serial::print("[REG] fetching ");
        serial::print(http_path);
        serial::print("...\n");
        match do_fetch(&mut iface, &mut dev, &mut sockets, http_path) {
            Some(bytes) => {
                serial::print("[REG] ");
                serial::print(http_path);
                serial::print(" fetched: ");
                print_usize(bytes.len());
                serial::print(" bytes\n");
                static_results.push((route_path.as_bytes().to_vec(), bytes, content_type.as_bytes().to_vec()));
            }
            None => {
                serial::print("[REG] ");
                serial::print(http_path);
                serial::print(" not found, skip\n");
            }
        }
        for _ in 0..50000 {
            let ts = Instant::from_millis(crate::timer::now_ms() as i64);
            iface.poll(ts, &mut dev, &mut sockets);
        }
    }

    (wasm_results, static_results, dev)
}

static mut NEXT_LOCAL_PORT: u16 = 49152;

fn do_fetch(
    iface: &mut Interface,
    dev: &mut VirtioNetDev,
    sockets: &mut SocketSet,
    path: &str,
) -> Option<Vec<u8>> {
    // TCP ソケット作成
    let rx = tcp::SocketBuffer::new(vec![0u8; 8192]);
    let tx = tcp::SocketBuffer::new(vec![0u8; 4096]);
    let mut sock = tcp::Socket::new(rx, tx);

    // 接続ごとに異なるローカルポートを使う（TIME_WAIT 回避）
    let local_port = unsafe {
        let p = NEXT_LOCAL_PORT;
        NEXT_LOCAL_PORT = if p >= 60000 { 49152 } else { p + 1 };
        p
    };
    let remote = IpEndpoint::new(
        IpAddress::v4(REGISTRY_IP[0], REGISTRY_IP[1], REGISTRY_IP[2], REGISTRY_IP[3]),
        REGISTRY_PORT,
    );
    sock.connect(iface.context(), remote, local_port).ok()?;
    let handle = sockets.add(sock);

    // 接続完了まで待機（最大 ~100000 polls）
    for _ in 0..100000 {
        poll(iface, dev, sockets);
        let s = sockets.get::<tcp::Socket>(handle);
        if s.may_send() { break; }
        if s.state() == tcp::State::Closed {
            serial::print("[REG] TCP closed before connect\n");
            return None;
        }
    }

    // リクエスト送信
    {
        let s = sockets.get_mut::<tcp::Socket>(handle);
        if !s.may_send() {
            serial::print("[REG] connect timeout, state=");
            // state debug
            let state_str = match s.state() {
                tcp::State::Closed => "Closed",
                tcp::State::Listen => "Listen",
                tcp::State::SynSent => "SynSent",
                tcp::State::SynReceived => "SynReceived",
                tcp::State::Established => "Established",
                tcp::State::FinWait1 => "FinWait1",
                tcp::State::FinWait2 => "FinWait2",
                tcp::State::CloseWait => "CloseWait",
                tcp::State::Closing => "Closing",
                tcp::State::LastAck => "LastAck",
                tcp::State::TimeWait => "TimeWait",
            };
            serial::print(state_str);
            serial::print("\n");
            return None;
        }
        let req = alloc::format!(
            "GET {} HTTP/1.0\r\nHost: {}.{}.{}.{}\r\nConnection: close\r\n\r\n",
            path,
            REGISTRY_IP[0], REGISTRY_IP[1], REGISTRY_IP[2], REGISTRY_IP[3]
        );
        s.send_slice(req.as_bytes()).ok()?;
    }

    // レスポンス受信（接続が閉じるまで）
    let mut raw: Vec<u8> = Vec::new();
    for _ in 0..2000000 {
        poll(iface, dev, sockets);
        let s = sockets.get_mut::<tcp::Socket>(handle);
        if s.can_recv() {
            let mut buf = [0u8; 1024];
            let n = s.recv_slice(&mut buf).unwrap_or(0);
            if n > 0 { raw.extend_from_slice(&buf[..n]); }
        }
        if !s.is_open() { break; }
    }

    sockets.remove(handle);

    // HTTP ヘッダーをスキップしてボディを取得
    let sep = b"\r\n\r\n";
    let body_start = raw.windows(4).position(|w| w == sep)? + 4;
    let body = raw[body_start..].to_vec();

    if body.is_empty() { None } else { Some(body) }
}

fn poll(iface: &mut Interface, dev: &mut VirtioNetDev, sockets: &mut SocketSet) {
    let ts = Instant::from_millis(crate::timer::now_ms() as i64);
    iface.poll(ts, dev, sockets);
}

fn print_usize(n: usize) {
    let mut buf = [0u8; 20];
    let mut i = 20;
    let mut n = n;
    if n == 0 { serial::print("0"); return; }
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    if let Ok(s) = core::str::from_utf8(&buf[i..]) {
        serial::print(s);
    }
}
