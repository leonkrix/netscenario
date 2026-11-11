//! The one place with `unsafe`: a packet socket bound to an interface, which does not receive what it sends itself.
//!
//! `nix` 0.31 cannot make the address that `bind` and `send` on a packet socket need (the field of `LinkAddr` is private and
//! there is no constructor), and neither `nix` nor `rustix` knows `PACKET_IGNORE_OUTGOING`. So these two calls are made here.
#![allow(unsafe_code)]

use std::os::fd::{AsRawFd, OwnedFd};

use nix::sys::socket::{setsockopt, socket, sockopt, AddressFamily, SockFlag, SockProtocol, SockType};

const PACKET_IGNORE_OUTGOING: libc::c_int = 23;
const PACKET_VNET_HDR: libc::c_int = 15;

pub fn open_packet(interface: &str, vnet_hdr: bool) -> OwnedFd {
    let fd = socket(
        AddressFamily::Packet,
        SockType::Raw,
        SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC,
        SockProtocol::EthAll,
    )
    .unwrap_or_else(|e| panic!("socket(AF_PACKET): {e}"));
    let index = nix::net::if_::if_nametoindex(interface).unwrap_or_else(|e| panic!("no interface {interface}: {e}"));
    // SAFETY: `sll` is a plain C struct, filled completely before the call, and the length is its size.
    let bound = unsafe {
        let mut sll: libc::sockaddr_ll = std::mem::zeroed();
        sll.sll_family = libc::AF_PACKET as u16;
        sll.sll_protocol = (libc::ETH_P_ALL as u16).to_be();
        sll.sll_ifindex = index as i32;
        libc::bind(fd.as_raw_fd(), (&raw const sll).cast(), std::mem::size_of::<libc::sockaddr_ll>() as u32)
    };
    assert!(bound == 0, "bind {interface}: {}", std::io::Error::last_os_error());
    let one: libc::c_int = 1;
    // SAFETY: a pointer to a live `c_int` and its size.
    let set = unsafe {
        libc::setsockopt(fd.as_raw_fd(), libc::SOL_PACKET, PACKET_IGNORE_OUTGOING, (&raw const one).cast(), 4)
    };
    assert!(set == 0, "PACKET_IGNORE_OUTGOING: {}", std::io::Error::last_os_error());
    if vnet_hdr {
        // Every frame is read and written with a 10 byte `virtio_net_hdr` in front, which keeps an unfinished checksum unfinished
        // (the sender left it to the device) so that the interface that receives the frame finishes it, as it would from a real wire
        // SAFETY: a pointer to a live `c_int` and its size.
        let set = unsafe { libc::setsockopt(fd.as_raw_fd(), libc::SOL_PACKET, PACKET_VNET_HDR, (&raw const one).cast(), 4) };
        assert!(set == 0, "PACKET_VNET_HDR: {}", std::io::Error::last_os_error());
    }
    let _ = setsockopt(&fd, sockopt::RcvBuf, &(4 * 1024 * 1024));
    let _ = setsockopt(&fd, sockopt::SndBuf, &(4 * 1024 * 1024));
    fd
}
