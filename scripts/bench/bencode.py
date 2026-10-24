#!/usr/bin/env python3
"""Bencode encoder and torrent helpers shared by the bench generators."""

from __future__ import annotations

import hashlib


def encode(value) -> bytes:
    """Encode a Python value as bencode. Dict keys are sorted, as required."""
    if isinstance(value, bool):
        return b"i1e" if value else b"i0e"
    if isinstance(value, int):
        return b"i" + str(value).encode() + b"e"
    if isinstance(value, str):
        return encode(value.encode())
    if isinstance(value, bytes):
        return str(len(value)).encode() + b":" + value
    if isinstance(value, (list, tuple)):
        return b"l" + b"".join(encode(item) for item in value) + b"e"
    if isinstance(value, dict):
        items = sorted(
            ((key.encode() if isinstance(key, str) else key), item)
            for key, item in value.items()
        )
        return b"d" + b"".join(encode(key) + encode(item) for key, item in items) + b"e"
    raise TypeError(f"cannot bencode {type(value)!r}")


def sha1(data: bytes) -> bytes:
    return hashlib.sha1(data).digest()


def info_dict(name: str, length: int, piece_length: int, pieces: bytes) -> dict:
    return {
        b"length": length,
        b"name": name.encode(),
        b"piece length": piece_length,
        b"pieces": pieces,
    }


def torrent_bytes(name: str, length: int, piece_length: int, pieces: bytes) -> tuple[bytes, str]:
    """Return (torrent bencode, uppercase info hash hex)."""
    info = info_dict(name, length, piece_length, pieces)
    info_hash = sha1(encode(info)).hex().upper()
    # No announce key: benchmarks must not generate tracker traffic.
    return encode({b"info": info}), info_hash
