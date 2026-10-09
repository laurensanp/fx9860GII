"""Minimal Protocol 7.00 client for CASIO fx-9860G-family calculators over USB (WinUSB/libusb).

Based on the protocol documentation at https://cahuteproject.org/topics/protocols/seven.html
"""
import sys
import time

import libusb_package
import usb.core
import usb.util

VID, PID = 0x07CF, 0x6101

T_CMD, T_DATA, T_SWAP, T_CHECK, T_ACK, T_NAK, T_TERM = 0x01, 0x02, 0x03, 0x05, 0x06, 0x15, 0x18
TYPE_NAMES = {1: "CMD", 2: "DATA", 3: "SWAP", 5: "CHECK", 6: "ACK", 0x15: "NAK", 0x18: "TERM"}


def checksum(body: bytes) -> bytes:
    return b"%02X" % ((-sum(body)) & 0xFF)


def pad(data: bytes) -> bytes:
    out = bytearray()
    for b in data:
        if b < 0x20:
            out += bytes((0x5C, b + 0x20))
        elif b == 0x5C:
            out += b"\\\\"
        else:
            out.append(b)
    return bytes(out)


def unpad(data: bytes) -> bytes:
    out, i = bytearray(), 0
    while i < len(data):
        if data[i] == 0x5C:
            nxt = data[i + 1]
            out.append(0x5C if nxt == 0x5C else nxt - 0x20)
            i += 2
        else:
            out.append(data[i])
            i += 1
    return bytes(out)


def build(t: int, st: int, data: bytes | None = None) -> bytes:
    body = b"%02X" % st
    if data is None:
        body += b"0"
    else:
        body += b"1" + b"%04X" % len(data) + data
    return bytes((t,)) + body + checksum(body)


def cmd_payload(*args: bytes, ow=0, dt=0, fs=0) -> bytes:
    args = list(args) + [b""] * (6 - len(args))
    return (b"%02X%02X%08X" % (ow, dt, fs)) + b"".join(b"%02X" % len(a) for a in args) + b"".join(args)


class Packet:
    def __init__(self, t, st, data):
        self.t, self.st, self.data = t, st, data

    def __repr__(self):
        d = "" if self.data is None else f" data[{len(self.data)}]={self.data[:60]!r}"
        return f"<{TYPE_NAMES.get(self.t, hex(self.t))} {self.st:02X}{d}>"


class Link:
    def __init__(self, verbose=False):
        self.verbose = verbose
        self.dev = usb.core.find(idVendor=VID, idProduct=PID, backend=libusb_package.get_libusb1_backend())
        if self.dev is None:
            raise SystemExit("Calculator not found (07CF:6101). Is it in LINK > RECV mode?")
        self.dev.set_configuration()
        intf = self.dev.get_active_configuration()[(0, 0)]
        self.ep_in = self.ep_out = None
        for ep in intf:
            if usb.util.endpoint_type(ep.bmAttributes) != usb.util.ENDPOINT_TYPE_BULK:
                continue
            if usb.util.endpoint_direction(ep.bEndpointAddress) == usb.util.ENDPOINT_IN:
                self.ep_in = ep.bEndpointAddress
            else:
                self.ep_out = ep.bEndpointAddress
        # Device enabling control flow (harmless on models that don't need it).
        try:
            self.dev.ctrl_transfer(0x41, 0x01, 0, 0, None, 300)
        except usb.core.USBError as e:
            if self.verbose:
                print("enable ctrl transfer:", e)
        self.buf = bytearray()
        self.stalls = 0

    def _read(self, n, timeout):
        deadline = time.monotonic() + timeout
        while len(self.buf) < n:
            left = deadline - time.monotonic()
            if left <= 0:
                raise TimeoutError(f"timeout waiting for {n} bytes (have {bytes(self.buf)!r})")
            try:
                self.buf += self.dev.read(self.ep_in, 4096, int(max(left, 0.05) * 1000))
            except usb.core.USBTimeoutError:
                pass
            except usb.core.USBError as e:
                if e.errno != 32:  # only recover from endpoint stalls ("pipe error")
                    raise
                self.stalls += 1
                if self.stalls > 50:
                    raise
                print(f"[usb stall #{self.stalls}, clearing]", file=sys.stderr)
                self.dev.clear_halt(self.ep_in)
                time.sleep(0.05)
        out = bytes(self.buf[:n])
        del self.buf[:n]
        return out

    def send(self, raw: bytes):
        if self.verbose:
            print(">>", raw[:80])
        self.dev.write(self.ep_out, raw, 5000)

    def recv(self, timeout=10.0) -> Packet:
        hdr = self._read(4, timeout)
        t, st, ex = hdr[0], int(hdr[1:3], 16), hdr[3:4]
        body = hdr[1:]
        data = None
        if ex == b"1":
            ds = self._read(4, timeout)
            data = self._read(int(ds, 16), timeout)
            body += ds + data
        cs = self._read(2, timeout)
        if self.verbose:
            print("<<", bytes((t,)) + body + cs)
        if cs != checksum(body):
            raise IOError(f"bad checksum {cs!r} != {checksum(body)!r}")
        return Packet(t, st, data)

    def exchange(self, raw: bytes, timeout=10.0) -> Packet:
        self.send(raw)
        return self.recv(timeout)

    # --- flows ---
    def start(self):
        p = self.exchange(build(T_CHECK, 0x00))
        if p.t != T_ACK:
            raise IOError(f"initial check not acknowledged: {p}")

    def terminate(self):
        try:
            self.exchange(build(T_TERM, 0x00), timeout=3)
        except Exception as e:  # calculator may just drop the link
            if self.verbose:
                print("terminate:", e)

    def device_info(self) -> dict:
        p = self.exchange(build(T_CMD, 0x01))
        if p.t != T_ACK or p.st != 0x02:
            raise IOError(f"unexpected reply to device info: {p}")
        d = unpad(p.data)

        def s(o, n):
            return d[o:o + n].split(b"\xff")[0].rstrip(b"\x00").decode("latin-1").strip()

        return {
            "hardware_id": s(0, 8),
            "cpu_id": s(8, 16),
            "preprog_rom_kb": s(24, 8),
            "flash_rom_kb": s(32, 8),
            "ram_kb": s(40, 8),
            "preprog_rom_version": s(48, 16),
            "bootcode_version": s(64, 16),
            "bootcode_offset": s(80, 8),
            "bootcode_size": s(88, 8),
            "os_version": s(96, 16),
            "os_offset": s(112, 8),
            "os_size": s(120, 8),
            "protocol": s(128, 4),
            "product_id": s(132, 16),
            "user_name": s(148, 16),
            "_raw_len": len(d),
        }


    # --- storage memory ---
    def _expect_ack(self, p, what):
        if p.t != T_ACK:
            raise IOError(f"{what}: calculator replied {p}")

    def _passive_loop(self, on_command):
        """After a roleswap: the calculator is active. Handle its commands until it swaps back."""
        while True:
            p = self.recv(timeout=30)
            if p.t == T_SWAP:
                return
            if p.t != T_CMD:
                raise IOError(f"unexpected packet while passive: {p}")
            on_command(p)

    def _receive_data(self, st, expected_size):
        out = bytearray()
        expect, t0 = 1, time.monotonic()
        while True:
            try:
                p = self.recv(timeout=30)
            except IOError as e:
                if "checksum" not in str(e):
                    raise
                print(f"[bad checksum on packet {expect}, asking resend]", file=sys.stderr)
                self.buf.clear()
                self.send(build(T_NAK, 0x01))
                continue
            if p.t != T_DATA or p.st != st:
                raise IOError(f"expected data packet {st:02X} #{expect}, got {p}")
            d = unpad(p.data)
            total, cur = int(d[0:4], 16), int(d[4:8], 16)
            if cur == expect:
                out += d[8:]
                expect += 1
            # (a repeated packet is ACKed again without being stored twice)
            self.send(build(T_ACK, 0x00))
            if cur % 256 == 0 or cur == total:
                rate = len(out) / max(time.monotonic() - t0, 1e-3) / 1024
                print(f"\r  {cur}/{total} packets, {len(out) // 1024} KiB, {rate:.1f} KiB/s",
                      end="", file=sys.stderr, flush=True)
            if cur == total:
                print(file=sys.stderr)
                break
        if len(out) != expected_size:
            raise IOError(f"received {len(out)} bytes, expected {expected_size}")
        return bytes(out)

    @staticmethod
    def parse_cmd(data: bytes):
        ow, dt, fs = int(data[0:2], 16), int(data[2:4], 16), int(data[4:12], 16)
        sizes = [int(data[12 + 2 * i:14 + 2 * i], 16) for i in range(6)]
        args, off = [], 24
        for n in sizes:
            args.append(data[off:off + n].decode("latin-1"))
            off += n
        return ow, dt, fs, args

    def list_files(self, device="fls0"):
        files = []
        self._expect_ack(self.exchange(build(T_CMD, 0x4D, cmd_payload(b"", b"", b"", b"", device.encode()))), "list")
        self.send(build(T_SWAP, 0x00))

        def on_cmd(p):
            if p.st == 0x4E:
                _, _, fs, a = self.parse_cmd(p.data)
                files.append((a[0], a[1], fs))
            self.send(build(T_ACK, 0x00))

        self._passive_loop(on_cmd)
        return files

    def capacity(self, device="fls0"):
        result = {}
        self._expect_ack(self.exchange(build(T_CMD, 0x4B, cmd_payload(b"", b"", b"", b"", device.encode()))), "capacity")
        self.send(build(T_SWAP, 0x00))

        def on_cmd(p):
            result["free"] = self.parse_cmd(p.data)[2]
            self.send(build(T_ACK, 0x00))

        self._passive_loop(on_cmd)
        return result.get("free")

    def get_file(self, name, directory="", device="fls0"):
        result = {}
        payload = cmd_payload(directory.encode(), name.encode(), b"", b"", device.encode())
        self._expect_ack(self.exchange(build(T_CMD, 0x44, payload)), f"request {name}")
        self.send(build(T_SWAP, 0x00))

        def on_cmd(p):
            if p.st != 0x45:
                raise IOError(f"unexpected command {p}")
            fs = self.parse_cmd(p.data)[2]
            self.send(build(T_ACK, 0x00))
            result["data"] = self._receive_data(0x45, fs)

        self._passive_loop(on_cmd)
        return result["data"]

    def send_file(self, name, data: bytes, directory="", device="fls0", progress=None):
        payload = cmd_payload(directory.encode(), name.encode(), b"", b"", device.encode(), ow=0x02, fs=len(data))
        p = self.exchange(build(T_CMD, 0x45, payload))
        if p.t == T_NAK and p.st == 0x02:  # overwrite confirmation requested
            p = self.exchange(build(T_ACK, 0x01))
        self._expect_ack(p, f"send {name}")
        chunks = [data[i:i + 256] for i in range(0, len(data), 256)] or [b""]
        for i, c in enumerate(chunks, 1):
            body = b"%04X%04X" % (len(chunks), i) + pad(c)
            self._expect_ack(self.exchange(build(T_DATA, 0x45, body), timeout=30), f"data {i}/{len(chunks)}")
            if progress:
                progress(i, len(chunks))

    def delete_file(self, name, directory="", device="fls0"):
        payload = cmd_payload(directory.encode(), name.encode(), b"", b"", device.encode())
        self._expect_ack(self.exchange(build(T_CMD, 0x46, payload)), f"delete {name}")

    def optimize(self, device="fls0"):
        self._expect_ack(self.exchange(build(T_CMD, 0x51, cmd_payload(b"", b"", b"", b"", device.encode())), timeout=120), "optimize")


def main(argv):
    import argparse
    import os

    ap = argparse.ArgumentParser(description="Protocol 7.00 tool for CASIO fx-9860G calculators")
    ap.add_argument("-v", "--verbose", action="store_true")
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("info")
    sub.add_parser("ls")
    g = sub.add_parser("get"); g.add_argument("name"); g.add_argument("-o", "--out"); g.add_argument("-d", "--dir", default="")
    s = sub.add_parser("put"); s.add_argument("file"); s.add_argument("--name"); s.add_argument("-d", "--dir", default="")
    pl = sub.add_parser("pull", help="download a file, then delete it from the calculator")
    pl.add_argument("name"); pl.add_argument("--outdir", default=".")
    r = sub.add_parser("rm"); r.add_argument("name"); r.add_argument("-d", "--dir", default="")
    sub.add_parser("optimize")
    pr = sub.add_parser("prep", help="optimize storage, send an add-in, list storage")
    pr.add_argument("file")
    a = ap.parse_args(argv)

    link = Link(verbose=a.verbose)
    link.start()
    try:
        if a.cmd == "info":
            for k, v in link.device_info().items():
                print(f"{k:22} {v}")
        elif a.cmd == "ls":
            for d, n, fs in link.list_files():
                print(f"{fs:>9}  {d + '/' if d else ''}{n}")
            print(f"free: {link.capacity()} bytes")
        elif a.cmd == "get":
            data = link.get_file(a.name, a.dir)
            out = a.out or a.name
            with open(out, "wb") as f:
                f.write(data)
            print(f"saved {len(data)} bytes to {out}")
        elif a.cmd == "put":
            data = open(a.file, "rb").read()
            name = a.name or os.path.basename(a.file)
            link.send_file(name, data, a.dir,
                           progress=lambda i, n: print(f"\r{i}/{n} packets", end="", flush=True))
            print(f"\nsent {name} ({len(data)} bytes)")
            for d, n, fs in link.list_files():
                print(f"{fs:>9}  {d + '/' if d else ''}{n}")
            print(f"free: {link.capacity()} bytes")
        elif a.cmd == "pull":
            files = link.list_files()
            if not any(n == a.name and not d for d, n, _ in files):
                print(f"{a.name} is not on the calculator. Files: {[n for _, n, _ in files]}")
                link.optimize()
                print(f"optimized storage; free: {link.capacity()} bytes")
                sys.exit(2)
            t0 = time.monotonic()
            data = link.get_file(a.name)
            os.makedirs(a.outdir, exist_ok=True)
            out = os.path.join(a.outdir, a.name)
            with open(out, "wb") as f:
                f.write(data)
            dt = time.monotonic() - t0
            s = sum(int.from_bytes(data[i:i + 4], "big") for i in range(0, len(data) - 3, 4)) & 0xFFFFFFFF
            print(f"saved {len(data)} bytes to {out} in {dt:.0f}s; word sum {s:08X}")
            link.delete_file(a.name)
            link.optimize()
            print(f"deleted {a.name} from calculator and optimized; free: {link.capacity()} bytes")
        elif a.cmd == "rm":
            link.delete_file(a.name, a.dir)
            print(f"deleted {a.name}")
        elif a.cmd == "prep":
            print(f"free before optimize: {link.capacity()} bytes")
            link.optimize()
            print(f"free after optimize:  {link.capacity()} bytes")
            data = open(a.file, "rb").read()
            link.send_file(os.path.basename(a.file), data)
            print(f"sent {os.path.basename(a.file)} ({len(data)} bytes)")
            for d, n, fs in link.list_files():
                print(f"{fs:>9}  {d + '/' if d else ''}{n}")
            print(f"free: {link.capacity()} bytes")
        elif a.cmd == "optimize":
            link.optimize()
            print("storage optimized")
    finally:
        link.terminate()


if __name__ == "__main__":
    main(sys.argv[1:])
