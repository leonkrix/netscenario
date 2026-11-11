#!/usr/bin/env python3
"""Switch off transmit checksum offload (and with it TSO and GSO) of interfaces, with the ioctl `ethtool` uses.

  ethtool_off.py IFACE...

Only for the spike: `ethtool` is not installed on every machine, and the question of S21 is whether frames that leave a node with
a finished checksum pass a packet engine (the engine does not finish a checksum that the sender left to the device)."""
import array
import fcntl
import socket
import struct
import sys

SIOCETHTOOL = 0x8946
ETHTOOL_STXCSUM = 0x00000017
ETHTOOL_STSO = 0x0000001F
ETHTOOL_SGSO = 0x00000024
ETHTOOL_SGRO = 0x0000002C


def set_value(sock, name, command, value):
    buffer = array.array("B", struct.pack("II", command, value))
    address, _ = buffer.buffer_info()
    request = struct.pack("16sP", name.encode(), address)
    fcntl.ioctl(sock.fileno(), SIOCETHTOOL, request)


sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
for name in sys.argv[1:]:
    for command in (ETHTOOL_SGSO, ETHTOOL_STSO, ETHTOOL_SGRO, ETHTOOL_STXCSUM):
        set_value(sock, name, command, 0)
print("offloads off:", " ".join(sys.argv[1:]))
