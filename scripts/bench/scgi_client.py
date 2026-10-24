#!/usr/bin/env python3
"""Minimal SCGI + XML-RPC client for a running rstorrent daemon.

Stdlib only, so the benchmark harness runs on any host with Python 3.8+.

Wire format: netstring-framed SCGI request, HTTP-ish response with a
Content-Length header (see crates/rt-scgi). XML-RPC is used because the
in-tree integration tests already exercise that dialect.

Usage:
    python3 scgi_client.py /tmp/rt.sock system.client_version
    python3 scgi_client.py /tmp/rt.sock d.hash <40-hex-info-hash>
"""

from __future__ import annotations

import socket
import sys
import xml.etree.ElementTree as ET
from xml.sax.saxutils import escape

DEFAULT_TIMEOUT = 30.0


def encode_frame(headers, body: bytes) -> bytes:
    """Encode one SCGI request: <len>:key\\0value\\0...,<body>."""
    pairs = b"".join(
        f"{key}".encode() + b"\x00" + f"{value}".encode() + b"\x00"
        for key, value in headers.items()
    )
    return str(len(pairs)).encode() + b":" + pairs + b"," + body


def xmlrpc_body(method: str, params=()) -> bytes:
    """Build an XML-RPC methodCall body. Params are sent as strings."""
    rendered = "".join(
        f"<param><value><string>{escape(str(param))}</string></value></param>"
        for param in params
    )
    body = (
        '<?xml version="1.0"?>'
        f"<methodCall><methodName>{escape(method)}</methodName>"
        f"<params>{rendered}</params></methodCall>"
    )
    return body.encode()


def _content_length(header: bytes) -> int:
    for line in header.split(b"\r\n"):
        name, _, value = line.partition(b":")
        if name.strip().lower() == b"content-length":
            return int(value.strip())
    raise RuntimeError("SCGI response missing Content-Length")


class Connection:
    """A keep-alive SCGI connection; one frame at a time."""

    def __init__(self, sock_path: str, timeout: float = DEFAULT_TIMEOUT):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(timeout)
        self.sock.connect(sock_path)
        self.buffer = b""

    def _fill(self, minimum: int) -> None:
        while len(self.buffer) < minimum:
            chunk = self.sock.recv(65536)
            if not chunk:
                raise ConnectionError("SCGI connection closed mid-response")
            self.buffer += chunk

    def call(self, method: str, params=()):
        body = xmlrpc_body(method, params)
        self.sock.sendall(encode_frame({"CONTENT_LENGTH": len(body), "SCGI": 1}, body))

        while b"\r\n\r\n" not in self.buffer:
            self._fill(len(self.buffer) + 1)
        header, _, rest = self.buffer.partition(b"\r\n\r\n")
        expected = _content_length(header)

        self.buffer = rest
        self._fill(expected)
        payload = self.buffer[:expected]
        self.buffer = self.buffer[expected:]
        return header, payload

    def close(self) -> None:
        self.sock.close()

    def __enter__(self):
        return self

    def __exit__(self, *_exc):
        self.close()


def call_raw(sock_path: str, method: str, params=(), timeout=DEFAULT_TIMEOUT):
    """Perform one XML-RPC call and return (header, body) bytes."""
    with Connection(sock_path, timeout) as connection:
        return connection.call(method, params)


def call(sock_path: str, method: str, params=(), timeout=DEFAULT_TIMEOUT) -> bytes:
    """Perform one XML-RPC call and return the response body."""
    return call_raw(sock_path, method, params, timeout)[1]


def parse_values(body: bytes):
    """Return the decoded scalar values in an XML-RPC response.

    Handles the element names rstorrent emits (<string>, <int>, <i8>,
    <double>, <boolean>) and flattens them in document order.
    """
    try:
        root = ET.fromstring(body)
    except ET.ParseError:
        return []

    values = []
    for element in root.iter():
        tag = element.tag.rsplit("}", 1)[-1]
        if element.text is None:
            continue
        if tag in ("string", "i8", "int", "double", "boolean", "dateTime.iso8601"):
            values.append(element.text)
    return values


def first_value(body: bytes) -> str | None:
    values = parse_values(body)
    return values[0] if values else None


def is_fault(body: bytes) -> bool:
    return b"<fault>" in body


def find_hashes(body: bytes):
    """All 40-character hex strings in a response, in order.

    Used to count rows in a `d.multicall2` result without a full XML-RPC
    array decoder.
    """
    import re

    return re.findall(rb"[0-9A-Fa-f]{40}", body)


def main(argv) -> int:
    if len(argv) < 3:
        print(__doc__.strip())
        return 2
    sock, method, *params = argv[1:]
    header, body = call_raw(sock, method, params)
    status = next(
        (line for line in header.decode(errors="replace").splitlines() if line.lower().startswith("status")),
        "",
    )
    print(status)
    print(body.decode(errors="replace"))
    return 0 if not is_fault(body) else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
