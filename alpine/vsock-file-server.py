#!/usr/bin/env python3
"""vsock file I/O server for unikernel communication."""

import os
import socket
import struct
import sys

DATA_ROOT = "/data"
VSOCK_PORT = 1235

OP_READ = 0x01
OP_WRITE = 0x02
OP_LIST = 0x03

STATUS_OK = 0x00
STATUS_ERROR = 0x01


def log(msg):
    print(msg, file=sys.stderr, flush=True)


def recv_exactly(conn, n):
    buf = b""
    while len(buf) < n:
        chunk = conn.recv(n - len(buf))
        if not chunk:
            raise ConnectionError("connection closed while reading")
        buf += chunk
    return buf


def send_response(conn, status, data=b""):
    header = struct.pack("<BI", status, len(data))
    conn.sendall(header + data)


def safe_path(path_str):
    full = os.path.realpath(os.path.join(DATA_ROOT, path_str))
    if not full.startswith(DATA_ROOT + "/") and full != DATA_ROOT:
        return None
    return full


def handle_connection(conn):
    try:
        header = recv_exactly(conn, 7)
        op, path_len, data_len = struct.unpack("<BHI", header)

        path_bytes = recv_exactly(conn, path_len)
        path_str = path_bytes.decode("utf-8")

        payload = recv_exactly(conn, data_len) if data_len > 0 else b""

        full_path = safe_path(path_str)
        if full_path is None:
            log(f"path traversal blocked: {path_str}")
            send_response(conn, STATUS_ERROR, b"path traversal denied")
            return

        if op == OP_READ:
            log(f"READ {full_path}")
            if not os.path.isfile(full_path):
                send_response(conn, STATUS_ERROR, b"file not found")
                return
            with open(full_path, "rb") as f:
                data = f.read()
            send_response(conn, STATUS_OK, data)

        elif op == OP_WRITE:
            log(f"WRITE {full_path} ({data_len} bytes)")
            os.makedirs(os.path.dirname(full_path), exist_ok=True)
            with open(full_path, "wb") as f:
                f.write(payload)
            send_response(conn, STATUS_OK)

        elif op == OP_LIST:
            log(f"LIST {full_path}")
            if not os.path.isdir(full_path):
                send_response(conn, STATUS_ERROR, b"not a directory")
                return
            entries = os.listdir(full_path)
            data = "\n".join(entries).encode("utf-8")
            send_response(conn, STATUS_OK, data)

        else:
            log(f"unknown op: {op:#x}")
            send_response(conn, STATUS_ERROR, b"unknown operation")

    except ConnectionError as e:
        log(f"connection error: {e}")
    except Exception as e:
        log(f"error: {e}")
        try:
            send_response(conn, STATUS_ERROR, str(e).encode("utf-8"))
        except Exception:
            pass


def main():
    sock = socket.socket(socket.AF_VSOCK, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind((socket.VMADDR_CID_ANY, VSOCK_PORT))
    sock.listen(4)
    log(f"vsock file server listening on port {VSOCK_PORT}")

    while True:
        conn, addr = sock.accept()
        log(f"connection from CID={addr[0]} port={addr[1]}")
        try:
            handle_connection(conn)
        finally:
            conn.close()


if __name__ == "__main__":
    main()
