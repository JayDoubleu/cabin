#!/usr/bin/env python3
"""Checks that a Cabin guest really answers the RDP protocol.

gnome-remote-desktop requires NLA, so a plain TLS request is refused with
HYBRID_REQUIRED_BY_SERVER. Both results are reported, because seeing the
refusal confirms the server is a real RDP server and not just an open port.
"""
import socket
import struct
import sys

NAMES = {0: "RDP (legacy, no TLS)", 1: "TLS", 2: "CredSSP/NLA",
         4: "CredSSP + Early User Auth", 8: "RDSTLS"}
FAIL = {1: "SSL_REQUIRED_BY_SERVER", 2: "SSL_NOT_ALLOWED_BY_SERVER",
        3: "SSL_CERT_NOT_ON_SERVER", 4: "INCONSISTENT_FLAGS",
        5: "HYBRID_REQUIRED_BY_SERVER", 6: "SSL_WITH_USER_AUTH_REQUIRED"}


def probe(port, requested, label):
    # X.224 Connection Request: LI | CR | DST-REF | SRC-REF | CLASS | NEG_REQ
    neg = struct.pack("<BBHI", 0x01, 0x00, 0x0008, requested)
    x224 = bytes([6 + len(neg), 0xE0, 0, 0, 0, 0, 0]) + neg
    tpkt = struct.pack(">BBH", 3, 0, 4 + len(x224)) + x224
    with socket.create_connection(("127.0.0.1", port), timeout=10) as s:
        s.sendall(tpkt)
        resp = s.recv(4096)
    if len(resp) >= 19 and resp[5] == 0xD0:
        kind, value = resp[11], struct.unpack("<I", resp[15:19])[0]
        if kind == 0x02:
            print(f"  {label}: negotiated {NAMES.get(value, hex(value))}")
            return True
        if kind == 0x03:
            print(f"  {label}: refused, {FAIL.get(value, hex(value))}")
            return False
    print(f"  {label}: no Connection Confirm ({resp.hex()})")
    return False


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 13389
    print(f"probing 127.0.0.1:{port}")
    try:
        probe(port, 0x00000001, "TLS only ")
        ok = probe(port, 0x00000003, "TLS or NLA")
    except OSError as e:
        print(f"  cannot connect: {e}")
        return 1
    print("RESULT:", "the guest speaks RDP" if ok else "no usable RDP protocol")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
