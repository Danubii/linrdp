#!/usr/bin/env python3
"""Graphical loopback regression. Opens one window; server closes it after checks.

Run: python3 tools/vnc_pipeline_smoke.py target/release/fjern
No credentials or existing profiles are used. Requires a graphical session.
"""
import os
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib


def main():
    with tempfile.TemporaryDirectory(prefix="fjern-pipeline-") as config, socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        listener.settimeout(15)
        env = dict(os.environ, XDG_CONFIG_HOME=config, FJERN_VNC_STATS="1")
        client = subprocess.Popen(
            [sys.argv[1], "vnc", "127.0.0.1", str(listener.getsockname()[1])],
            env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        try:
            with listener.accept()[0] as conn:
                conn.settimeout(10)

                def read(n):
                    data = b""
                    while len(data) < n:
                        chunk = conn.recv(n - len(data))
                        if not chunk:
                            raise EOFError("client disconnected before completing smoke test")
                        data += chunk
                    return data

                def update(rectangles):
                    conn.sendall(struct.pack(">BBH", 0, 0, len(rectangles)) + b"".join(rectangles))

                def rect(x, y, w, h, encoding, payload):
                    return struct.pack(">HHHHi", x, y, w, h, encoding) + payload

                conn.sendall(b"RFB 003.008\n")
                assert read(12) == b"RFB 003.008\n"
                conn.sendall(b"\x01\x01")
                assert read(1) == b"\x01"
                conn.sendall(bytes(4))
                assert read(1) == b"\x01"
                pf = struct.pack(">BBBBHHHBBBxxx", 32, 24, 0, 1, 255, 255, 255, 16, 8, 0)
                name = b"Fjern pipeline regression"
                conn.sendall(struct.pack(">HH", 1920, 1080) + pf + struct.pack(">I", len(name)) + name)
                requests = 0
                sizes = set()
                started = time.monotonic()
                compressor = zlib.compressobj()
                encodings = []
                while time.monotonic() - started < 10:
                    kind = read(1)[0]
                    if kind == 0:
                        read(19)
                    elif kind == 2:
                        count = struct.unpack(">BH", read(3))[1]
                        encodings = struct.unpack(">" + "i" * count, read(4 * count))
                    elif kind == 3:
                        request = read(9)
                        sizes.add(struct.unpack(">HH", request[5:9]))
                        requests += 1
                        if requests == 1:
                            update([rect(0, 0, 1920, 1080, 0, b"\x11\x22\x33\x00" * (1920 * 1080))])
                        elif requests == 2:
                            update([rect(0, 16, 1920, 1064, 1, struct.pack(">HH", 0, 0))])
                        elif requests in (3, 4):
                            tiles = b"\x01\x55\x66\x77" * (30 * 17)
                            packed = compressor.compress(tiles) + compressor.flush(zlib.Z_SYNC_FLUSH)
                            update([rect(0, 0, 1920, 1080, 16, struct.pack(">I", len(packed)) + packed)])
                        elif requests == 5:
                            screens = b"\x01\x00\x00\x00" + struct.pack(">IHHHHI", 1, 0, 0, 1280, 720, 0)
                            update([rect(0, 0, 1280, 720, -308, screens),
                                    rect(0, 0, 1280, 720, 0, b"\x88\x99\xaa\x00" * (1280 * 720))])
                        else:
                            update([])
                        if (1280, 720) in sizes and time.monotonic() - started > 3:
                            break
                    elif kind == 4:
                        read(7)
                    elif kind == 5:
                        read(5)
                    elif kind == 6:
                        length = struct.unpack(">I", read(7)[3:])[0]
                        assert length <= 1024 * 1024
                        read(length)
                    elif kind == 251:
                        read(23)
                    else:
                        raise AssertionError(f"unexpected message {kind}")
                assert {0, 1, 16, -308}.issubset(encodings)
                assert (1280, 720) in sizes, "ExtendedDesktopSize did not update refresh dimensions"
            _, stderr = client.communicate(timeout=10)
            log = stderr.decode(errors="replace")
            assert "VNC stats:" in log, log
            assert "panicked" not in log and "image encoding" not in log, log
            # Deliberate server EOF is an expected connection error, not a UI close.
            assert client.returncode == 1, (client.returncode, log)
            print("PASS: Raw, CopyRect, persistent ZRLE, ExtendedDesktopSize, refreshed dimensions, server EOF")
            print("\n".join(line for line in log.splitlines() if line.startswith("VNC stats:")))
        finally:
            if client.poll() is None:
                client.terminate()
                client.communicate(timeout=10)


if __name__ == "__main__":
    main()
