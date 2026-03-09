#!/usr/bin/env python3
"""Generate WAT source for earthquake monitor on unikernel WASM.

Routes: GET /
Memory layout:
  Page 0 (0-65535):        response area (WASM writes)
  Page 1 (65536-131071):   request area  (kernel writes)
  Page 2 (131072-...):     app data
    131072: last_fetch_ts (i64, 8 bytes)
    131080: feed_len      (i32, 4 bytes)
    131084: feed_data     (max 32KB)
    200000+: static blobs
  Page 3 (196608-262143):  scratch area (format_time buffer, etc.)
  Page 4 (262144-...):     parsed entry area (up to 20 entries x 84 bytes)

Usage:
    python3 wasm/eq_gen.py > /tmp/eq.wat
    wat2wasm /tmp/eq.wat -o /tmp/eq.wasm
"""

def wat_esc(data: bytes) -> str:
    r = []
    for b in data:
        if   b == 0x22: r.append('\\"')
        elif b == 0x5c: r.append('\\\\')
        elif b == 0x0d: r.append('\\r')
        elif b == 0x0a: r.append('\\n')
        elif b == 0x09: r.append('\\t')
        elif 0x20 <= b <= 0x7e: r.append(chr(b))
        else: r.append(f'\\{b:02x}')
    return '"' + ''.join(r) + '"'

# ── Memory layout constants ─────────────────────────────────────────
LAST_FETCH_TS = 131072   # i64: Unix timestamp of last get_feed call
FEED_LEN      = 131080   # i32: cached JSON byte length
FEED_DATA     = 131084   # up to 32KB JSON cache
MAX_FEED_LEN  = 32768
CACHE_TTL     = 60       # seconds

# Entry area for parsed JSON results
ENTRY_SLOT   = 84
ENTRY_AREA   = 4 * 65536  # Page 4 = 262144
ENTRY_AT     = 0   # at bytes [0..24]
ENTRY_AT_LEN = 25
ENTRY_MAGI   = 26  # magnitude integer digit (0-9)
ENTRY_MAGF   = 27  # magnitude fractional digit (0-9)
ENTRY_ANM    = 28  # anm bytes [28..57] (30 bytes max = ~5 unicode-escaped chars)
ENTRY_ANMLEN = 58
ENTRY_LAT    = 59  # lat_tenths (i32, signed, e.g. 253 for 25.3°N)
ENTRY_LON    = 63  # lon_tenths (i32, signed, e.g. 1250 for 125.0°E)
ENTRY_MAXI   = 67  # maxi: single ASCII char ('0'..'7')
                   # [68..83] unused
MAX_ENTRIES  = 20

# Scratch buffer (format_time output, mag string)
TIME_BUF     = 196608  # Page 3 start

# ── Static blobs ────────────────────────────────────────────────────
HTML_HDR  = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n"
NOT_FOUND = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nNot Found"
ERR_500   = b"HTTP/1.1 500 Internal Server Error\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n"
LOG_INIT  = b"eq: handle_request"
LOG_FETCH = b"eq: fetching feed"
LOG_CACHE = b"eq: using cache"
LOG_EMPTY = b"eq: no data"
LOG_ERR   = b"eq: get_feed error"
PATH_ROOT = b"/"

HTML_PRE = (
    b'<!DOCTYPE html><html><head><meta charset="UTF-8">'
    b'<meta http-equiv="refresh" content="60">'
    b'<title>Earthquake Monitor</title>'
    b'<style>'
    b'*{margin:0;padding:0;box-sizing:border-box}'
    b'body{background:#0d1117;color:#e6edf3;font-family:monospace;padding:20px}'
    b'h1{color:#58a6ff;margin-bottom:8px;font-size:18px}'
    b'small{color:#8b949e;font-weight:normal;margin-left:8px;font-size:12px}'
    b'.map-wrap{display:flex;justify-content:center;margin-bottom:16px}'
    b'svg{border:1px solid #30363d;border-radius:6px;background:#0a0f1a}'
    b'.eq-list{max-width:800px;margin:0 auto}'
    b'table{width:100%;border-collapse:collapse;font-size:13px}'
    b'th{background:#161b22;color:#8b949e;padding:8px 12px;text-align:left;'
    b'border-bottom:1px solid #30363d;font-weight:normal}'
    b'td{padding:7px 12px;border-bottom:1px solid #21262d}'
    b'tr:hover td{background:#161b22}'
    b'.m0{color:#adbac7}.m1{color:#adbac7}.m2{color:#e3b341}.m3{color:#f0883e}'
    b'.m4{color:#f85149}.m5{color:#ff7b72}'
    b'tr:has(td.m3) td:first-child{border-left:3px solid #f0883e}'
    b'tr:has(td.m4) td:first-child{border-left:3px solid #f85149}'
    b'tr:has(td.m5) td:first-child{border-left:3px solid #ff7b72}'
    b'tr:has(td.m3){background:rgba(240,136,62,0.04)}'
    b'tr:has(td.m4){background:rgba(248,81,73,0.08)}'
    b'tr:has(td.m5){background:rgba(255,123,114,0.12)}'
    b'.time{font-size:15px;font-weight:bold;letter-spacing:0.05em}'
    b'.loc{font-size:14px}'
    b'.status{color:#8b949e;font-size:12px;margin-top:12px;text-align:center}'
    b'</style></head><body>'
    b'<div class="eq-list">'
    b'<h1>Earthquake Monitor <small>AI-native unikernel</small></h1>'
)

SVG_MAP = (
    b'<div class="map-wrap"><svg width="500" height="600" viewBox="0 0 500 600">'
    b'<text x="10" y="20" fill="#58a6ff" font-size="11" font-family="monospace">Japan Seismic Map</text>'
    # Honshu — clockwise from Tsugaru strait: E coast (Pacific) → W coast (Sea of Japan)
    b'<polyline points="'
    b'303,143 308,152 315,157 319,175 319,181 315,198 310,200 310,222 307,233 '
    b'308,248 299,256 292,258 289,269 285,261 279,266 272,268 264,278 261,289 '
    b'257,285 255,275 258,266 252,268 243,266 230,272 215,279 '
    b'220,271 225,266 233,253 247,251 258,251 264,248 264,240 '
    b'272,215 292,207 301,174 300,164 303,143'
    b'" fill="none" stroke="#30363d" stroke-width="1.5"/>'
    # Hokkaido — clockwise from Matsumae (SW)
    b'<polyline points="'
    b'301,141 307,133 309,124 316,117 333,131 342,111 355,104 357,103 '
    b'340,93 333,86 319,65 314,72 316,95 313,108 309,120 301,141'
    b'" fill="none" stroke="#30363d" stroke-width="1.5"/>'
    # Kyushu
    b'<polyline points="'
    b'217,283 213,287 208,291 205,291 208,306 213,302 214,324 216,335 '
    b'222,327 223,318 225,305 223,292 217,283'
    b'" fill="none" stroke="#30363d" stroke-width="1.5"/>'
    # Shikoku
    b'<polyline points="'
    b'247,277 243,294 231,305 227,296 229,284 232,278 242,274 247,277'
    b'" fill="none" stroke="#30363d" stroke-width="1"/>'
    b'<text x="320" y="84" fill="#484f58" font-size="9" font-family="monospace">Hokkaido</text>'
    b'<text x="268" y="246" fill="#484f58" font-size="9" font-family="monospace">Honshu</text>'
    b'<text x="198" y="315" fill="#484f58" font-size="9" font-family="monospace">Kyushu</text>'
    b'<text x="232" y="292" fill="#484f58" font-size="8" font-family="monospace">Shikoku</text>'
    b'<!-- legend -->'
    b'<circle cx="20" cy="545" r="4" fill="#ffa500"/>'
    b'<text x="28" y="549" fill="#8b949e" font-size="9" font-family="monospace">M&lt;3</text>'
    b'<circle cx="60" cy="545" r="6" fill="#ff6600"/>'
    b'<text x="70" y="549" fill="#8b949e" font-size="9" font-family="monospace">M3-4</text>'
    b'<circle cx="105" cy="545" r="8" fill="#ff3300"/>'
    b'<text x="117" y="549" fill="#8b949e" font-size="9" font-family="monospace">M4-5</text>'
    b'<circle cx="155" cy="545" r="12" fill="#cc0000"/>'
    b'<text x="171" y="549" fill="#8b949e" font-size="9" font-family="monospace">M5+</text>'
    # SVG_MAP does NOT include closing </svg> — circles are injected, then HTML_MAP_CLOSE closes it
)

HTML_MAP_CLOSE = b'</svg></div>'

HTML_TABLE_HDR = (
    b'<table>'
    b'<thead><tr>'
    b'<th style="width:80px">Time (JST)</th>'
    b'<th>Location</th>'
    b'<th style="width:80px">Mag</th>'
    b'<th style="width:60px">Int</th>'
    b'</tr></thead><tbody>'
)

HTML_LOADING = (
    b'<p style="color:#8b949e;padding:20px;text-align:center">'
    b'Loading earthquake data...</p>'
)

HTML_NO_DATA = (
    b'<p style="color:#8b949e;padding:20px;text-align:center">'
    b'No recent earthquake data.</p>'
)

HTML_TABLE_FTR = b'</tbody></table>'

HTML_POST = (
    b'</div>'
    b'<p class="status">Auto-refresh every 60s | Data: JMA via unikernel fw_cfg</p>'
    b'</body></html>'
)

HTML_ERR = (
    b'<!DOCTYPE html><html><head><meta charset="UTF-8">'
    b'<title>Error</title>'
    b'<style>body{background:#0d1117;color:#f85149;font-family:monospace;padding:20px}</style>'
    b'</head><body>'
    b'<h1>Earthquake Monitor - Error</h1>'
    b'<p>Failed to retrieve earthquake data. Retrying...</p>'
    b'<meta http-equiv="refresh" content="10">'
    b'</body></html>'
)

# ── ALL static blobs in one list (order determines memory layout) ────
# Extra small blobs needed by the WAT must also be here so offsets are
# available when the WAT f-string is evaluated.
statics = [
    # Main HTML/HTTP blobs
    ('HTML_HDR',      HTML_HDR),
    ('NOT_FOUND',     NOT_FOUND),
    ('ERR_500',       ERR_500),
    ('LOG_INIT',      LOG_INIT),
    ('LOG_FETCH',     LOG_FETCH),
    ('LOG_CACHE',     LOG_CACHE),
    ('LOG_EMPTY',     LOG_EMPTY),
    ('LOG_ERR',       LOG_ERR),
    ('PATH_ROOT',     PATH_ROOT),
    ('HTML_PRE',      HTML_PRE),
    ('SVG_MAP',       SVG_MAP),
    ('HTML_MAP_CLOSE',HTML_MAP_CLOSE),
    ('HTML_TABLE_HDR',HTML_TABLE_HDR),
    ('HTML_LOADING',  HTML_LOADING),
    ('HTML_NO_DATA',  HTML_NO_DATA),
    ('HTML_TABLE_FTR',HTML_TABLE_FTR),
    ('HTML_POST',     HTML_POST),
    ('HTML_ERR',      HTML_ERR),
    # JSON field key strings (used in find_field calls)
    ('KEY_AT',        b'at'),
    ('KEY_MAG',       b'mag'),
    ('KEY_ANM',       b'anm'),
    ('KEY_COD',       b'cod'),
    ('KEY_MAXI',      b'maxi'),
    # SVG circle fragments
    ('SVG_CIR_PRE',   b'<circle cx="'),
    ('SVG_CY',        b'" cy="'),
    ('SVG_R',         b'" r="'),
    ('SVG_FILL',      b'" fill="#'),
    ('SVG_CIR_SFX',   b'" opacity="0.85"/>'),
    # Color table: 7 bytes per entry (6 hex chars + NUL pad), 4 entries
    # Indices 0-3 correspond to M<3, M3-4, M4-5, M>=5
    ('COLOR_TABLE',   b'ffa500\x00ff6600\x00ff3300\x00cc0000\x00'),
    # HTML table row/cell fragments
    ('TR_OPEN',       b'<tr>'),
    ('TR_CLOSE',      b'</tr>'),
    ('TD_PRE',        b'<td class="m'),
    ('TD_MID',        b'">'),
    ('TD_SFX',        b'</td>'),
    ('TD_TIME_MID',   b' time">'),
    ('TD_LOC_MID',    b' loc">'),
]

# ── Compute offsets ───────────────────────────────────────────────────
off = {}
cur = 200000
for name, data in statics:
    off[name] = cur
    cur += len(data)

d = dict(statics)
o = lambda n: off[n]
l = lambda n: len(d[n])

pages = max(6, (cur + 65535) // 65536)

# ── Emit WAT ──────────────────────────────────────────────────────────
wat = f"""\
;; eq — earthquake monitor on unikernel WASM
;; Route: GET /
;; Pages: {pages} ({pages*65536} bytes total)
;; Cache TTL: {CACHE_TTL}s, max entries: {MAX_ENTRIES}
;; Memory:
;;   0-65535:      response area
;;   65536-131071: request area
;;   131072:       last_fetch_ts (i64)
;;   131080:       feed_len (i32)
;;   131084:       feed_data ({MAX_FEED_LEN} bytes max)
;;   {TIME_BUF}:   scratch buffer (format_time, mag string)
;;   {ENTRY_AREA}: parsed entries ({MAX_ENTRIES} x {ENTRY_SLOT} bytes)
;;   200000+:      static blobs
(module
  (import "host" "log"      (func $log      (param i32 i32)))
  (import "host" "now"      (func $now      (result i64)))
  (import "host" "get_feed" (func $get_feed (param i32 i32) (result i32)))
  (memory (export "memory") {pages})

  (func (export "get_response_ptr") (result i32) i32.const 0)

  ;; ── handle_request ────────────────────────────────────────────────
  ;; Params: method_ptr, method_len, path_ptr, path_len, body_ptr, body_len
  ;; Returns: response byte length written at offset 0
  (func (export "handle_request")
    (param $mp i32)(param $ml i32)
    (param $pp i32)(param $pl i32)
    (param $bp i32)(param $bl i32)
    (result i32)
    (call $log (i32.const {o('LOG_INIT')}) (i32.const {l('LOG_INIT')}))
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const {l('PATH_ROOT')}))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const {o('PATH_ROOT')}) (i32.const {l('PATH_ROOT')})))
      (then (return (call $serve_root))))
    (memory.copy (i32.const 0) (i32.const {o('NOT_FOUND')}) (i32.const {l('NOT_FOUND')}))
    i32.const {l('NOT_FOUND')}
  )

  ;; ── meq: memory byte equality ─────────────────────────────────────
  (func $meq (param $a i32)(param $al i32)(param $b i32)(param $bl i32)(result i32)
    (local $i i32)
    (if (i32.ne (local.get $al) (local.get $bl)) (then (return (i32.const 0))))
    (block $k (loop $lp
      (br_if $k (i32.ge_u (local.get $i) (local.get $al)))
      (br_if $k (i32.ne
        (i32.load8_u (i32.add (local.get $a) (local.get $i)))
        (i32.load8_u (i32.add (local.get $b) (local.get $i)))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $lp)))
    (i32.eq (local.get $i) (local.get $al))
  )

  ;; ── cp: memcpy n bytes, return new dst pointer ────────────────────
  (func $cp (param $d i32)(param $s i32)(param $n i32)(result i32)
    (memory.copy (local.get $d) (local.get $s) (local.get $n))
    (i32.add (local.get $d) (local.get $n))
  )

  ;; ── wb: write single byte, return next pointer ────────────────────
  (func $wb (param $p i32)(param $b i32)(result i32)
    (i32.store8 (local.get $p) (local.get $b))
    (i32.add (local.get $p) (i32.const 1))
  )

  ;; ── write_u32_dec: write decimal digits of v at p, return new ptr ─
  (func $write_u32_dec (param $p i32)(param $v i32)(result i32)
    (local $start i32)(local $end i32)(local $tmp i32)
    (local.set $start (local.get $p))
    ;; Write digits in reverse order
    (block $done (loop $lp
      (i32.store8 (local.get $p)
                  (i32.add (i32.rem_u (local.get $v) (i32.const 10)) (i32.const 48)))
      (local.set $p (i32.add (local.get $p) (i32.const 1)))
      (local.set $v (i32.div_u (local.get $v) (i32.const 10)))
      (br_if $done (i32.eqz (local.get $v)))
      (br $lp)))
    ;; Reverse the written digits in-place
    (local.set $end (i32.sub (local.get $p) (i32.const 1)))
    (local.set $tmp (local.get $start))
    (block $rb (loop $rl
      (br_if $rb (i32.ge_u (local.get $tmp) (local.get $end)))
      (local.set $start (i32.load8_u (local.get $tmp)))
      (i32.store8 (local.get $tmp) (i32.load8_u (local.get $end)))
      (i32.store8 (local.get $end) (local.get $start))
      (local.set $tmp (i32.add (local.get $tmp) (i32.const 1)))
      (local.set $end (i32.sub (local.get $end) (i32.const 1)))
      (br $rl)))
    (local.get $p)
  )

  ;; ── maybe_refresh: fetch new feed data if cache is stale ──────────
  ;; Returns: 0=ok, -1=get_feed error
  (func $maybe_refresh (result i32)
    (local $ts i64)(local $last i64)(local $n i32)
    (local.set $ts   (call $now))
    (local.set $last (i64.load (i32.const {LAST_FETCH_TS})))
    (if (i64.ge_u
          (i64.sub (local.get $ts) (local.get $last))
          (i64.const {CACHE_TTL}))
      (then
        (call $log (i32.const {o('LOG_FETCH')}) (i32.const {l('LOG_FETCH')}))
        (local.set $n (call $get_feed (i32.const {FEED_DATA}) (i32.const {MAX_FEED_LEN})))
        (if (i32.lt_s (local.get $n) (i32.const 0)) (then
          (call $log (i32.const {o('LOG_ERR')}) (i32.const {l('LOG_ERR')}))
          (return (i32.const -1))
        ))
        (i32.store  (i32.const {FEED_LEN})      (local.get $n))
        (i64.store  (i32.const {LAST_FETCH_TS}) (local.get $ts))
      ) (else
        (call $log (i32.const {o('LOG_CACHE')}) (i32.const {l('LOG_CACHE')}))
      )
    )
    i32.const 0
  )

  ;; ── skip_ws: skip ASCII whitespace, return new index ──────────────
  (func $skip_ws (param $base i32)(param $i i32)(param $end i32)(result i32)
    (block $done (loop $lp
      (br_if $done (i32.ge_u (local.get $i) (local.get $end)))
      (br_if $done (i32.gt_u
        (i32.load8_u (i32.add (local.get $base) (local.get $i)))
        (i32.const 32)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $lp)))
    (local.get $i)
  )

  ;; ── find_field: scan for JSON key, return index after ':' or -1 ───
  ;; Searches for: "key_bytes": (with exact key length match)
  ;; Params: base, i (start offset into base), end (exclusive offset into base),
  ;;         kp (key bytes ptr), kl (key bytes len)
  ;; Returns: new i positioned after the ':', or -1 if not found
  (func $find_field
    (param $base i32)(param $i i32)(param $end i32)
    (param $kp i32)(param $kl i32)
    (result i32)
    (local $j i32)(local $ok i32)
    (block $done (loop $lp
      (br_if $done (i32.ge_u (local.get $i) (local.get $end)))
      ;; Look for opening '"'
      (if (i32.eq (i32.load8_u (i32.add (local.get $base) (local.get $i))) (i32.const 34))
        (then
          ;; Try to match key bytes after '"'
          (local.set $j  (i32.const 0))
          (local.set $ok (i32.const 1))
          (block $nm (loop $ml
            (br_if $nm (i32.ge_u (local.get $j) (local.get $kl)))
            ;; bounds check
            (if (i32.ge_u
                  (i32.add (i32.add (local.get $i) (i32.const 1)) (local.get $j))
                  (local.get $end))
              (then (local.set $ok (i32.const 0)) (br $nm)))
            ;; byte compare
            (if (i32.ne
                  (i32.load8_u (i32.add (local.get $base)
                                        (i32.add (i32.add (local.get $i) (i32.const 1))
                                                 (local.get $j))))
                  (i32.load8_u (i32.add (local.get $kp) (local.get $j))))
              (then (local.set $ok (i32.const 0)) (br $nm)))
            (local.set $j (i32.add (local.get $j) (i32.const 1)))
            (br $ml)))
          (if (local.get $ok)
            (then
              ;; Check closing '"' at position i+1+kl
              (if (i32.lt_u
                    (i32.add (i32.add (local.get $i) (i32.const 1)) (local.get $kl))
                    (local.get $end))
                (then
                  (if (i32.eq
                        (i32.load8_u (i32.add (local.get $base)
                                              (i32.add (i32.add (local.get $i) (i32.const 1))
                                                       (local.get $kl))))
                        (i32.const 34))
                    (then
                      ;; key found: advance past opening '"' + key + closing '"' + ':' = kl+3
                      (return (i32.add (local.get $i)
                                       (i32.add (local.get $kl) (i32.const 3))))))))))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $lp)))
    ;; not found
    i32.const -1
  )

  ;; ── hex_nib: ASCII hex char → 0-15 (invalid → 0) ─────────────────
  (func $hex_nib (param $c i32) (result i32)
    (if (i32.and (i32.ge_u (local.get $c) (i32.const 48))
                 (i32.le_u (local.get $c) (i32.const 57)))
      (then (return (i32.sub (local.get $c) (i32.const 48)))))
    (if (i32.and (i32.ge_u (local.get $c) (i32.const 65))
                 (i32.le_u (local.get $c) (i32.const 70)))
      (then (return (i32.sub (local.get $c) (i32.const 55)))))
    (if (i32.and (i32.ge_u (local.get $c) (i32.const 97))
                 (i32.le_u (local.get $c) (i32.const 102)))
      (then (return (i32.sub (local.get $c) (i32.const 87)))))
    i32.const 0
  )

  ;; ── read_str_val: read quoted JSON string value into buffer ────────
  ;; Handles \\uXXXX escape sequences → UTF-8 (covers Japanese BMP chars).
  ;; Writes up to max_out bytes into out_ptr.
  ;; Stores byte count at out_len_ptr (i32).
  ;; Returns: new i (past closing '"'), or -1 on error.
  (func $read_str_val
    (param $base i32)(param $i i32)(param $end i32)
    (param $out_ptr i32)(param $max_out i32)(param $out_len_ptr i32)
    (result i32)
    (local $c i32)(local $n i32)(local $cp i32)
    (local.set $i (call $skip_ws (local.get $base) (local.get $i) (local.get $end)))
    (if (i32.ge_u (local.get $i) (local.get $end)) (then (return (i32.const -1))))
    ;; must start with '"'
    (if (i32.ne (i32.load8_u (i32.add (local.get $base) (local.get $i))) (i32.const 34))
      (then (return (i32.const -1))))
    (local.set $i (i32.add (local.get $i) (i32.const 1)))
    (block $done (loop $lp
      (br_if $done (i32.ge_u (local.get $i) (local.get $end)))
      (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $i))))
      ;; closing '"'
      (br_if $done (i32.eq (local.get $c) (i32.const 34)))
      ;; handle backslash escape
      (if (i32.eq (local.get $c) (i32.const 92))
        (then
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br_if $done (i32.ge_u (local.get $i) (local.get $end)))
          (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $i))))
          ;; \\uXXXX → UTF-8
          (if (i32.eq (local.get $c) (i32.const 117))
            (then
              (local.set $cp (i32.const 0))
              ;; H3
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (if (i32.lt_u (local.get $i) (local.get $end))
                (then (local.set $cp (i32.shl
                  (call $hex_nib (i32.load8_u (i32.add (local.get $base) (local.get $i))))
                  (i32.const 12)))))
              ;; H2
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (if (i32.lt_u (local.get $i) (local.get $end))
                (then (local.set $cp (i32.or (local.get $cp) (i32.shl
                  (call $hex_nib (i32.load8_u (i32.add (local.get $base) (local.get $i))))
                  (i32.const 8))))))
              ;; H1
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (if (i32.lt_u (local.get $i) (local.get $end))
                (then (local.set $cp (i32.or (local.get $cp) (i32.shl
                  (call $hex_nib (i32.load8_u (i32.add (local.get $base) (local.get $i))))
                  (i32.const 4))))))
              ;; H0
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (if (i32.lt_u (local.get $i) (local.get $end))
                (then (local.set $cp (i32.or (local.get $cp)
                  (call $hex_nib (i32.load8_u (i32.add (local.get $base) (local.get $i))))))))
              ;; encode code point → UTF-8
              (if (i32.lt_u (local.get $cp) (i32.const 128))
                (then
                  (if (i32.lt_u (local.get $n) (local.get $max_out))
                    (then
                      (i32.store8 (i32.add (local.get $out_ptr) (local.get $n)) (local.get $cp))
                      (local.set $n (i32.add (local.get $n) (i32.const 1))))))
              (else (if (i32.lt_u (local.get $cp) (i32.const 2048))
                (then
                  (if (i32.lt_u (local.get $n) (local.get $max_out))
                    (then
                      (i32.store8 (i32.add (local.get $out_ptr) (local.get $n))
                        (i32.or (i32.const 192) (i32.shr_u (local.get $cp) (i32.const 6))))
                      (local.set $n (i32.add (local.get $n) (i32.const 1)))))
                  (if (i32.lt_u (local.get $n) (local.get $max_out))
                    (then
                      (i32.store8 (i32.add (local.get $out_ptr) (local.get $n))
                        (i32.or (i32.const 128) (i32.and (local.get $cp) (i32.const 63))))
                      (local.set $n (i32.add (local.get $n) (i32.const 1))))))
              (else
                ;; 3-byte (U+0800..U+FFFF) — all Japanese BMP chars
                (if (i32.lt_u (local.get $n) (local.get $max_out))
                  (then
                    (i32.store8 (i32.add (local.get $out_ptr) (local.get $n))
                      (i32.or (i32.const 224) (i32.shr_u (local.get $cp) (i32.const 12))))
                    (local.set $n (i32.add (local.get $n) (i32.const 1)))))
                (if (i32.lt_u (local.get $n) (local.get $max_out))
                  (then
                    (i32.store8 (i32.add (local.get $out_ptr) (local.get $n))
                      (i32.or (i32.const 128)
                        (i32.and (i32.shr_u (local.get $cp) (i32.const 6)) (i32.const 63))))
                    (local.set $n (i32.add (local.get $n) (i32.const 1)))))
                (if (i32.lt_u (local.get $n) (local.get $max_out))
                  (then
                    (i32.store8 (i32.add (local.get $out_ptr) (local.get $n))
                      (i32.or (i32.const 128) (i32.and (local.get $cp) (i32.const 63))))
                    (local.set $n (i32.add (local.get $n) (i32.const 1)))))))))
              ;; advance past last hex digit and restart loop
              (local.set $i (i32.add (local.get $i) (i32.const 1)))
              (br $lp)
            )))) ;; end if 'u', end if '\'
      ;; store byte if buffer not full
      (if (i32.lt_u (local.get $n) (local.get $max_out))
        (then
          (i32.store8 (i32.add (local.get $out_ptr) (local.get $n)) (local.get $c))
          (local.set $n (i32.add (local.get $n) (i32.const 1)))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $lp)))
    (i32.store (local.get $out_len_ptr) (local.get $n))
    ;; advance past closing '"'
    (i32.add (local.get $i) (i32.const 1))
  )

  ;; ── parse_signed_tenths: parse "+DD.D" → i32 tenths from base+i ──
  ;; Returns new i after parsing. Writes result (signed tenths) to out_ptr.
  ;; Handles missing decimal as 0.
  (func $parse_signed_tenths
    (param $base i32)(param $i i32)(param $end i32)(param $out_ptr i32)
    (result i32)
    (local $sign i32)(local $val i32)(local $c i32)
    (if (i32.ge_u (local.get $i) (local.get $end)) (then (return (local.get $i))))
    ;; sign
    (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $i))))
    (if (i32.eq (local.get $c) (i32.const 43)) ;; '+'
      (then (local.set $sign (i32.const 1))  (local.set $i (i32.add (local.get $i) (i32.const 1))))
    (else (if (i32.eq (local.get $c) (i32.const 45)) ;; '-'
      (then (local.set $sign (i32.const -1)) (local.set $i (i32.add (local.get $i) (i32.const 1))))
    (else (local.set $sign (i32.const 1))))))
    ;; integer digits
    (local.set $val (i32.const 0))
    (block $int_done (loop $int_lp
      (br_if $int_done (i32.ge_u (local.get $i) (local.get $end)))
      (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $i))))
      (br_if $int_done (i32.lt_u (local.get $c) (i32.const 48)))
      (br_if $int_done (i32.gt_u (local.get $c) (i32.const 57)))
      (local.set $val (i32.add (i32.mul (local.get $val) (i32.const 10))
                               (i32.sub (local.get $c) (i32.const 48))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $int_lp)))
    ;; convert to tenths
    (local.set $val (i32.mul (local.get $val) (i32.const 10)))
    ;; decimal digit (after '.')
    (if (i32.lt_u (local.get $i) (local.get $end))
      (then
        (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $i))))
        (if (i32.eq (local.get $c) (i32.const 46)) ;; '.'
          (then
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (if (i32.lt_u (local.get $i) (local.get $end))
              (then
                (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $i))))
                (if (i32.and (i32.ge_u (local.get $c) (i32.const 48))
                             (i32.le_u (local.get $c) (i32.const 57)))
                  (then
                    (local.set $val (i32.add (local.get $val) (i32.sub (local.get $c) (i32.const 48))))
                    (local.set $i (i32.add (local.get $i) (i32.const 1)))))))))))
    ;; apply sign and store
    (i32.store (local.get $out_ptr) (i32.mul (local.get $sign) (local.get $val)))
    (local.get $i)
  )

  ;; ── parse_cod: parse ISO 6709 "+DD.D+DDD.D+alt/" after find_field ─
  ;; Skips whitespace, '"', then calls parse_signed_tenths twice.
  ;; Writes lat_tenths to lat_ptr (i32) and lon_tenths to lon_ptr (i32).
  (func $parse_cod
    (param $base i32)(param $i i32)(param $end i32)
    (param $lat_ptr i32)(param $lon_ptr i32)
    (result i32)
    (local.set $i (call $skip_ws (local.get $base) (local.get $i) (local.get $end)))
    (if (i32.ge_u (local.get $i) (local.get $end)) (then (return (local.get $i))))
    ;; skip opening '"'
    (if (i32.eq (i32.load8_u (i32.add (local.get $base) (local.get $i))) (i32.const 34))
      (then (local.set $i (i32.add (local.get $i) (i32.const 1)))))
    ;; parse lat
    (local.set $i (call $parse_signed_tenths
      (local.get $base) (local.get $i) (local.get $end) (local.get $lat_ptr)))
    ;; parse lon
    (local.set $i (call $parse_signed_tenths
      (local.get $base) (local.get $i) (local.get $end) (local.get $lon_ptr)))
    (local.get $i)
  )

  ;; ── parse_feed: parse JSON array from FEED_DATA into ENTRY_AREA ───
  ;; Returns: count of parsed entries (0..{MAX_ENTRIES})
  (func $parse_feed (result i32)
    (local $base i32)(local $flen i32)(local $end i32)(local $i i32)
    (local $cnt i32)(local $entry i32)
    (local $fi i32)(local $tmp i32)
    (local $c i32)(local $obj_end i32)
    (local $depth i32)(local $in_str i32)(local $esc i32)
    (local.set $base (i32.const {FEED_DATA}))
    (local.set $flen (i32.load (i32.const {FEED_LEN})))
    (local.set $end  (i32.add (local.get $base) (local.get $flen)))
    ;; skip to '[' (array start)
    (block $arr_start (loop $pre
      (br_if $arr_start (i32.ge_u (i32.add (local.get $base) (local.get $i)) (local.get $end)))
      (if (i32.eq (i32.load8_u (i32.add (local.get $base) (local.get $i))) (i32.const 91))
        (then
          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $arr_start)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $pre)))
    ;; main loop: iterate JSON objects
    (block $arr_done (loop $arr_lp
      (br_if $arr_done (i32.ge_u (i32.add (local.get $base) (local.get $i)) (local.get $end)))
      (br_if $arr_done (i32.ge_u (local.get $cnt) (i32.const {MAX_ENTRIES})))
      ;; skip to '{{' or ']'
      (block $obj_found (loop $skip
        (br_if $obj_found (i32.ge_u (i32.add (local.get $base) (local.get $i)) (local.get $end)))
        (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $i))))
        (if (i32.eq (local.get $c) (i32.const 93)) (then (br $arr_done))) ;; ']'
        (if (i32.eq (local.get $c) (i32.const 123))                        ;; '{{'
          (then
            (local.set $i (i32.add (local.get $i) (i32.const 1)))
            (br $obj_found)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $skip)))
      ;; find matching '}}' with proper depth/string tracking (handles nested objects in "int" field)
      (local.set $obj_end (local.get $i))
      (local.set $depth (i32.const 1))
      (local.set $in_str (i32.const 0))
      (local.set $esc (i32.const 0))
      (block $ob_found (loop $ob
        (br_if $ob_found (i32.ge_u (i32.add (local.get $base) (local.get $obj_end)) (local.get $end)))
        (br_if $ob_found (i32.eqz (local.get $depth)))
        (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $obj_end))))
        (if (local.get $esc)
          (then (local.set $esc (i32.const 0)))
          (else (if (local.get $in_str)
            (then
              (if (i32.eq (local.get $c) (i32.const 92))  ;; '\'
                (then (local.set $esc (i32.const 1))))
              (if (i32.eq (local.get $c) (i32.const 34))  ;; '"'
                (then (local.set $in_str (i32.const 0)))))
            (else
              (if (i32.eq (local.get $c) (i32.const 34))  ;; '"' start string
                (then (local.set $in_str (i32.const 1))))
              (if (i32.eq (local.get $c) (i32.const 123))  ;; '{{'
                (then (local.set $depth (i32.add (local.get $depth) (i32.const 1)))))
              (if (i32.eq (local.get $c) (i32.const 125))  ;; '}}'
                (then
                  (local.set $depth (i32.sub (local.get $depth) (i32.const 1)))
                  (br_if $ob_found (i32.eqz (local.get $depth)))))))))
        (local.set $obj_end (i32.add (local.get $obj_end) (i32.const 1)))
        (br $ob)))
      ;; slot pointer
      (local.set $entry (i32.add (i32.const {ENTRY_AREA})
                                  (i32.mul (local.get $cnt) (i32.const {ENTRY_SLOT}))))
      (memory.fill (local.get $entry) (i32.const 0) (i32.const {ENTRY_SLOT}))
      ;; --- "at" field ---
      (local.set $fi (call $find_field
        (local.get $base) (local.get $i) (local.get $obj_end)
        (i32.const {o('KEY_AT')}) (i32.const {l('KEY_AT')})))
      (if (i32.ne (local.get $fi) (i32.const -1))
        (then
          (local.set $tmp (call $read_str_val
            (local.get $base) (local.get $fi) (local.get $obj_end)
            (i32.add (local.get $entry) (i32.const {ENTRY_AT}))
            (i32.const 25)
            (i32.add (local.get $entry) (i32.const {ENTRY_AT_LEN}))))))
      ;; --- "mag" field: quoted string "M2.8" ---
      (local.set $fi (call $find_field
        (local.get $base) (local.get $i) (local.get $obj_end)
        (i32.const {o('KEY_MAG')}) (i32.const {l('KEY_MAG')})))
      (if (i32.ne (local.get $fi) (i32.const -1))
        (then
          (local.set $fi (call $skip_ws (local.get $base) (local.get $fi) (local.get $obj_end)))
          (if (i32.lt_u (local.get $fi) (local.get $obj_end))
            (then
              ;; skip optional opening '"'
              (if (i32.eq (i32.load8_u (i32.add (local.get $base) (local.get $fi))) (i32.const 34))
                (then (local.set $fi (i32.add (local.get $fi) (i32.const 1)))))
              ;; skip 'M' (ASCII 77)
              (if (i32.and
                    (i32.lt_u (local.get $fi) (local.get $obj_end))
                    (i32.eq (i32.load8_u (i32.add (local.get $base) (local.get $fi))) (i32.const 77)))
                (then (local.set $fi (i32.add (local.get $fi) (i32.const 1)))))
              ;; read integer digit
              (if (i32.lt_u (local.get $fi) (local.get $obj_end))
                (then
                  (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $fi))))
                  (if (i32.and
                        (i32.ge_u (local.get $c) (i32.const 48))
                        (i32.le_u (local.get $c) (i32.const 57)))
                    (then
                      (i32.store8 (i32.add (local.get $entry) (i32.const {ENTRY_MAGI}))
                                  (i32.sub (local.get $c) (i32.const 48)))
                      (local.set $fi (i32.add (local.get $fi) (i32.const 1)))
                      ;; skip '.' (ASCII 46)
                      (if (i32.and
                            (i32.lt_u (local.get $fi) (local.get $obj_end))
                            (i32.eq (i32.load8_u (i32.add (local.get $base) (local.get $fi))) (i32.const 46)))
                        (then (local.set $fi (i32.add (local.get $fi) (i32.const 1)))))
                      ;; read fractional digit
                      (if (i32.lt_u (local.get $fi) (local.get $obj_end))
                        (then
                          (local.set $c (i32.load8_u (i32.add (local.get $base) (local.get $fi))))
                          (if (i32.and
                                (i32.ge_u (local.get $c) (i32.const 48))
                                (i32.le_u (local.get $c) (i32.const 57)))
                            (then
                              (i32.store8 (i32.add (local.get $entry) (i32.const {ENTRY_MAGF}))
                                          (i32.sub (local.get $c) (i32.const 48)))))))))))))))
      ;; --- "anm" field (location name, 30 bytes max) ---
      (local.set $fi (call $find_field
        (local.get $base) (local.get $i) (local.get $obj_end)
        (i32.const {o('KEY_ANM')}) (i32.const {l('KEY_ANM')})))
      (if (i32.ne (local.get $fi) (i32.const -1))
        (then
          (local.set $tmp (call $read_str_val
            (local.get $base) (local.get $fi) (local.get $obj_end)
            (i32.add (local.get $entry) (i32.const {ENTRY_ANM}))
            (i32.const 30)
            (i32.add (local.get $entry) (i32.const {ENTRY_ANMLEN}))))))
      ;; --- "cod" field: ISO 6709 "+DD.D+DDD.D+alt/" → lat_tenths, lon_tenths ---
      (local.set $fi (call $find_field
        (local.get $base) (local.get $i) (local.get $obj_end)
        (i32.const {o('KEY_COD')}) (i32.const {l('KEY_COD')})))
      (if (i32.ne (local.get $fi) (i32.const -1))
        (then
          (local.set $tmp (call $parse_cod
            (local.get $base) (local.get $fi) (local.get $obj_end)
            (i32.add (local.get $entry) (i32.const {ENTRY_LAT}))
            (i32.add (local.get $entry) (i32.const {ENTRY_LON}))))))
      ;; --- "maxi" field (intensity: "1".."7") ---
      (local.set $fi (call $find_field
        (local.get $base) (local.get $i) (local.get $obj_end)
        (i32.const {o('KEY_MAXI')}) (i32.const {l('KEY_MAXI')})))
      (if (i32.ne (local.get $fi) (i32.const -1))
        (then
          ;; skip whitespace and '"', read 1 byte
          (local.set $fi (call $skip_ws (local.get $base) (local.get $fi) (local.get $obj_end)))
          (if (i32.lt_u (local.get $fi) (local.get $obj_end))
            (then
              (if (i32.eq (i32.load8_u (i32.add (local.get $base) (local.get $fi))) (i32.const 34))
                (then (local.set $fi (i32.add (local.get $fi) (i32.const 1)))))
              (if (i32.lt_u (local.get $fi) (local.get $obj_end))
                (then
                  (i32.store8
                    (i32.add (local.get $entry) (i32.const {ENTRY_MAXI}))
                    (i32.load8_u (i32.add (local.get $base) (local.get $fi))))))))))

      (local.set $cnt (i32.add (local.get $cnt) (i32.const 1)))
      ;; advance past this object
      (local.set $i (i32.add (local.get $obj_end) (i32.const 1)))
      (br $arr_lp)))
    (local.get $cnt)
  )

  ;; ── write_svg_circle: emit <circle> for one quake ─────────────────
  ;; mag_int selects radius and fill color.
  (func $write_svg_circle (param $p i32)(param $x i32)(param $y i32)(param $mag_int i32)(result i32)
    (local $r i32)(local $color_idx i32)
    ;; Determine r and color_idx from magnitude integer
    (if (i32.lt_u (local.get $mag_int) (i32.const 3))
      (then (local.set $r (i32.const 4))  (local.set $color_idx (i32.const 0)))
    (else (if (i32.lt_u (local.get $mag_int) (i32.const 4))
      (then (local.set $r (i32.const 6))  (local.set $color_idx (i32.const 1)))
    (else (if (i32.lt_u (local.get $mag_int) (i32.const 5))
      (then (local.set $r (i32.const 8))  (local.set $color_idx (i32.const 2)))
    (else
          (local.set $r (i32.const 12)) (local.set $color_idx (i32.const 3))))))))
    (local.set $p (call $cp (local.get $p) (i32.const {o('SVG_CIR_PRE')}) (i32.const {l('SVG_CIR_PRE')})))
    (local.set $p (call $write_u32_dec (local.get $p) (local.get $x)))
    (local.set $p (call $cp (local.get $p) (i32.const {o('SVG_CY')}) (i32.const {l('SVG_CY')})))
    (local.set $p (call $write_u32_dec (local.get $p) (local.get $y)))
    (local.set $p (call $cp (local.get $p) (i32.const {o('SVG_R')}) (i32.const {l('SVG_R')})))
    (local.set $p (call $write_u32_dec (local.get $p) (local.get $r)))
    (local.set $p (call $cp (local.get $p) (i32.const {o('SVG_FILL')}) (i32.const {l('SVG_FILL')})))
    ;; color string: 6 bytes from COLOR_TABLE at color_idx*7
    (local.set $p (call $cp (local.get $p)
      (i32.add (i32.const {o('COLOR_TABLE')}) (i32.mul (local.get $color_idx) (i32.const 7)))
      (i32.const 6)))
    (local.set $p (call $cp (local.get $p) (i32.const {o('SVG_CIR_SFX')}) (i32.const {l('SVG_CIR_SFX')})))
    (local.get $p)
  )

  ;; ── write_td: emit <td class="mN">data</td> ───────────────────────
  ;; cls: 0-4 mapped to '0'-'4' (ASCII 48+cls)
  (func $write_td (param $p i32)(param $data i32)(param $dlen i32)(param $cls i32)(result i32)
    (local.set $p (call $cp  (local.get $p) (i32.const {o('TD_PRE')}) (i32.const {l('TD_PRE')})))
    (local.set $p (call $wb  (local.get $p) (i32.add (local.get $cls) (i32.const 48))))
    (local.set $p (call $cp  (local.get $p) (i32.const {o('TD_MID')}) (i32.const {l('TD_MID')})))
    (local.set $p (call $cp  (local.get $p) (local.get $data) (local.get $dlen)))
    (local.set $p (call $cp  (local.get $p) (i32.const {o('TD_SFX')}) (i32.const {l('TD_SFX')})))
    (local.get $p)
  )

  ;; ── format_time: extract "MM-DD HH:MM" from ISO-8601 at string ────
  ;; at: "YYYY-MM-DDTHH:MM:SS+09:00"
  ;;      0123456789012345
  ;; Writes 11 bytes to out. Returns 11, or 0 if at_len < 16.
  (func $format_time (param $at i32)(param $at_len i32)(param $out i32)(result i32)
    (if (i32.lt_u (local.get $at_len) (i32.const 16)) (then (return (i32.const 0))))
    (i32.store8 (i32.add (local.get $out) (i32.const 0))
                (i32.load8_u (i32.add (local.get $at) (i32.const 5))))   ;; M
    (i32.store8 (i32.add (local.get $out) (i32.const 1))
                (i32.load8_u (i32.add (local.get $at) (i32.const 6))))   ;; M
    (i32.store8 (i32.add (local.get $out) (i32.const 2)) (i32.const 45)) ;; '-'
    (i32.store8 (i32.add (local.get $out) (i32.const 3))
                (i32.load8_u (i32.add (local.get $at) (i32.const 8))))   ;; D
    (i32.store8 (i32.add (local.get $out) (i32.const 4))
                (i32.load8_u (i32.add (local.get $at) (i32.const 9))))   ;; D
    (i32.store8 (i32.add (local.get $out) (i32.const 5)) (i32.const 32)) ;; ' '
    (i32.store8 (i32.add (local.get $out) (i32.const 6))
                (i32.load8_u (i32.add (local.get $at) (i32.const 11))))  ;; H
    (i32.store8 (i32.add (local.get $out) (i32.const 7))
                (i32.load8_u (i32.add (local.get $at) (i32.const 12))))  ;; H
    (i32.store8 (i32.add (local.get $out) (i32.const 8)) (i32.const 58)) ;; ':'
    (i32.store8 (i32.add (local.get $out) (i32.const 9))
                (i32.load8_u (i32.add (local.get $at) (i32.const 14))))  ;; M
    (i32.store8 (i32.add (local.get $out) (i32.const 10))
                (i32.load8_u (i32.add (local.get $at) (i32.const 15))))  ;; M
    i32.const 11
  )

  ;; ── serve_root: build complete HTTP/HTML response ─────────────────
  (func $serve_root (result i32)
    (local $p i32)(local $rc i32)(local $cnt i32)(local $i i32)
    (local $entry i32)
    (local $at i32)(local $at_len i32)
    (local $anm i32)(local $anm_len i32)
    (local $mag_int i32)(local $mag_frac i32)
    (local $lat_t i32)(local $lon_t i32)
    (local $x i32)(local $y i32)(local $cls i32)(local $tmp_len i32)

    ;; refresh cache if stale
    (local.set $rc (call $maybe_refresh))
    (if (i32.eq (local.get $rc) (i32.const -1))
      (then
        (local.set $p (call $cp (i32.const 0)
          (i32.const {o('ERR_500')}) (i32.const {l('ERR_500')})))
        (local.set $p (call $cp (local.get $p)
          (i32.const {o('HTML_ERR')}) (i32.const {l('HTML_ERR')})))
        (return (local.get $p))))

    ;; HTTP header + HTML preamble
    (local.set $p (call $cp (i32.const 0)
      (i32.const {o('HTML_HDR')}) (i32.const {l('HTML_HDR')})))
    (local.set $p (call $cp (local.get $p)
      (i32.const {o('HTML_PRE')}) (i32.const {l('HTML_PRE')})))
    ;; SVG map open (circles injected below, then closed by HTML_MAP_CLOSE)
    (local.set $p (call $cp (local.get $p)
      (i32.const {o('SVG_MAP')}) (i32.const {l('SVG_MAP')})))

    ;; no data yet?
    (if (i32.eqz (i32.load (i32.const {FEED_LEN})))
      (then
        (call $log (i32.const {o('LOG_EMPTY')}) (i32.const {l('LOG_EMPTY')}))
        (local.set $p (call $cp (local.get $p)
          (i32.const {o('HTML_MAP_CLOSE')}) (i32.const {l('HTML_MAP_CLOSE')})))
        (local.set $p (call $cp (local.get $p)
          (i32.const {o('HTML_LOADING')}) (i32.const {l('HTML_LOADING')})))
        (local.set $p (call $cp (local.get $p)
          (i32.const {o('HTML_POST')}) (i32.const {l('HTML_POST')})))
        (return (local.get $p))))

    ;; parse JSON feed
    (local.set $cnt (call $parse_feed))

    ;; --- inject SVG circles for each entry (GPS coords → SVG x/y) ---
    ;; x = (lon_tenths * 92 - 98600) / 100
    ;; y = (91200 - lat_tenths * 186) / 100
    (local.set $i (i32.const 0))
    (block $svg_done (loop $svg_lp
      (br_if $svg_done (i32.ge_u (local.get $i) (local.get $cnt)))
      (local.set $entry (i32.add (i32.const {ENTRY_AREA})
                                  (i32.mul (local.get $i) (i32.const {ENTRY_SLOT}))))
      (local.set $mag_int (i32.load8_u (i32.add (local.get $entry) (i32.const {ENTRY_MAGI}))))
      (local.set $lat_t (i32.load (i32.add (local.get $entry) (i32.const {ENTRY_LAT}))))
      (local.set $lon_t (i32.load (i32.add (local.get $entry) (i32.const {ENTRY_LON}))))
      (if (i32.or (local.get $lat_t) (local.get $lon_t))
        (then
          ;; x = (lon_tenths * 92 - 98600) / 100
          (local.set $x (i32.div_s
            (i32.sub (i32.mul (local.get $lon_t) (i32.const 92)) (i32.const 98600))
            (i32.const 100)))
          ;; y = (91200 - lat_tenths * 186) / 100
          (local.set $y (i32.div_s
            (i32.sub (i32.const 91200) (i32.mul (local.get $lat_t) (i32.const 186)))
            (i32.const 100)))
          ;; only draw if within SVG bounds
          (if (i32.and
                (i32.and (i32.ge_s (local.get $x) (i32.const 0))
                         (i32.lt_s (local.get $x) (i32.const 500)))
                (i32.and (i32.ge_s (local.get $y) (i32.const 0))
                         (i32.lt_s (local.get $y) (i32.const 600))))
            (then
              (local.set $p (call $write_svg_circle
                (local.get $p) (local.get $x) (local.get $y) (local.get $mag_int)))))))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $svg_lp)))

    ;; close SVG
    (local.set $p (call $cp (local.get $p)
      (i32.const {o('HTML_MAP_CLOSE')}) (i32.const {l('HTML_MAP_CLOSE')})))

    ;; no parsed entries?
    (if (i32.eqz (local.get $cnt))
      (then
        (local.set $p (call $cp (local.get $p)
          (i32.const {o('HTML_NO_DATA')}) (i32.const {l('HTML_NO_DATA')})))
        (local.set $p (call $cp (local.get $p)
          (i32.const {o('HTML_POST')}) (i32.const {l('HTML_POST')})))
        (return (local.get $p))))

    ;; --- table ---
    (local.set $p (call $cp (local.get $p)
      (i32.const {o('HTML_TABLE_HDR')}) (i32.const {l('HTML_TABLE_HDR')})))

    (local.set $i (i32.const 0))
    (block $tbl_done (loop $tbl_lp
      (br_if $tbl_done (i32.ge_u (local.get $i) (local.get $cnt)))
      (local.set $entry (i32.add (i32.const {ENTRY_AREA})
                                  (i32.mul (local.get $i) (i32.const {ENTRY_SLOT}))))
      (local.set $at      (i32.add (local.get $entry) (i32.const {ENTRY_AT})))
      (local.set $at_len  (i32.load8_u (i32.add (local.get $entry) (i32.const {ENTRY_AT_LEN}))))
      (local.set $mag_int (i32.load8_u (i32.add (local.get $entry) (i32.const {ENTRY_MAGI}))))
      (local.set $mag_frac(i32.load8_u (i32.add (local.get $entry) (i32.const {ENTRY_MAGF}))))
      (local.set $anm     (i32.add (local.get $entry) (i32.const {ENTRY_ANM})))
      (local.set $anm_len (i32.load8_u (i32.add (local.get $entry) (i32.const {ENTRY_ANMLEN}))))

      ;; determine CSS class (0=low, 3=M3, 4=M4+)
      (local.set $cls (select (i32.const 3) (local.get $mag_int)
                              (i32.gt_u (local.get $mag_int) (i32.const 3))))
      (local.set $cls (select (i32.const 0) (local.get $cls)
                              (i32.eqz (local.get $cls))))

      ;; <tr>
      (local.set $p (call $cp (local.get $p) (i32.const {o('TR_OPEN')}) (i32.const {l('TR_OPEN')})))

      ;; time cell — <td class="mN time">HH:MM</td>
      (local.set $tmp_len (call $format_time
        (local.get $at) (local.get $at_len) (i32.const {TIME_BUF})))
      (local.set $p (call $cp (local.get $p) (i32.const {o('TD_PRE')}) (i32.const {l('TD_PRE')})))
      (local.set $p (call $wb (local.get $p) (i32.add (local.get $cls) (i32.const 48))))
      (local.set $p (call $cp (local.get $p) (i32.const {o('TD_TIME_MID')}) (i32.const {l('TD_TIME_MID')})))
      (local.set $p (call $cp (local.get $p) (i32.const {TIME_BUF}) (local.get $tmp_len)))
      (local.set $p (call $cp (local.get $p) (i32.const {o('TD_SFX')}) (i32.const {l('TD_SFX')})))

      ;; location cell — <td class="mN loc">name</td>
      (local.set $p (call $cp (local.get $p) (i32.const {o('TD_PRE')}) (i32.const {l('TD_PRE')})))
      (local.set $p (call $wb (local.get $p) (i32.add (local.get $cls) (i32.const 48))))
      (local.set $p (call $cp (local.get $p) (i32.const {o('TD_LOC_MID')}) (i32.const {l('TD_LOC_MID')})))
      (local.set $p (call $cp (local.get $p) (local.get $anm) (local.get $anm_len)))
      (local.set $p (call $cp (local.get $p) (i32.const {o('TD_SFX')}) (i32.const {l('TD_SFX')})))

      ;; magnitude cell: write "M<int>.<frac>" to TIME_BUF+12 (4 bytes)
      (i32.store8 (i32.add (i32.const {TIME_BUF}) (i32.const 12)) (i32.const 77))  ;; 'M'
      (i32.store8 (i32.add (i32.const {TIME_BUF}) (i32.const 13))
                  (i32.add (local.get $mag_int)  (i32.const 48)))
      (i32.store8 (i32.add (i32.const {TIME_BUF}) (i32.const 14)) (i32.const 46))  ;; '.'
      (i32.store8 (i32.add (i32.const {TIME_BUF}) (i32.const 15))
                  (i32.add (local.get $mag_frac) (i32.const 48)))
      (local.set $p (call $write_td
        (local.get $p) (i32.add (i32.const {TIME_BUF}) (i32.const 12))
        (i32.const 4) (local.get $cls)))

      ;; intensity cell (maxi = JMA intensity "1".."7")
      (i32.store8 (i32.const {TIME_BUF}) (i32.load8_u (i32.add (local.get $entry) (i32.const {ENTRY_MAXI}))))
      (local.set $p (call $write_td (local.get $p) (i32.const {TIME_BUF}) (i32.const 1) (local.get $cls)))

      ;; </tr>
      (local.set $p (call $cp (local.get $p) (i32.const {o('TR_CLOSE')}) (i32.const {l('TR_CLOSE')})))

      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $tbl_lp)))

    (local.set $p (call $cp (local.get $p)
      (i32.const {o('HTML_TABLE_FTR')}) (i32.const {l('HTML_TABLE_FTR')})))
    (local.set $p (call $cp (local.get $p)
      (i32.const {o('HTML_POST')}) (i32.const {l('HTML_POST')})))
    (local.get $p)
  )

"""

# Emit all data sections
for name, data in statics:
    wat += f'  (data (i32.const {off[name]}) {wat_esc(data)})\n'
wat += ')\n'

print(wat, end='')
