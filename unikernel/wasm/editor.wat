;; editor.wat — Web text editor WASM for unikernel
;; Routes:
;;   GET  /editor/api/files  → JSON array of filenames in /doc/
;;   POST /editor/api/read   → body=filename → file content
;;   POST /editor/api/save   → body="filename\ncontent" → 200 OK
;;
;; Memory layout:
;;   Page 0 (0-65535):       response output buffer
;;   Page 1 (65536-131071):  request area (managed by wasm_rt.rs)
;;     65536: method (256 bytes)
;;     65792: path   (1024 bytes)
;;     66816: body   (32768 bytes)
;;   Page 2 (131072-196607): path scratch (256 bytes) + file content buffer
;;     131072: constructed path /doc/filename (256 bytes)
;;     131328: file content read buffer (32512 bytes)
;;   Page 3 (196608-262143): file list buffer (32768 bytes)
;;   Page 4+ (262144+):      static data
(module
  (import "host" "log"        (func $log        (param i32 i32)))
  (import "host" "file_read"  (func $file_read  (param i32 i32 i32 i32) (result i32)))
  (import "host" "file_write" (func $file_write (param i32 i32 i32 i32) (result i32)))
  (import "host" "file_list"  (func $file_list  (param i32 i32 i32 i32) (result i32)))

  (memory (export "memory") 6)

  (func (export "get_response_ptr") (result i32) i32.const 0)

  ;; handle_request(mp, ml, pp, pl, bp, bl) -> i32
  (func (export "handle_request")
    (param $mp i32)(param $ml i32)
    (param $pp i32)(param $pl i32)
    (param $bp i32)(param $bl i32)
    (result i32)

    (call $log (i32.const 262546) (i32.const 21))

    ;; Route: /api/files (10 bytes) — net.rs strips "/editor" prefix
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const 10))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const 262518) (i32.const 10)))
      (then (return (call $files))))

    ;; Route: /api/read (9 bytes)
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const 9))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const 262528) (i32.const 9)))
      (then (return (call $read (local.get $bp) (local.get $bl)))))

    ;; Route: /api/save (9 bytes)
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const 9))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const 262537) (i32.const 9)))
      (then (return (call $save (local.get $bp) (local.get $bl)))))

    ;; 404
    (memory.copy (i32.const 0) (i32.const 262346) (i32.const 80))
    i32.const 80
  )

  ;; ── $meq: compare two byte ranges for equality ──────────────────
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

  ;; ── $cp: copy bytes, return new destination pointer ─────────────
  (func $cp (param $d i32)(param $s i32)(param $n i32)(result i32)
    (memory.copy (local.get $d) (local.get $s) (local.get $n))
    (i32.add (local.get $d) (local.get $n))
  )

  ;; ── $files: GET /editor/api/files → JSON array ──────────────────
  ;; file_list("/doc/", 5, 196608, 32768) returns newline-separated names
  ;; Convert to ["name1","name2"] JSON
  (func $files (result i32)
    (local $n i32)    ;; bytes returned by file_list
    (local $p i32)    ;; write pointer into response (page 0)
    (local $i i32)    ;; index into list buffer
    (local $b i32)    ;; current byte
    (local $first i32);; 1 if first entry
    (local $in_name i32) ;; 1 if currently reading a name

    ;; call file_list("doc/", 196608, 32768)
    (local.set $n (call $file_list
      (i32.const 262513) (i32.const 4)
      (i32.const 196608) (i32.const 32768)))

    ;; write response header: HTTP/1.1 200 OK (JSON) — 102 bytes
    (local.set $p (call $cp (i32.const 0) (i32.const 262144) (i32.const 102)))

    ;; write opening bracket
    (i32.store8 (local.get $p) (i32.const 91)) ;; '['
    (local.set $p (i32.add (local.get $p) (i32.const 1)))

    (local.set $first (i32.const 1))
    (local.set $in_name (i32.const 0))
    (local.set $i (i32.const 0))

    ;; if file_list failed or returned nothing, just output []
    (if (i32.gt_s (local.get $n) (i32.const 0))
      (then
        (block $done (loop $loop
          (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
          (local.set $b (i32.load8_u (i32.add (i32.const 196608) (local.get $i))))

          (if (i32.eq (local.get $b) (i32.const 10)) ;; '\n'
            (then
              (if (local.get $in_name)
                (then
                  ;; close quote
                  (i32.store8 (local.get $p) (i32.const 34)) ;; '"'
                  (local.set $p (i32.add (local.get $p) (i32.const 1)))
                  (local.set $in_name (i32.const 0))
                ))
            )
            (else
              (if (i32.eqz (local.get $in_name))
                (then
                  ;; start new entry
                  (if (i32.eqz (local.get $first))
                    (then
                      (i32.store8 (local.get $p) (i32.const 44)) ;; ','
                      (local.set $p (i32.add (local.get $p) (i32.const 1)))
                    )
                  )
                  (i32.store8 (local.get $p) (i32.const 34)) ;; '"'
                  (local.set $p (i32.add (local.get $p) (i32.const 1)))
                  (local.set $first (i32.const 0))
                  (local.set $in_name (i32.const 1))
                  ;; write the byte itself
                  (i32.store8 (local.get $p) (local.get $b))
                  (local.set $p (i32.add (local.get $p) (i32.const 1)))
                )
                (else
                  ;; inside a name, just write the byte
                  (i32.store8 (local.get $p) (local.get $b))
                  (local.set $p (i32.add (local.get $p) (i32.const 1)))
                )
              )
            )
          )

          (local.set $i (i32.add (local.get $i) (i32.const 1)))
          (br $loop)
        ))
        ;; close any trailing name (no trailing newline case)
        (if (local.get $in_name)
          (then
            (i32.store8 (local.get $p) (i32.const 34)) ;; '"'
            (local.set $p (i32.add (local.get $p) (i32.const 1)))
          )
        )
      )
    )

    ;; write closing bracket
    (i32.store8 (local.get $p) (i32.const 93)) ;; ']'
    (local.set $p (i32.add (local.get $p) (i32.const 1)))

    local.get $p
  )

  ;; ── $read: POST /editor/api/read → file content ─────────────────
  ;; body = filename (e.g. "test.txt")
  ;; Build path /doc/filename in scratch at 131072
  ;; Call file_read, return content
  (func $read (param $bp i32)(param $bl i32)(result i32)
    (local $path_ptr i32)
    (local $path_len i32)
    (local $n i32)
    (local $p i32)

    ;; limit filename length to 240 chars
    (local.set $bl (select (i32.const 240) (local.get $bl)
                           (i32.gt_u (local.get $bl) (i32.const 240))))

    (if (i32.eqz (local.get $bl)) (then
      (memory.copy (i32.const 0) (i32.const 262427) (i32.const 84))
      (return (i32.const 84))))

    ;; construct doc/filename at 131072
    (memory.copy (i32.const 131072) (i32.const 262513) (i32.const 4)) ;; "doc/"
    (memory.copy (i32.add (i32.const 131072) (i32.const 4)) (local.get $bp) (local.get $bl))
    (local.set $path_ptr (i32.const 131072))
    (local.set $path_len (i32.add (i32.const 4) (local.get $bl)))

    ;; call file_read
    (local.set $n (call $file_read
      (local.get $path_ptr) (local.get $path_len)
      (i32.const 131328) (i32.const 32512)))

    (if (i32.lt_s (local.get $n) (i32.const 0)) (then
      (memory.copy (i32.const 0) (i32.const 262346) (i32.const 80))
      (return (i32.const 80))))

    ;; write response: HTTP 200 text/plain header + content
    (local.set $p (call $cp (i32.const 0) (i32.const 262248) (i32.const 96)))
    (memory.copy (local.get $p) (i32.const 131328) (local.get $n))
    (local.set $p (i32.add (local.get $p) (local.get $n)))
    local.get $p
  )

  ;; ── $save: POST /editor/api/save ────────────────────────────────
  ;; body = "filename\ncontent"
  ;; Find '\n', split, write file
  (func $save (param $bp i32)(param $bl i32)(result i32)
    (local $i i32)
    (local $nl_pos i32)  ;; position of '\n' in body
    (local $found i32)
    (local $name_ptr i32)
    (local $name_len i32)
    (local $cont_ptr i32)
    (local $cont_len i32)
    (local $path_len i32)
    (local $ret i32)

    (if (i32.eqz (local.get $bl)) (then
      (memory.copy (i32.const 0) (i32.const 262427) (i32.const 84))
      (return (i32.const 84))))

    ;; find '\n' (10) in body
    (local.set $found (i32.const 0))
    (local.set $i (i32.const 0))
    (block $found_blk (loop $search
      (br_if $found_blk (i32.ge_u (local.get $i) (local.get $bl)))
      (if (i32.eq (i32.load8_u (i32.add (local.get $bp) (local.get $i))) (i32.const 10))
        (then
          (local.set $nl_pos (local.get $i))
          (local.set $found (i32.const 1))
          (br $found_blk)))
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $search)
    ))

    (if (i32.eqz (local.get $found)) (then
      (memory.copy (i32.const 0) (i32.const 262427) (i32.const 84))
      (return (i32.const 84))))

    (local.set $name_ptr (local.get $bp))
    (local.set $name_len (local.get $nl_pos))
    (local.set $cont_ptr (i32.add (local.get $bp) (i32.add (local.get $nl_pos) (i32.const 1))))
    (local.set $cont_len (i32.sub (local.get $bl) (i32.add (local.get $nl_pos) (i32.const 1))))

    ;; validate: name must be non-empty and <= 240 bytes
    (if (i32.or (i32.eqz (local.get $name_len))
                (i32.gt_u (local.get $name_len) (i32.const 240)))
      (then
        (memory.copy (i32.const 0) (i32.const 262427) (i32.const 84))
        (return (i32.const 84))))

    ;; construct doc/filename at 131072
    (memory.copy (i32.const 131072) (i32.const 262513) (i32.const 4)) ;; "doc/"
    (memory.copy (i32.add (i32.const 131072) (i32.const 4))
                 (local.get $name_ptr) (local.get $name_len))
    (local.set $path_len (i32.add (i32.const 4) (local.get $name_len)))

    ;; call file_write
    (local.set $ret (call $file_write
      (i32.const 131072) (local.get $path_len)
      (local.get $cont_ptr) (local.get $cont_len)))

    (if (i32.lt_s (local.get $ret) (i32.const 0)) (then
      (memory.copy (i32.const 0) (i32.const 262427) (i32.const 84))
      (return (i32.const 84))))

    ;; return 200 OK
    (memory.copy (i32.const 0) (i32.const 262248) (i32.const 96))
    i32.const 96
  )

  ;; ── Static data ─────────────────────────────────────────────────
  ;; 262144: HTTP 200 OK (JSON) — 104 bytes
  (data (i32.const 262144) "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n")
  ;; 262248: HTTP 200 OK (text/plain) — 98 bytes
  (data (i32.const 262248) "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n")
  ;; 262346: HTTP 404 — 81 bytes
  (data (i32.const 262346) "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nNot Found")
  ;; 262427: HTTP 400 — 86 bytes
  (data (i32.const 262427) "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nBad Request")
  ;; 262513: "doc/" — 4 bytes
  (data (i32.const 262513) "doc/")
  ;; 262518: "/api/files" — 10 bytes (net.rs strips "/editor" prefix)
  (data (i32.const 262518) "/api/files")
  ;; 262528: "/api/read" — 9 bytes
  (data (i32.const 262528) "/api/read")
  ;; 262537: "/api/save" — 9 bytes
  (data (i32.const 262537) "/api/save")
  ;; 262546: "handle_request called" — 21 bytes
  (data (i32.const 262546) "handle_request called")
)
