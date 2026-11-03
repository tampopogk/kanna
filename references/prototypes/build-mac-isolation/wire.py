import json
import struct

MAX_HEADER = 64 * 1024


def send(sock, header, payload=b""):
    header = dict(header, size=len(payload))
    encoded = json.dumps(header, separators=(",", ":")).encode()
    sock.sendall(struct.pack(">I", len(encoded)) + encoded + payload)


def recv_exact(sock, n):
    chunks = []
    while n:
        chunk = sock.recv(min(n, 1 << 20))
        if not chunk:
            raise EOFError("peer closed")
        chunks.append(chunk)
        n -= len(chunk)
    return b"".join(chunks)


def recv(sock, max_payload):
    (length,) = struct.unpack(">I", recv_exact(sock, 4))
    if length > MAX_HEADER:
        raise ValueError("header too large")
    header = json.loads(recv_exact(sock, length))
    if not isinstance(header, dict):
        raise ValueError("header is not an object")
    size = header.get("size", 0)
    if not isinstance(size, int) or size < 0 or size > max_payload:
        raise ValueError("payload size out of bounds: %r" % (size,))
    return header, recv_exact(sock, size)
