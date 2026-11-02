"""A UDP echo server: every datagram it receives goes back to where it came from."""

import socket
import sys

address, port = sys.argv[1], int(sys.argv[2])
server = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
server.bind((address, port))
while True:
    data, sender = server.recvfrom(2048)
    server.sendto(data, sender)
