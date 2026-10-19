#!/usr/bin/env python3
import os
import sys
import time
import socket
import subprocess
import xmlrpc.client
from pathlib import Path

def scgi_call(host, port, method_name, *params):
    xml_payload = xmlrpc.client.dumps(params, methodname=method_name)
    headers = [
        (b"CONTENT_LENGTH", str(len(xml_payload)).encode("ascii")),
        (b"SCGI", b"1"),
    ]
    netstring = b"".join(k + b"\x00" + v + b"\x00" for k, v in headers)
    scgi_frame = f"{len(netstring)}:".encode("ascii") + netstring + b"," + xml_payload.encode("utf-8")

    with socket.create_connection((host, port), timeout=10) as s:
        s.sendall(scgi_frame)
        response = b""
        while True:
            chunk = s.recv(4096)
            if not chunk:
                break
            response += chunk

    header_end = response.find(b"\r\n\r\n")
    if header_end == -1:
        header_end = response.find(b"\n\n")
        body = response[header_end + 2:]
    else:
        body = response[header_end + 4:]
    
    val, _ = xmlrpc.client.loads(body)
    return val[0]

def main():
    root_dir = Path("/home/chmuri/git/rtorrent")
    test_dir = root_dir / "test_run"
    download_dir = test_dir / "download"
    session_dir = test_dir / "session"
    torrent_file = root_dir / "ubuntu-26.04.1-desktop-amd64.iso.torrent"

    download_dir.mkdir(parents=True, exist_ok=True)
    session_dir.mkdir(parents=True, exist_ok=True)

    scgi_port = 5000
    p2p_port = 50123

    print(f"=== Krok 1: Uruchamianie lokalnego rTorrenta (port SCGI: {scgi_port}, P2P: {p2p_port}) ===")
    rtorrent_bin = root_dir / "target" / "release" / "rtorrent"
    
    cmd = [
        str(rtorrent_bin),
        "--daemon",
        "-n", # no rc
        "-d", str(download_dir),
        "-s", str(session_dir),
        "-p", f"{p2p_port}-{p2p_port}",
        "-O", f"network.scgi.open_port = 127.0.0.1:{scgi_port}",
    ]

    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    time.sleep(1.0) # Dajmy chwilę na start serwera

    try:
        print("=== Krok 2: Test połączenia XML-RPC przez SCGI ===")
        ver = scgi_call("127.0.0.1", scgi_port, "system.client_version")
        print(f"-> system.client_version zwróciło: {ver}")
        assert ver == "0.10.0", f"Nieoczekiwana wersja: {ver}"

        print("=== Krok 3: Sprawdzenie pustej listy pobierań ===")
        dls = scgi_call("127.0.0.1", scgi_port, "download_list")
        print(f"-> Początkowa download_list: {dls}")

        print(f"=== Krok 4: Wczytanie i start torrenta z pliku: {torrent_file.name} ===")
        res_hash = scgi_call("127.0.0.1", scgi_port, "load.start", str(torrent_file.resolve()))
        print(f"-> load.start zwróciło hash: {res_hash}")

        time.sleep(1.0)

        print("=== Krok 5: Weryfikacja obecności torrenta w systemie ===")
        dls = scgi_call("127.0.0.1", scgi_port, "download_list")
        print(f"-> download_list po dodaniu: {dls}")
        assert res_hash in dls, f"Hash {res_hash} nie znajduje się w download_list!"

        name = scgi_call("127.0.0.1", scgi_port, "d.name", res_hash)
        print(f"-> d.name: {name}")
        assert "ubuntu" in name.lower()

        size = scgi_call("127.0.0.1", scgi_port, "d.size_bytes", res_hash)
        print(f"-> d.size_bytes: {size} ({size / (1024**3):.2f} GB)")

        active = scgi_call("127.0.0.1", scgi_port, "d.is_active", res_hash)
        print(f"-> d.is_active: {active}")

        print("=== Krok 6: Test multicall (dokładnie tak jak pobiera dane Flood) ===")
        mc_res = scgi_call(
            "127.0.0.1",
            scgi_port,
            "d.multicall2",
            "main",
            "d.hash=",
            "d.name=",
            "d.size_bytes=",
            "d.is_active=",
            "d.bytes_done=",
            "d.down.rate=",
            "d.up.rate=",
        )
        print(f"-> Wynik d.multicall2 (Flood view): {mc_res}")

        print("=== Krok 7: Monitorowanie stanu pobierania przez kolejne sekundy ===")
        for i in range(5):
            time.sleep(1.0)
            status = scgi_call(
                "127.0.0.1",
                scgi_port,
                "d.multicall2",
                "main",
                "d.bytes_done=",
                "d.down.rate=",
                "d.up.rate=",
            )
            print(f"[{i+1}s] bytes_done: {status[0][0]}, down_rate: {status[0][1]} B/s, up_rate: {status[0][2]} B/s")

        print("=== Wszystkie testy API i pobierania zakończone pełnym sukcesem! ===")

    finally:
        print("Zamykanie testowej instancji rtorrent...")
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()

if __name__ == "__main__":
    main()
