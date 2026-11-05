//! `TCP_INFO` of a connection, asked of the kernel through `sock_diag` (D-086).
//!
//! `getsockopt(TCP_INFO)` has no safe wrapper in the libraries of the workspace, and the workspace has no `unsafe` (D-025).
//! The kernel gives the same structure through a netlink socket: a request for one connection, named by its two addresses,
//! is answered with the attribute `INET_DIAG_INFO`, which is `struct tcp_info`. The sampler can therefore sit in a thread of
//! its own and does not need the descriptor of the socket that it watches. Only the fields that are read are named below; the
//! structure has grown with the kernel, so a shorter answer from an older kernel gives zero for what it lacks, and the layout
//! of the fields that are read has not changed since they were added.

use std::io;
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::Duration;

use nix::sys::socket::{
    AddressFamily, MsgFlags, SockFlag, SockProtocol, SockType, recv, send, setsockopt, socket,
    sockopt::ReceiveTimeout,
};
use nix::sys::time::{TimeVal, TimeValLike};

const SOCK_DIAG_BY_FAMILY: u16 = 20;
const NLMSG_ERROR: u16 = 2;
const NLM_F_REQUEST: u16 = 1;
const INET_DIAG_INFO: u16 = 2;
const IPPROTO_TCP: u8 = 6;
const AF_INET: u8 = 2;
const AF_INET6: u8 = 10;
/// The extension bit that asks for the attribute `INET_DIAG_INFO`.
const EXT_INFO: u8 = 1 << (INET_DIAG_INFO - 1);
/// The length of the header of a netlink message.
const NLMSG_HDRLEN: usize = 16;
/// The length of `struct inet_diag_req_v2`.
const REQUEST_LEN: usize = 56;
/// The length of `struct inet_diag_msg`.
const MESSAGE_LEN: usize = 72;

/// How long the kernel may take to answer. It answers from memory, at once; this is a net for a kernel that does not.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);

/// What is read of `struct tcp_info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TcpInfo {
    /// `tcpi_state`: 1 is established.
    pub state: u8,
    /// `tcpi_snd_mss`: the size of a segment the sender uses, in bytes.
    pub snd_mss: u32,
    /// `tcpi_rtt`: the smoothed round trip time in microseconds.
    pub rtt_us: u32,
    /// `tcpi_snd_ssthresh`: the slow start threshold in segments.
    pub snd_ssthresh: u32,
    /// `tcpi_snd_cwnd`: the congestion window in segments.
    pub snd_cwnd: u32,
    /// `tcpi_total_retrans`: the segments retransmitted over the life of the connection.
    pub total_retrans: u32,
    /// `tcpi_bytes_acked`: the bytes acknowledged by the peer, as the kernel counts them: the SYN counts as one, so a connection
    /// that has sent nothing reads 1. Use the difference of two samples.
    pub bytes_acked: u64,
}

impl TcpInfo {
    /// Reads the fields from the attribute. Anything past its end reads as zero.
    #[must_use]
    pub fn from_bytes(info: &[u8]) -> Self {
        let u32_at = |at: usize| {
            info.get(at..at + 4)
                .map_or(0, |b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
        };
        let u64_at = |at: usize| {
            info.get(at..at + 8).map_or(0, |b| {
                u64::from_ne_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            })
        };
        Self {
            state: info.first().copied().unwrap_or(0),
            snd_mss: u32_at(16),
            rtt_us: u32_at(68),
            snd_ssthresh: u32_at(76),
            snd_cwnd: u32_at(80),
            total_retrans: u32_at(100),
            bytes_acked: u64_at(120),
        }
    }
}

/// A netlink socket of the family `sock_diag`, in the network namespace of the process that opens it.
#[derive(Debug)]
pub struct SockDiag {
    fd: OwnedFd,
    sequence: u32,
}

impl SockDiag {
    /// Opens the socket.
    ///
    /// # Errors
    ///
    /// The error of the system, for example on a kernel without `sock_diag`.
    pub fn open() -> io::Result<Self> {
        let fd = socket(
            AddressFamily::Netlink,
            SockType::Raw,
            SockFlag::SOCK_CLOEXEC,
            SockProtocol::NetlinkSockDiag,
        )
        .map_err(io::Error::from)?;
        let timeout =
            TimeVal::milliseconds(i64::try_from(ANSWER_TIMEOUT.as_millis()).unwrap_or(2_000));
        setsockopt(&fd, ReceiveTimeout, &timeout).map_err(io::Error::from)?;
        Ok(Self { fd, sequence: 0 })
    }

    /// The `TCP_INFO` of the connection from `local` to `peer`, as the socket at `local` sees it.
    ///
    /// # Errors
    ///
    /// `NotFound` if no such connection exists (it was closed), or the error of the system.
    pub fn tcp_info(&mut self, local: SocketAddr, peer: SocketAddr) -> io::Result<TcpInfo> {
        self.sequence = self.sequence.wrapping_add(1);
        let request = request(local, peer, self.sequence);
        send(self.fd.as_raw_fd(), &request, MsgFlags::empty()).map_err(io::Error::from)?;
        let mut buffer = vec![0_u8; 8192];
        let length =
            recv(self.fd.as_raw_fd(), &mut buffer, MsgFlags::empty()).map_err(io::Error::from)?;
        parse(&buffer[..length])
    }
}

/// The message that asks for one connection.
fn request(local: SocketAddr, peer: SocketAddr, sequence: u32) -> Vec<u8> {
    let mut message = Vec::with_capacity(NLMSG_HDRLEN + REQUEST_LEN);
    let total = u32::try_from(NLMSG_HDRLEN + REQUEST_LEN).unwrap_or(0);
    message.extend(total.to_ne_bytes());
    message.extend(SOCK_DIAG_BY_FAMILY.to_ne_bytes());
    message.extend(NLM_F_REQUEST.to_ne_bytes());
    message.extend(sequence.to_ne_bytes());
    message.extend(0_u32.to_ne_bytes());
    // struct inet_diag_req_v2
    message.push(if local.is_ipv4() { AF_INET } else { AF_INET6 });
    message.push(IPPROTO_TCP);
    message.push(EXT_INFO);
    message.push(0);
    message.extend(u32::MAX.to_ne_bytes());
    // struct inet_diag_sockid: the ports and the addresses in network order, the interface (any), the cookie (none)
    message.extend(local.port().to_be_bytes());
    message.extend(peer.port().to_be_bytes());
    message.extend(sixteen(local));
    message.extend(sixteen(peer));
    message.extend(0_u32.to_ne_bytes());
    message.extend(u32::MAX.to_ne_bytes());
    message.extend(u32::MAX.to_ne_bytes());
    message
}

/// An address as the 16 bytes of the structure: an IPv4 address in the first four.
fn sixteen(address: SocketAddr) -> [u8; 16] {
    let mut bytes = [0_u8; 16];
    match address {
        SocketAddr::V4(v4) => bytes[..4].copy_from_slice(&v4.ip().octets()),
        SocketAddr::V6(v6) => bytes.copy_from_slice(&v6.ip().octets()),
    }
    bytes
}

/// Finds `tcp_info` in the answer, or the error that the kernel sent.
fn parse(answer: &[u8]) -> io::Result<TcpInfo> {
    let bad = |what: &str| io::Error::new(io::ErrorKind::InvalidData, what.to_owned());
    let mut at = 0;
    while at + NLMSG_HDRLEN <= answer.len() {
        let length =
            u32::from_ne_bytes([answer[at], answer[at + 1], answer[at + 2], answer[at + 3]])
                as usize;
        let kind = u16::from_ne_bytes([answer[at + 4], answer[at + 5]]);
        if length < NLMSG_HDRLEN || at + length > answer.len() {
            return Err(bad("a netlink message that does not fit its answer"));
        }
        let body = &answer[at + NLMSG_HDRLEN..at + length];
        if kind == NLMSG_ERROR {
            let code = body
                .get(..4)
                .map_or(0, |b| i32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
            return Err(if code == 0 {
                bad("an empty answer")
            } else {
                io::Error::from_raw_os_error(-code)
            });
        }
        if kind == SOCK_DIAG_BY_FAMILY {
            let mut attributes = body.get(MESSAGE_LEN..).unwrap_or_default();
            while attributes.len() >= 4 {
                let size = usize::from(u16::from_ne_bytes([attributes[0], attributes[1]]));
                let attribute = u16::from_ne_bytes([attributes[2], attributes[3]]);
                if size < 4 || size > attributes.len() {
                    break;
                }
                if attribute == INET_DIAG_INFO {
                    return Ok(TcpInfo::from_bytes(&attributes[4..size]));
                }
                let step = size.next_multiple_of(4).min(attributes.len());
                attributes = &attributes[step..];
            }
            return Err(bad("the answer has no tcp_info"));
        }
        at += length.next_multiple_of(4);
    }
    Err(bad("no message in the answer"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};

    fn connected() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    fn the_fields_are_read_from_their_offsets() {
        let mut info = vec![0_u8; 200];
        info[0] = 1;
        let put32 = |info: &mut Vec<u8>, at: usize, value: u32| {
            info[at..at + 4].copy_from_slice(&value.to_ne_bytes());
        };
        put32(&mut info, 16, 1448);
        put32(&mut info, 68, 40_123);
        put32(&mut info, 76, 77);
        put32(&mut info, 80, 10);
        put32(&mut info, 100, 3);
        info[120..128].copy_from_slice(&123_456_u64.to_ne_bytes());
        assert_eq!(
            TcpInfo::from_bytes(&info),
            TcpInfo {
                state: 1,
                snd_mss: 1448,
                rtt_us: 40_123,
                snd_ssthresh: 77,
                snd_cwnd: 10,
                total_retrans: 3,
                bytes_acked: 123_456,
            }
        );
        // An answer from an older kernel is shorter: what it lacks reads as zero
        let short = TcpInfo::from_bytes(&info[..72]);
        assert_eq!(
            (short.rtt_us, short.snd_cwnd, short.bytes_acked),
            (40_123, 0, 0)
        );
    }

    /// The kernel answers for a real connection: it is established, it has a window, and the bytes that the peer read have
    /// been acknowledged.
    #[test]
    fn the_kernel_tells_the_state_of_a_connection() {
        let (mut client, mut server) = connected();
        let mut diag = SockDiag::open().expect("sock_diag is not available");
        let (local, peer) = (client.local_addr().unwrap(), client.peer_addr().unwrap());
        let before = diag.tcp_info(local, peer).unwrap();
        assert_eq!(before.state, 1, "{before:?}");
        assert!(before.snd_cwnd > 0 && before.snd_mss > 0, "{before:?}");
        client.write_all(&vec![7_u8; 100_000]).unwrap();
        let mut got = vec![0_u8; 100_000];
        server.read_exact(&mut got).unwrap();
        // The acknowledgement follows the read by a moment
        let mut after = before;
        for _ in 0..100 {
            after = diag.tcp_info(local, peer).unwrap();
            if after.bytes_acked >= before.bytes_acked + 100_000 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            after.bytes_acked >= before.bytes_acked + 100_000,
            "{before:?} then {after:?}"
        );
    }

    /// Canary: a connection that does not exist is `NotFound`, not a sample of zeros.
    #[test]
    fn a_connection_that_does_not_exist_is_not_found() {
        let (client, server) = connected();
        let (local, peer) = (client.local_addr().unwrap(), client.peer_addr().unwrap());
        drop(client);
        drop(server);
        let mut diag = SockDiag::open().expect("sock_diag is not available");
        // The pair of addresses is gone, or in TIME_WAIT: either way it is not an established connection
        let outcome = diag.tcp_info(local, peer);
        assert!(
            outcome.as_ref().is_err() || outcome.as_ref().is_ok_and(|info| info.state != 1),
            "{outcome:?}"
        );
        let nowhere: SocketAddr = "127.0.0.1:9".parse().unwrap();
        let error = diag.tcp_info(nowhere, nowhere).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound, "{error}");
    }

    #[test]
    fn an_answer_that_is_cut_is_an_error() {
        let mut message = request(
            "127.0.0.1:1".parse().unwrap(),
            "127.0.0.1:2".parse().unwrap(),
            1,
        );
        // The length says more than there is
        message.truncate(40);
        assert!(parse(&message).is_err());
        assert!(parse(&[]).is_err());
    }
}
