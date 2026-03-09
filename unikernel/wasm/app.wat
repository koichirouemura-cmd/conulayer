;; app.wat — Earthquake Monitor JSON API (SPA backend)
;; Routes: GET /api/quake → raw feed JSON, others → 404
;; Pages: 4 (262144 bytes total)
(module
  (import "host" "log" (func $log (param i32 i32)))
  (import "host" "get_feed" (func $get_feed (param i32 i32) (result i32)))
  (memory (export "memory") 4)

  (func (export "get_response_ptr") (result i32) i32.const 0)

  (func (export "handle_request")
    (param $mp i32)(param $ml i32)
    (param $pp i32)(param $pl i32)
    (param $bp i32)(param $bl i32)
    (result i32)
    (call $log (i32.const 200000) (i32.const 18))
    ;; "api/quake" (9 bytes) — プレフィックス "/" を除いたパスで比較
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const 9))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const 200018) (i32.const 9)))
      (then (return (call $quake))))
    ;; 404
    (memory.copy (i32.const 0) (i32.const 200132) (i32.const 82))
    i32.const 82
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

  ;; /api/quake: prepend JSON header, write feed data after it
  (func $quake (result i32)
    (local $n i32)
    ;; JSON 200 OK header → offset 0 (102 bytes)
    (memory.copy (i32.const 0) (i32.const 200028) (i32.const 102))
    ;; feed data → offset 102 (up to 195000 bytes, stays below data section at 200000)
    (local.set $n (call $get_feed (i32.const 102) (i32.const 195000)))
    ;; n <= 0: no data or error → 404
    (if (i32.le_s (local.get $n) (i32.const 0)) (then
      (memory.copy (i32.const 0) (i32.const 200132) (i32.const 82))
      (return (i32.const 82))
    ))
    (i32.add (i32.const 102) (local.get $n))
  )

  ;; offset 200000: log message (18 bytes)
  (data (i32.const 200000) "eq: handle_request")
  ;; offset 200018: route path (9 bytes, no leading slash — stripped by kernel)
  (data (i32.const 200018) "api/quake")
  ;; offset 200028: JSON 200 OK header (102 bytes)
  ;; HTTP/1.1 200 OK\r\n(17) + Content-Type: application/json\r\n(32) + Access-Control-Allow-Origin: *\r\n(32) + Connection: close\r\n(19) + \r\n(2) = 102
  (data (i32.const 200028) "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n")
  ;; offset 200132: 404 response (82 bytes)
  (data (i32.const 200132) "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nNot Found")
)
