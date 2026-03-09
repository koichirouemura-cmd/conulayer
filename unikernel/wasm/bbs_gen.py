#!/usr/bin/env python3
"""Generate WAT source for in-memory BBS on unikernel WASM."""

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

# ── Static blobs ────────────────────────────────────────────────
HTML_HDR  = b"HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n"
JSON_HDR  = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n"
CRTD_HDR  = b"HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n"
NOT_FOUND = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nNot Found"
BAD_REQ   = b"HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nBad Request"
LOG_MSG   = b"handle_request called"
JSON_PFX  = b'{"messages":['
JSON_SFX  = b']}'
PATH_ROOT = b"/"
PATH_MSGS = b"/api/messages"
PATH_POST = b"/post"

HTML = (
    b'<!DOCTYPE html><html><head><meta charset="UTF-8">'
    b'<title>BBS on unikernel</title>'
    b'<style>'
    b'*{margin:0;padding:0;box-sizing:border-box}'
    b'body{background:#0d1117;color:#e6edf3;font-family:monospace;'
    b'padding:20px;max-width:600px;margin:0 auto}'
    b'h1{color:#58a6ff;margin-bottom:16px;font-size:18px}'
    b'small{color:#8b949e;font-weight:normal;margin-left:8px;font-size:12px}'
    b'textarea{width:100%;background:#161b22;border:1px solid #30363d;'
    b'color:#e6edf3;padding:10px;border-radius:6px;font-family:monospace;'
    b'font-size:13px;resize:vertical;min-height:80px}'
    b'textarea:focus{outline:none;border-color:#58a6ff}'
    b'button{margin-top:8px;background:#238636;color:#fff;border:none;'
    b'padding:8px 20px;border-radius:6px;cursor:pointer;font-size:13px}'
    b'button:hover{background:#2ea043}'
    b'#msgs{margin-top:20px;display:flex;flex-direction:column;gap:10px}'
    b'.msg{background:#161b22;border:1px solid #30363d;border-radius:6px;'
    b'padding:12px;font-size:13px;word-break:break-all;line-height:1.5}'
    b'.empty{color:#8b949e;font-size:13px}'
    b'</style></head><body>'
    b'<h1>BBS <small>on unikernel WASM</small></h1>'
    b'<textarea id="t" placeholder="Ctrl+Enter to post" maxlength="200"></textarea>'
    b'<button onclick="post()">Post</button>'
    b'<div id="msgs"><p class="empty">Loading...</p></div>'
    b'<script>'
    b'async function post(){'
    b'const t=document.getElementById("t"),m=t.value.trim();'
    b'if(!m)return;'
    b'try{await fetch("/post",{method:"POST",body:m});t.value="";load()}'
    b'catch(e){}'
    b'}'
    b'async function load(){'
    b'try{'
    b'const r=await fetch("/api/messages"),d=await r.json();'
    b'const e=document.getElementById("msgs");'
    b'if(!d.messages||!d.messages.length){'
    b'e.innerHTML=\'<p class="empty">No messages yet</p>\';return}'
    b'e.innerHTML=d.messages.slice().reverse().map(m=>'
    b'\'<div class="msg">\'+m.replace(/&/g,"&amp;").replace(/</g,"&lt;").replace(/>/g,"&gt;")+\'</div>\''
    b').join("")'
    b'}catch(e){}}'
    b'document.getElementById("t").addEventListener("keydown",e=>{'
    b'if(e.ctrlKey&&e.key==="Enter")post()});'
    b'load();setInterval(load,3000)'
    b'</script></body></html>'
)

# ── Memory layout ────────────────────────────────────────────────
MSG_COUNT = 131072
MSG_AREA  = 131076
SLOT_SIZE = 256
MAX_MSGS  = 20
SHIFT_DST = MSG_AREA
SHIFT_SRC = MSG_AREA + SLOT_SIZE
SHIFT_LEN = (MAX_MSGS - 1) * SLOT_SIZE
LAST_SLOT = MSG_AREA + (MAX_MSGS - 1) * SLOT_SIZE

statics = [
    ('HTML_HDR',  HTML_HDR),
    ('JSON_HDR',  JSON_HDR),
    ('CRTD_HDR',  CRTD_HDR),
    ('NOT_FOUND', NOT_FOUND),
    ('BAD_REQ',   BAD_REQ),
    ('LOG_MSG',   LOG_MSG),
    ('JSON_PFX',  JSON_PFX),
    ('JSON_SFX',  JSON_SFX),
    ('PATH_ROOT', PATH_ROOT),
    ('PATH_MSGS', PATH_MSGS),
    ('PATH_POST', PATH_POST),
    ('HTML',      HTML),
]

off = {}
cur = 200000
for name, data in statics:
    off[name] = cur
    cur += len(data)

pages = max(4, (cur + 65535) // 65536)
d = dict(statics)
o = lambda n: off[n]
l = lambda n: len(d[n])

wat = f"""\
;; BBS — in-memory bulletin board on unikernel WASM
;; Routes: GET /  GET /api/messages  POST /post
;; Pages: {pages} ({pages*65536} bytes total)
(module
  (import "host" "log" (func $log (param i32 i32)))
  (memory (export "memory") {pages})

  (func (export "get_response_ptr") (result i32) i32.const 0)

  (func (export "handle_request")
    (param $mp i32)(param $ml i32)
    (param $pp i32)(param $pl i32)
    (param $bp i32)(param $bl i32)
    (result i32)
    (call $log (i32.const {o('LOG_MSG')}) (i32.const {l('LOG_MSG')}))
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const {l('PATH_ROOT')}))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const {o('PATH_ROOT')}) (i32.const {l('PATH_ROOT')})))
      (then (return (call $html))))
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const {l('PATH_MSGS')}))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const {o('PATH_MSGS')}) (i32.const {l('PATH_MSGS')})))
      (then (return (call $msgs))))
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const {l('PATH_POST')}))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const {o('PATH_POST')}) (i32.const {l('PATH_POST')})))
      (then (return (call $post (local.get $bp) (local.get $bl)))))
    (memory.copy (i32.const 0) (i32.const {o('NOT_FOUND')}) (i32.const {l('NOT_FOUND')}))
    i32.const {l('NOT_FOUND')}
  )

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

  (func $cp (param $d i32)(param $s i32)(param $n i32)(result i32)
    (memory.copy (local.get $d) (local.get $s) (local.get $n))
    (i32.add (local.get $d) (local.get $n))
  )

  (func $html (result i32)
    (local $p i32)
    (local.set $p (call $cp (i32.const 0)
      (i32.const {o('HTML_HDR')}) (i32.const {l('HTML_HDR')})))
    (local.set $p (call $cp (local.get $p)
      (i32.const {o('HTML')}) (i32.const {l('HTML')})))
    (local.get $p)
  )

  (func $post (param $bp i32)(param $bl i32)(result i32)
    (local $cnt i32)(local $sl i32)(local $al i32)
    (local.set $al (select (i32.const 252) (local.get $bl)
                           (i32.gt_u (local.get $bl) (i32.const 252))))
    (if (i32.eqz (local.get $al)) (then
      (memory.copy (i32.const 0) (i32.const {o('BAD_REQ')}) (i32.const {l('BAD_REQ')}))
      (return (i32.const {l('BAD_REQ')}))))
    (local.set $cnt (i32.load (i32.const {MSG_COUNT})))
    (if (i32.ge_u (local.get $cnt) (i32.const {MAX_MSGS})) (then
      (memory.copy (i32.const {SHIFT_DST}) (i32.const {SHIFT_SRC}) (i32.const {SHIFT_LEN}))
      (local.set $sl (i32.const {LAST_SLOT}))
    ) (else
      (local.set $sl (i32.add (i32.const {MSG_AREA})
                              (i32.mul (local.get $cnt) (i32.const {SLOT_SIZE}))))
      (i32.store (i32.const {MSG_COUNT}) (i32.add (local.get $cnt) (i32.const 1)))
    ))
    (i32.store (local.get $sl) (local.get $al))
    (memory.copy (i32.add (local.get $sl) (i32.const 4)) (local.get $bp) (local.get $al))
    (memory.copy (i32.const 0) (i32.const {o('CRTD_HDR')}) (i32.const {l('CRTD_HDR')}))
    i32.const {l('CRTD_HDR')}
  )

  (func $msgs (result i32)
    (local $p i32)(local $cnt i32)(local $i i32)
    (local $sl i32)(local $ml i32)(local $mp i32)
    (local $j i32)(local $b i32)
    (local.set $p (call $cp (i32.const 0)
      (i32.const {o('JSON_HDR')}) (i32.const {l('JSON_HDR')})))
    (local.set $p (call $cp (local.get $p)
      (i32.const {o('JSON_PFX')}) (i32.const {l('JSON_PFX')})))
    (local.set $cnt (i32.load (i32.const {MSG_COUNT})))
    (block $ob (loop $ol
      (br_if $ob (i32.ge_u (local.get $i) (local.get $cnt)))
      (local.set $sl (i32.add (i32.const {MSG_AREA})
                              (i32.mul (local.get $i) (i32.const {SLOT_SIZE}))))
      (local.set $ml (i32.load (local.get $sl)))
      (local.set $mp (i32.add (local.get $sl) (i32.const 4)))
      (i32.store8 (local.get $p) (i32.const 34))
      (local.set $p (i32.add (local.get $p) (i32.const 1)))
      (local.set $j (i32.const 0))
      (block $ib (loop $il
        (br_if $ib (i32.ge_u (local.get $j) (local.get $ml)))
        (local.set $b (i32.load8_u (i32.add (local.get $mp) (local.get $j))))
        (if (i32.eq (local.get $b) (i32.const 34)) (then
          (i32.store8 (local.get $p) (i32.const 92))
          (local.set $p (i32.add (local.get $p) (i32.const 1)))
          (i32.store8 (local.get $p) (i32.const 34))
          (local.set $p (i32.add (local.get $p) (i32.const 1)))
        ) (else (if (i32.eq (local.get $b) (i32.const 92)) (then
          (i32.store8 (local.get $p) (i32.const 92))
          (local.set $p (i32.add (local.get $p) (i32.const 1)))
          (i32.store8 (local.get $p) (i32.const 92))
          (local.set $p (i32.add (local.get $p) (i32.const 1)))
        ) (else (if (i32.ge_u (local.get $b) (i32.const 32)) (then
          (i32.store8 (local.get $p) (local.get $b))
          (local.set $p (i32.add (local.get $p) (i32.const 1)))
        ))))))
        (local.set $j (i32.add (local.get $j) (i32.const 1)))
        (br $il)
      ))
      (i32.store8 (local.get $p) (i32.const 34))
      (local.set $p (i32.add (local.get $p) (i32.const 1)))
      (if (i32.lt_u (i32.add (local.get $i) (i32.const 1)) (local.get $cnt)) (then
        (i32.store8 (local.get $p) (i32.const 44))
        (local.set $p (i32.add (local.get $p) (i32.const 1)))
      ))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $ol)
    ))
    (local.set $p (call $cp (local.get $p)
      (i32.const {o('JSON_SFX')}) (i32.const {l('JSON_SFX')})))
    (local.get $p)
  )

"""
for name, data in statics:
    wat += f'  (data (i32.const {off[name]}) {wat_esc(data)})\n'
wat += ')\n'

print(wat, end='')
