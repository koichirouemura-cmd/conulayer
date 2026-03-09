;; BBS — in-memory bulletin board on unikernel WASM
;; Routes: GET /api/messages  POST /post
;; Pages: 4 (262144 bytes total)
(module
  (import "host" "log" (func $log (param i32 i32)))
  (memory (export "memory") 4)

  (func (export "get_response_ptr") (result i32) i32.const 0)

  (func (export "handle_request")
    (param $mp i32)(param $ml i32)
    (param $pp i32)(param $pl i32)
    (param $bp i32)(param $bl i32)
    (result i32)
    (call $log (i32.const 200483) (i32.const 21))
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const 13))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const 200520) (i32.const 13)))
      (then (return (call $msgs))))
    (if (i32.and
          (i32.eq (local.get $pl) (i32.const 5))
          (call $meq (local.get $pp) (local.get $pl)
                     (i32.const 200533) (i32.const 5)))
      (then (return (call $post (local.get $bp) (local.get $bl)))))
    (memory.copy (i32.const 0) (i32.const 200319) (i32.const 80))
    i32.const 80
  )

  ;; removed: $html function (GET / served as static file by Rust layer)

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

  (func $post (param $bp i32)(param $bl i32)(result i32)
    (local $cnt i32)(local $sl i32)(local $al i32)
    (local.set $al (select (i32.const 252) (local.get $bl)
                           (i32.gt_u (local.get $bl) (i32.const 252))))
    (if (i32.eqz (local.get $al)) (then
      (memory.copy (i32.const 0) (i32.const 200399) (i32.const 84))
      (return (i32.const 84))))
    (local.set $cnt (i32.load (i32.const 131072)))
    (if (i32.ge_u (local.get $cnt) (i32.const 20)) (then
      (memory.copy (i32.const 131076) (i32.const 131332) (i32.const 4864))
      (local.set $sl (i32.const 135940))
    ) (else
      (local.set $sl (i32.add (i32.const 131076)
                              (i32.mul (local.get $cnt) (i32.const 256))))
      (i32.store (i32.const 131072) (i32.add (local.get $cnt) (i32.const 1)))
    ))
    (i32.store (local.get $sl) (local.get $al))
    (memory.copy (i32.add (local.get $sl) (i32.const 4)) (local.get $bp) (local.get $al))
    (memory.copy (i32.const 0) (i32.const 200212) (i32.const 107))
    i32.const 107
  )

  (func $msgs (result i32)
    (local $p i32)(local $cnt i32)(local $i i32)
    (local $sl i32)(local $ml i32)(local $mp i32)
    (local $j i32)(local $b i32)
    (local.set $p (call $cp (i32.const 0)
      (i32.const 200110) (i32.const 102)))
    (local.set $p (call $cp (local.get $p)
      (i32.const 200504) (i32.const 13)))
    (local.set $cnt (i32.load (i32.const 131072)))
    (block $ob (loop $ol
      (br_if $ob (i32.ge_u (local.get $i) (local.get $cnt)))
      (local.set $sl (i32.add (i32.const 131076)
                              (i32.mul (local.get $i) (i32.const 256))))
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
      (i32.const 200517) (i32.const 2)))
    (local.get $p)
  )

  (data (i32.const 200110) "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n")
  (data (i32.const 200212) "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n")
  (data (i32.const 200319) "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nNot Found")
  (data (i32.const 200399) "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\nBad Request")
  (data (i32.const 200483) "handle_request called")
  (data (i32.const 200504) "{\"messages\":[")
  (data (i32.const 200517) "]}")
  (data (i32.const 200520) "/api/messages")
  (data (i32.const 200533) "/post")
)
