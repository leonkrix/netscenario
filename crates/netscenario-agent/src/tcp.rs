//! The TCP workloads: bulk transfers and requests with responses (WRK-04, WRK-05; D-086).
//!
//! A server (`tcp-serve`) in one node and a client (`tcp-bulk` or `tcp-request`) in another cooperate over ordinary TCP
//! connections of the kernel of the machine. The client says what it wants in a hello of 32 bytes; the server needs nothing
//! else on its command line. Both write a file of [`streams`](crate::streams) records.
//!
//! **A bulk connection** sends its payload in chunks, each with a header of four bytes that holds its length, and ends with a
//! chunk of length 0. The server answers the end with one byte and the client reads it: that moment is the completion of the
//! transfer, when the last byte has been read by the application at the far end, and the socket of the client is still open
//! when it takes its last sample of `TCP_INFO`. (A half-close would have told the server the end as well, but the socket
//! would be in `TIME_WAIT` by the time the client knew, and the kernel gives no `tcp_info` for it.) The header costs four
//! bytes in 65,536; the payload that is counted is the payload.
//!
//! **A request/response connection** sends a request of the same size every time and reads a response of the size named in the
//! hello. In a closed loop the next request follows the response; in an open loop requests are due at fixed moments and a
//! request that is due while the connection is busy waits, and its latency counts from the moment it was due (so a slow
//! server cannot hide behind the clients that waited for it).

use std::io::{self, BufWriter, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::clock::monotonic_ns;
use crate::events::{ServeSummary, TcpSummary};
use crate::sockdiag::SockDiag;
use crate::streams::{Kind, Record, STATUS_FAILED, STATUS_LAST, STATUS_OK, Writer};

/// The length of the hello.
pub const HELLO_LEN: usize = 32;

const HELLO_MAGIC: &[u8; 4] = b"NSTP";
const HELLO_VERSION: u8 = 1;

/// The payload of a chunk of a bulk transfer, at most. A chunk is sent whole, so this bounds how long a stop has to wait.
pub const MAX_CHUNK: u32 = 1 << 20;

/// The interval at which the server counts the bytes it read, when nothing else is said, in nanoseconds.
pub const DEFAULT_PROGRESS_INTERVAL_NS: u64 = 10_000_000;

/// The interval between two samples of `TCP_INFO`, when nothing else is said, in nanoseconds.
pub const DEFAULT_INFO_INTERVAL_NS: u64 = 100_000_000;

/// How long a bulk connection waits, after the end of its data, for the server to say that it read everything, in nanoseconds.
pub const DEFAULT_FINISH_TIMEOUT_NS: u64 = 120_000_000_000;

/// How long a request waits for its response, when nothing else is said, in nanoseconds.
pub const DEFAULT_REQUEST_TIMEOUT_NS: u64 = 30_000_000_000;

/// How long a wait on a socket lasts before the stop flag is looked at again.
const POLL: Duration = Duration::from_millis(20);

/// How long a connection of the server goes on without data after the stop, before it gives up.
const IDLE_AFTER_STOP: Duration = Duration::from_millis(500);

/// How long a client may take to say hello after it connected, in nanoseconds. The hello is the first thing a client sends, and
/// on a congested path it can be lost again and again: the retransmissions wait 0.2, 0.4, 0.8, 1.6 and 3.2 s, so a patience of
/// 5 s refused a connection that TCP would have delivered a second later, and the client saw it as a broken pipe (a connection
/// in one run in about a hundred, with sixteen of them in a queue of 30 packets). The server waits as long as a client waits
/// for the server's answer at the end.
pub const DEFAULT_HELLO_PATIENCE_NS: u64 = DEFAULT_FINISH_TIMEOUT_NS;

/// How long the server waits for the client to close, after it answered the end of the data.
const CLOSE_PATIENCE: Duration = Duration::from_secs(5);

/// The stack of a thread of a connection, in bytes. A connection needs little, and the agent runs under a limit on its address
/// space (SAF-02), where the 2 MiB that the standard library gives each thread add up to the whole limit with a few hundred
/// connections: a workload with `connections: 1000` has two threads for each.
const CONNECTION_STACK: usize = 256 * 1024;

/// Starts a thread of a connection in `scope`, with the small stack.
fn start<'scope, 'env, F, T>(
    scope: &'scope thread::Scope<'scope, 'env>,
    work: F,
) -> thread::ScopedJoinHandle<'scope, T>
where
    F: FnOnce() -> T + Send + 'scope,
    T: Send + 'scope,
{
    thread::Builder::new()
        .stack_size(CONNECTION_STACK)
        .spawn_scoped(scope, work)
        .unwrap_or_else(|error| panic!("cannot start a thread: {error}"))
}

/// What a connection is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Data in one direction, then the end.
    Bulk,
    /// Requests and responses.
    Requests,
}

/// What a client says first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hello {
    /// What the connection is for.
    pub mode: Mode,
    /// The number of the connection in its workload.
    pub connection: u32,
    /// The size of a request in bytes (requests only).
    pub request_bytes: u32,
    /// The size of a response in bytes (requests only).
    pub response_bytes: u32,
}

impl Hello {
    /// The 32 bytes: the magic `NSTP`, the version 1, the mode (1 bulk, 2 requests), two zero bytes, then the connection, the
    /// size of a request and the size of a response, each four bytes in network order, and twelve zero bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; HELLO_LEN] {
        let mut bytes = [0_u8; HELLO_LEN];
        bytes[0..4].copy_from_slice(HELLO_MAGIC);
        bytes[4] = HELLO_VERSION;
        bytes[5] = match self.mode {
            Mode::Bulk => 1,
            Mode::Requests => 2,
        };
        bytes[8..12].copy_from_slice(&self.connection.to_be_bytes());
        bytes[12..16].copy_from_slice(&self.request_bytes.to_be_bytes());
        bytes[16..20].copy_from_slice(&self.response_bytes.to_be_bytes());
        bytes
    }

    /// Reads a hello.
    ///
    /// # Errors
    ///
    /// Why it is not one: the magic, the version or the mode is wrong, a reserved byte is not zero, or a request has no size.
    pub fn from_bytes(bytes: &[u8; HELLO_LEN]) -> Result<Self, String> {
        if &bytes[0..4] != HELLO_MAGIC {
            return Err("it does not start with NSTP".to_owned());
        }
        if bytes[4] != HELLO_VERSION {
            return Err(format!("version {} is not read", bytes[4]));
        }
        if bytes[6] != 0 || bytes[7] != 0 || bytes[20..].iter().any(|b| *b != 0) {
            return Err("a reserved byte is not zero".to_owned());
        }
        let mode = match bytes[5] {
            1 => Mode::Bulk,
            2 => Mode::Requests,
            other => return Err(format!("the mode {other} does not exist")),
        };
        let number = |at: usize| {
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
        };
        let hello = Self {
            mode,
            connection: number(8),
            request_bytes: number(12),
            response_bytes: number(16),
        };
        if hello.mode == Mode::Requests && (hello.request_bytes == 0 || hello.response_bytes == 0) {
            return Err("a request and a response need at least one byte".to_owned());
        }
        Ok(hello)
    }
}

/// The file that the records go to, written from several threads.
struct Sink {
    writer: Mutex<Writer<BufWriter<Box<dyn Write + Send>>>>,
    failed: AtomicBool,
}

impl Sink {
    fn new(out: impl Write + Send + 'static, flow: u32) -> io::Result<Self> {
        let boxed: Box<dyn Write + Send> = Box::new(out);
        Ok(Self {
            writer: Mutex::new(Writer::new(BufWriter::with_capacity(1 << 16, boxed), flow)?),
            failed: AtomicBool::new(false),
        })
    }

    /// Writes a record. An error is remembered and does not stop the transfer: the run is told at the end.
    fn put(&self, record: &Record) {
        let mut writer = self
            .writer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if writer.write(record).is_err() {
            self.failed.store(true, Ordering::SeqCst);
        }
    }

    fn finish(self) -> io::Result<()> {
        let writer = self
            .writer
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        writer.finish()?;
        if self.failed.load(Ordering::SeqCst) {
            return Err(io::Error::other("a record could not be written"));
        }
        Ok(())
    }
}

fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}

/// How a wait for bytes ended.
#[derive(Debug, PartialEq, Eq)]
enum Fill {
    /// The buffer is full.
    Full,
    /// The peer closed before the first byte.
    Closed,
    /// The stop came and nothing more arrived.
    Stopped,
}

/// Reads until `buffer` is full. A peer that closes in the middle is an error. Without a byte for `patience` the wait fails, and
/// after the stop it ends once nothing has arrived for [`IDLE_AFTER_STOP`].
fn fill(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    stop: &AtomicBool,
    patience: Duration,
) -> io::Result<Fill> {
    let mut done = 0;
    let mut last = Instant::now();
    while done < buffer.len() {
        match stream.read(&mut buffer[done..]) {
            Ok(0) if done == 0 => return Ok(Fill::Closed),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => {
                done += n;
                last = Instant::now();
            }
            Err(error) if is_timeout(&error) => {
                if stop.load(Ordering::SeqCst) && last.elapsed() > IDLE_AFTER_STOP {
                    return Ok(Fill::Stopped);
                }
                if last.elapsed() > patience {
                    return Err(io::ErrorKind::TimedOut.into());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(Fill::Full)
}

/// Writes all of `data`, looking at the stop flag between the waits. `false` means the stop came first.
fn write_all(stream: &mut TcpStream, mut data: &[u8], stop: &AtomicBool) -> io::Result<bool> {
    while !data.is_empty() {
        match stream.write(data) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => data = &data[n..],
            Err(error) if is_timeout(&error) => {
                if stop.load(Ordering::SeqCst) {
                    return Ok(false);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

/// Asks for a congestion control on a socket and checks that the kernel took it.
///
/// # Errors
///
/// A sentence that names the congestion control and the reason, for example a module that the machine lacks.
pub fn set_congestion(stream: &TcpStream, name: &str) -> Result<(), String> {
    rustix::net::sockopt::set_tcp_congestion(stream, name)
        .map_err(|error| format!("the congestion control '{name}' cannot be used here: {error}"))?;
    let now = rustix::net::sockopt::tcp_congestion(stream)
        .map_err(|error| format!("cannot read back the congestion control: {error}"))?;
    if now.trim_end_matches('\0') == name {
        Ok(())
    } else {
        Err(format!(
            "the congestion control '{name}' was asked for and the socket has '{now}'"
        ))
    }
}

/// The congestion control a socket has.
fn congestion_of(stream: &TcpStream) -> Option<String> {
    rustix::net::sockopt::tcp_congestion(stream)
        .ok()
        .map(|name| name.trim_end_matches('\0').to_owned())
}

// ---------------------------------------------------------------------------------------------------------------------
// The server

/// What the server does.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// The address to listen on. Port 0 lets the system choose.
    pub bind: SocketAddr,
    /// The workload, written to the file and the summary.
    pub flow: u32,
    /// The width of the intervals in which the bytes read are counted, in nanoseconds.
    pub progress_interval_ns: u64,
    /// The congestion control for the responses of the server.
    pub congestion: Option<String>,
    /// Stop after this many nanoseconds even if nobody said stop. A safety net.
    pub max_time_ns: Option<u64>,
    /// How long a client may take to say hello after it connected, in nanoseconds ([`DEFAULT_HELLO_PATIENCE_NS`]). A connection
    /// that has not said it by then is refused.
    pub hello_patience_ns: u64,
}

/// A server that listens and does not yet serve.
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    config: ServeConfig,
}

/// What one connection of the server did.
#[derive(Debug, Default)]
struct Served {
    bytes: u64,
    requests: u64,
    first_ns: Option<u64>,
    last_ns: Option<u64>,
    refused: bool,
}

impl Server {
    /// Binds the listener.
    ///
    /// # Errors
    ///
    /// The error of the system if the address cannot be used, or a congestion control that the machine lacks.
    pub fn bind(config: &ServeConfig) -> io::Result<Self> {
        let listener = TcpListener::bind(config.bind)?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            config: config.clone(),
        })
    }

    /// The address that the server listens on, with the port the system chose.
    ///
    /// # Errors
    ///
    /// The error of the system.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serves connections until `stop` is set or the time is up, writing the bytes it read into `out`, and returns what it did.
    ///
    /// # Errors
    ///
    /// The error of the listener or of `out`.
    pub fn run(
        self,
        stop: &AtomicBool,
        out: impl Write + Send + 'static,
    ) -> io::Result<ServeSummary> {
        let sink = Sink::new(out, self.config.flow)?;
        let deadline = self
            .config
            .max_time_ns
            .map(|limit| monotonic_ns().saturating_add(limit));
        let interval = self.config.progress_interval_ns.max(1);
        let congestion = self.config.congestion.as_deref();
        let patience = Duration::from_nanos(self.config.hello_patience_ns);
        let mut summary = ServeSummary {
            flow: self.config.flow,
            connections: 0,
            refused: 0,
            bytes_received: 0,
            requests: 0,
            first_receive_ns: None,
            last_receive_ns: None,
        };
        let stopping = AtomicBool::new(false);
        let outcome: io::Result<()> = thread::scope(|scope| {
            let mut handles = Vec::new();
            loop {
                if stop.load(Ordering::SeqCst) || deadline.is_some_and(|d| monotonic_ns() >= d) {
                    stopping.store(true, Ordering::SeqCst);
                    break;
                }
                match self.listener.accept() {
                    Ok((stream, _)) => {
                        let (sink, stopping) = (&sink, &stopping);
                        handles.push(start(scope, move || {
                            serve(stream, stopping, sink, interval, congestion, patience)
                        }));
                    }
                    Err(error) if is_timeout(&error) => thread::sleep(Duration::from_millis(2)),
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                    Err(error) => {
                        stopping.store(true, Ordering::SeqCst);
                        return Err(error);
                    }
                }
            }
            for handle in handles {
                let served = handle.join().unwrap_or_default();
                summary.connections += 1;
                if served.refused {
                    summary.refused += 1;
                }
                summary.bytes_received += served.bytes;
                summary.requests += served.requests;
                summary.first_receive_ns = match (summary.first_receive_ns, served.first_ns) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
                summary.last_receive_ns = summary.last_receive_ns.max(served.last_ns);
            }
            Ok(())
        });
        outcome?;
        sink.finish()?;
        Ok(summary)
    }
}

/// One connection of the server, from the hello to the end.
fn serve(
    mut stream: TcpStream,
    stop: &AtomicBool,
    sink: &Sink,
    interval: u64,
    congestion: Option<&str>,
    patience: Duration,
) -> Served {
    let mut served = Served::default();
    if stream.set_nonblocking(false).is_err()
        || stream.set_read_timeout(Some(POLL)).is_err()
        || stream.set_write_timeout(Some(POLL)).is_err()
    {
        served.refused = true;
        return served;
    }
    let _ = stream.set_nodelay(true);
    let mut raw = [0_u8; HELLO_LEN];
    let hello = match fill(&mut stream, &mut raw, stop, patience) {
        Ok(Fill::Full) => Hello::from_bytes(&raw).ok(),
        _ => None,
    };
    let Some(hello) = hello else {
        served.refused = true;
        return served;
    };
    if let Some(name) = congestion {
        // A response that goes out under another control than asked for would be a result about something else
        if set_congestion(&stream, name).is_err() {
            served.refused = true;
            return served;
        }
    }
    let _ = match hello.mode {
        Mode::Bulk => serve_bulk(&mut stream, &hello, stop, sink, interval, &mut served),
        Mode::Requests => serve_requests(&mut stream, &hello, stop, &mut served),
    };
    served
}

/// The bytes that arrived in one interval.
struct Bucket {
    start: u64,
    bytes: u64,
    reads: u64,
}

fn flush(sink: &Sink, connection: u32, bucket: &mut Bucket) {
    if bucket.bytes > 0 {
        sink.put(&Record {
            kind: Kind::Progress,
            status: STATUS_OK,
            connection,
            t: bucket.start,
            a: bucket.bytes,
            b: bucket.reads,
            c: 0,
            d: 0,
        });
    }
    bucket.bytes = 0;
    bucket.reads = 0;
}

/// Reads the chunks of a bulk connection, counts what arrives, and answers the end with one byte.
fn serve_bulk(
    stream: &mut TcpStream,
    hello: &Hello,
    stop: &AtomicBool,
    sink: &Sink,
    interval: u64,
    served: &mut Served,
) -> io::Result<()> {
    let mut buffer = vec![0_u8; 256 * 1024];
    let mut header = [0_u8; 4];
    let mut header_have = 0_usize;
    let mut remaining = 0_u64;
    let mut bucket = Bucket {
        start: 0,
        bytes: 0,
        reads: 0,
    };
    let mut last = Instant::now();
    let mut ended = false;
    while !ended {
        match stream.read(&mut buffer) {
            Ok(0) => {
                flush(sink, hello.connection, &mut bucket);
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            Ok(n) => {
                last = Instant::now();
                let now = monotonic_ns();
                let start = now - now % interval;
                if start != bucket.start {
                    flush(sink, hello.connection, &mut bucket);
                    bucket.start = start;
                }
                served.first_ns.get_or_insert(now);
                served.last_ns = Some(now);
                let mut at = 0;
                let mut payload = 0_u64;
                while at < n {
                    if remaining == 0 {
                        header[header_have] = buffer[at];
                        header_have += 1;
                        at += 1;
                        if header_have == header.len() {
                            header_have = 0;
                            remaining = u64::from(u32::from_be_bytes(header));
                            if remaining == 0 {
                                ended = true;
                                break;
                            }
                        }
                    } else {
                        let take = remaining.min((n - at) as u64);
                        payload += take;
                        remaining -= take;
                        at += usize::try_from(take).unwrap_or(n - at);
                    }
                }
                bucket.bytes += payload;
                bucket.reads += 1;
                served.bytes += payload;
            }
            Err(error) if is_timeout(&error) => {
                if bucket.bytes > 0 && monotonic_ns() / interval * interval != bucket.start {
                    flush(sink, hello.connection, &mut bucket);
                }
                if stop.load(Ordering::SeqCst) && last.elapsed() > IDLE_AFTER_STOP {
                    flush(sink, hello.connection, &mut bucket);
                    return Ok(());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                flush(sink, hello.connection, &mut bucket);
                return Err(error);
            }
        }
    }
    flush(sink, hello.connection, &mut bucket);
    // The end is answered, then the client closes; the server waits for that and lets go
    write_all(stream, &[1], stop)?;
    let mut rest = [0_u8; 1];
    let _ = fill(stream, &mut rest, stop, CLOSE_PATIENCE);
    Ok(())
}

/// Answers each request with a response of the size named in the hello.
fn serve_requests(
    stream: &mut TcpStream,
    hello: &Hello,
    stop: &AtomicBool,
    served: &mut Served,
) -> io::Result<()> {
    let mut request = vec![0_u8; hello.request_bytes as usize];
    let response = vec![0_u8; hello.response_bytes as usize];
    loop {
        // A client that is between two requests may take a long time; one that stops in the middle of a request may not
        match fill(stream, &mut request, stop, Duration::from_secs(3600))? {
            Fill::Full => {}
            Fill::Closed | Fill::Stopped => return Ok(()),
        }
        let now = monotonic_ns();
        served.first_ns.get_or_insert(now);
        served.last_ns = Some(now);
        served.bytes += u64::from(hello.request_bytes);
        if !write_all(stream, &response, stop)? {
            return Ok(());
        }
        served.requests += 1;
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// The clients

/// What the clients have in common.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// The server.
    pub target: SocketAddr,
    /// The workload.
    pub flow: u32,
    /// How many connections, from 1.
    pub connections: u32,
    /// The congestion control of the sockets.
    pub congestion: Option<String>,
    /// Start at this reading of `CLOCK_MONOTONIC`, so that several workloads start at one moment; at once if absent.
    pub start_at_ns: Option<u64>,
    /// The time between two samples of `TCP_INFO`, in nanoseconds; 0 takes none (the last one is always taken).
    pub info_interval_ns: u64,
}

/// What a bulk client sends.
#[derive(Debug, Clone)]
pub struct BulkConfig {
    /// What the clients have in common.
    pub common: ClientConfig,
    /// What each connection sends: a number of payload bytes, or for as long as a time.
    pub amount: Amount,
    /// The payload of a chunk in bytes, from 1 to [`MAX_CHUNK`].
    pub chunk_bytes: u32,
    /// How long a connection waits for the server to say that it read everything, in nanoseconds.
    pub finish_timeout_ns: u64,
}

/// How much a bulk connection sends: a number of payload bytes, a time from the start, or both, and it ends at whichever comes
/// first. At least one is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amount {
    /// This many payload bytes.
    pub bytes: Option<u64>,
    /// For this many nanoseconds from the start.
    pub duration_ns: Option<u64>,
}

impl Amount {
    /// This many bytes, however long it takes.
    #[must_use]
    pub fn bytes(bytes: u64) -> Self {
        Self {
            bytes: Some(bytes),
            duration_ns: None,
        }
    }

    /// As much as fits in this time.
    #[must_use]
    pub fn duration(duration_ns: u64) -> Self {
        Self {
            bytes: None,
            duration_ns: Some(duration_ns),
        }
    }
}

/// What a request client sends.
#[derive(Debug, Clone)]
pub struct RequestConfig {
    /// What the clients have in common.
    pub common: ClientConfig,
    /// The size of a request in bytes, from 1.
    pub request_bytes: u32,
    /// The size of a response in bytes, from 1.
    pub response_bytes: u32,
    /// How long the workload lasts from its start, in nanoseconds: no request is started after it.
    pub duration_ns: u64,
    /// The time between two requests of the whole workload (an open loop); the next request after the response when absent.
    pub interval_ns: Option<u64>,
    /// How long a request waits for its response, in nanoseconds.
    pub timeout_ns: u64,
}

/// What one connection of a client did.
#[derive(Debug, Default)]
struct Done {
    started: bool,
    failed: bool,
    bytes_sent: u64,
    bytes_received: u64,
    requests: u64,
    requests_failed: u64,
    congestion: Option<String>,
    first_error: Option<String>,
    info_error: Option<String>,
    first_ns: Option<u64>,
    last_ns: Option<u64>,
}

/// Waits for the start; `false` if the stop came first.
fn wait_for_start(start_at_ns: Option<u64>, stop: &AtomicBool) -> bool {
    let Some(start) = start_at_ns else {
        return !stop.load(Ordering::SeqCst);
    };
    loop {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        let now = monotonic_ns();
        if now >= start {
            return true;
        }
        thread::sleep(Duration::from_nanos((start - now).min(5_000_000)));
    }
}

/// The samples of `TCP_INFO` of one connection, taken in a thread of their own until `done` is set.
fn sample(
    sink: &Sink,
    connection: u32,
    local: SocketAddr,
    peer: SocketAddr,
    interval_ns: u64,
    done: &AtomicBool,
) -> Option<String> {
    let mut diag = match SockDiag::open() {
        Ok(diag) => diag,
        Err(error) => return Some(format!("cannot read TCP_INFO: {error}")),
    };
    let mut next = Instant::now();
    while !done.load(Ordering::SeqCst) {
        if Instant::now() >= next {
            take(&mut diag, sink, connection, local, peer, STATUS_OK);
            next += Duration::from_nanos(interval_ns);
        }
        thread::sleep(Duration::from_millis(1));
    }
    None
}

/// Takes one sample and writes it; a connection that is gone has none.
fn take(
    diag: &mut SockDiag,
    sink: &Sink,
    connection: u32,
    local: SocketAddr,
    peer: SocketAddr,
    status: u8,
) -> bool {
    let t = monotonic_ns();
    match diag.tcp_info(local, peer) {
        Ok(info) => {
            sink.put(&Record {
                kind: Kind::Info,
                status,
                connection,
                t,
                a: u64::from(info.rtt_us),
                b: u64::from(info.snd_cwnd),
                c: u64::from(info.total_retrans),
                d: info.bytes_acked,
            });
            true
        }
        Err(_) => false,
    }
}

/// Connects, sets the congestion control and says hello. The error is a sentence for the summary.
fn open(
    config: &ClientConfig,
    hello: &Hello,
    done: &mut Done,
) -> Result<(TcpStream, SocketAddr), String> {
    let stream = TcpStream::connect(config.target)
        .map_err(|error| format!("cannot connect to {}: {error}", config.target))?;
    let local = stream.local_addr().map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(POLL))
        .and_then(|()| stream.set_write_timeout(Some(POLL)))
        .map_err(|error| error.to_string())?;
    let _ = stream.set_nodelay(hello.mode == Mode::Requests);
    if let Some(name) = &config.congestion {
        set_congestion(&stream, name)?;
    }
    done.congestion = congestion_of(&stream);
    let mut stream = stream;
    let never = AtomicBool::new(false);
    write_all(&mut stream, &hello.to_bytes(), &never)
        .map_err(|error| format!("cannot say hello: {error}"))?;
    Ok((stream, local))
}

/// Runs the bulk connections and returns what they did.
///
/// # Errors
///
/// The error of `out`, or a congestion control that no socket could use.
pub fn run_bulk(
    config: &BulkConfig,
    stop: &AtomicBool,
    out: impl Write + Send + 'static,
) -> io::Result<TcpSummary> {
    let sink = Sink::new(out, config.common.flow)?;
    let summary = if wait_for_start(config.common.start_at_ns, stop) {
        let begin = config.common.start_at_ns.unwrap_or_else(monotonic_ns);
        thread::scope(|scope| {
            let handles: Vec<_> = (0..config.common.connections)
                .map(|connection| {
                    let sink = &sink;
                    start(scope, move || {
                        bulk_connection(config, connection, begin, stop, sink)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_default())
                .collect::<Vec<_>>()
        })
    } else {
        Vec::new()
    };
    sink.finish()?;
    Ok(summarise(config.common.flow, &summary, stop))
}

fn summarise(flow: u32, all: &[Done], stop: &AtomicBool) -> TcpSummary {
    let mut summary = TcpSummary {
        flow,
        connections: 0,
        failed: 0,
        bytes_sent: 0,
        bytes_received: 0,
        requests: 0,
        requests_failed: 0,
        congestion: None,
        first_error: None,
        info_error: None,
        first_ns: None,
        last_ns: None,
        stopped: stop.load(Ordering::SeqCst),
    };
    for done in all {
        summary.connections += u32::from(done.started);
        summary.failed += u32::from(done.failed);
        summary.bytes_sent += done.bytes_sent;
        summary.bytes_received += done.bytes_received;
        summary.requests += done.requests;
        summary.requests_failed += done.requests_failed;
        if summary.congestion.is_none() {
            summary.congestion.clone_from(&done.congestion);
        }
        if summary.first_error.is_none() {
            summary.first_error.clone_from(&done.first_error);
        }
        if summary.info_error.is_none() {
            summary.info_error.clone_from(&done.info_error);
        }
        summary.first_ns = match (summary.first_ns, done.first_ns) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        summary.last_ns = summary.last_ns.max(done.last_ns);
    }
    summary
}

/// One bulk connection from the connect to the answer of the server.
fn bulk_connection(
    config: &BulkConfig,
    connection: u32,
    begin: u64,
    stop: &AtomicBool,
    sink: &Sink,
) -> Done {
    let mut done = Done::default();
    let started = monotonic_ns();
    done.first_ns = Some(started);
    done.started = true;
    let hello = Hello {
        mode: Mode::Bulk,
        connection,
        request_bytes: 0,
        response_bytes: 0,
    };
    let (mut stream, local) = match open(&config.common, &hello, &mut done) {
        Ok(opened) => opened,
        Err(message) => {
            fail(sink, connection, started, started, 0, 0, &mut done, message);
            return done;
        }
    };
    let established = monotonic_ns();
    let peer = config.common.target;
    let sampling = AtomicBool::new(false);
    let mut ended = established;
    let outcome = thread::scope(|scope| {
        let sampler = (config.common.info_interval_ns > 0).then(|| {
            let (sampling, sink) = (&sampling, sink);
            let interval = config.common.info_interval_ns;
            start(scope, move || {
                sample(sink, connection, local, peer, interval, sampling)
            })
        });
        let result = send_chunks(config, begin, &mut stream, stop, &mut done);
        let finished = result.and_then(|complete| {
            if complete {
                // The end of the data was sent: wait for the server to say that it read it all
                let mut answer = [0_u8; 1];
                let patience = Duration::from_nanos(config.finish_timeout_ns);
                match fill(&mut stream, &mut answer, stop, patience)? {
                    Fill::Full => Ok(true),
                    Fill::Closed => Err(io::Error::other("the server closed without an answer")),
                    Fill::Stopped => Ok(false),
                }
            } else {
                Ok(false)
            }
        });
        ended = monotonic_ns();
        // The last sample is taken here, while the socket is whole
        if let Ok(mut diag) = SockDiag::open() {
            take(&mut diag, sink, connection, local, peer, STATUS_LAST);
        }
        sampling.store(true, Ordering::SeqCst);
        if let Some(sampler) = sampler {
            done.info_error = sampler.join().unwrap_or(None);
        }
        finished
    });
    done.last_ns = Some(ended);
    let status = match outcome {
        Ok(true) => STATUS_OK,
        Ok(false) => STATUS_FAILED,
        Err(error) => {
            done.failed = true;
            done.first_error = Some(error.to_string());
            STATUS_FAILED
        }
    };
    if status == STATUS_FAILED {
        done.failed = true;
    }
    sink.put(&Record {
        kind: Kind::Connection,
        status,
        connection,
        t: started,
        a: established,
        b: ended,
        c: done.bytes_sent,
        d: 0,
    });
    done
}

/// Sends the chunks and the end. `false` if the stop came first.
fn send_chunks(
    config: &BulkConfig,
    begin: u64,
    stream: &mut TcpStream,
    stop: &AtomicBool,
    done: &mut Done,
) -> io::Result<bool> {
    let chunk = config.chunk_bytes.clamp(1, MAX_CHUNK);
    let mut buffer = vec![0_u8; 4 + chunk as usize];
    let mut sent = 0_u64;
    loop {
        // A stop is looked at before every chunk: a write into a connection that takes everything never waits
        if stop.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if config.amount.bytes.is_some_and(|total| sent >= total)
            || config
                .amount
                .duration_ns
                .is_some_and(|length| monotonic_ns() >= begin.saturating_add(length))
        {
            break;
        }
        let size = config.amount.bytes.map_or(chunk, |total| {
            u32::try_from((total - sent).min(u64::from(chunk))).unwrap_or(chunk)
        });
        buffer[0..4].copy_from_slice(&size.to_be_bytes());
        if !write_all(stream, &buffer[..4 + size as usize], stop)? {
            return Ok(false);
        }
        sent += u64::from(size);
        done.bytes_sent = sent;
    }
    write_all(stream, &0_u32.to_be_bytes(), stop)
}

#[allow(clippy::too_many_arguments)]
fn fail(
    sink: &Sink,
    connection: u32,
    t: u64,
    established: u64,
    c: u64,
    d: u64,
    done: &mut Done,
    message: String,
) {
    done.failed = true;
    done.first_error.get_or_insert(message);
    let now = monotonic_ns();
    done.last_ns = Some(now);
    sink.put(&Record {
        kind: Kind::Connection,
        status: STATUS_FAILED,
        connection,
        t,
        a: established,
        b: now,
        c,
        d,
    });
}

/// Runs the request connections and returns what they did.
///
/// # Errors
///
/// The error of `out`.
pub fn run_requests(
    config: &RequestConfig,
    stop: &AtomicBool,
    out: impl Write + Send + 'static,
) -> io::Result<TcpSummary> {
    let sink = Sink::new(out, config.common.flow)?;
    let next = AtomicU64::new(0);
    let all = if wait_for_start(config.common.start_at_ns, stop) {
        let begin = config.common.start_at_ns.unwrap_or_else(monotonic_ns);
        thread::scope(|scope| {
            let handles: Vec<_> = (0..config.common.connections)
                .map(|connection| {
                    let (sink, next) = (&sink, &next);
                    start(scope, move || {
                        request_connection(config, connection, begin, next, stop, sink)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap_or_default())
                .collect::<Vec<_>>()
        })
    } else {
        Vec::new()
    };
    sink.finish()?;
    Ok(summarise(config.common.flow, &all, stop))
}

/// One connection that sends requests until the workload is over.
fn request_connection(
    config: &RequestConfig,
    connection: u32,
    begin: u64,
    next: &AtomicU64,
    stop: &AtomicBool,
    sink: &Sink,
) -> Done {
    let mut done = Done::default();
    let started = monotonic_ns();
    done.first_ns = Some(started);
    done.started = true;
    let hello = Hello {
        mode: Mode::Requests,
        connection,
        request_bytes: config.request_bytes,
        response_bytes: config.response_bytes,
    };
    let (mut stream, local) = match open(&config.common, &hello, &mut done) {
        Ok(opened) => opened,
        Err(message) => {
            fail(sink, connection, started, started, 0, 0, &mut done, message);
            return done;
        }
    };
    let established = monotonic_ns();
    let peer = config.common.target;
    let sampling = AtomicBool::new(false);
    thread::scope(|scope| {
        let sampler = (config.common.info_interval_ns > 0).then(|| {
            let (sampling, sink) = (&sampling, sink);
            let interval = config.common.info_interval_ns;
            start(scope, move || {
                sample(sink, connection, local, peer, interval, sampling)
            })
        });
        requests(
            config,
            connection,
            begin,
            next,
            &mut stream,
            stop,
            sink,
            &mut done,
        );
        if let Ok(mut diag) = SockDiag::open() {
            take(&mut diag, sink, connection, local, peer, STATUS_LAST);
        }
        sampling.store(true, Ordering::SeqCst);
        if let Some(sampler) = sampler {
            done.info_error = sampler.join().unwrap_or(None);
        }
    });
    let ended = monotonic_ns();
    done.last_ns = Some(ended);
    sink.put(&Record {
        kind: Kind::Connection,
        status: if done.failed {
            STATUS_FAILED
        } else {
            STATUS_OK
        },
        connection,
        t: started,
        a: established,
        b: ended,
        c: done.bytes_sent,
        d: done.bytes_received,
    });
    done
}

/// The loop of one connection: take the next request, wait for its moment, send it, read the response.
#[allow(clippy::too_many_arguments)]
fn requests(
    config: &RequestConfig,
    connection: u32,
    begin: u64,
    next: &AtomicU64,
    stream: &mut TcpStream,
    stop: &AtomicBool,
    sink: &Sink,
    done: &mut Done,
) {
    let end = begin.saturating_add(config.duration_ns);
    let mut request = vec![0_u8; config.request_bytes as usize];
    let mut response = vec![0_u8; config.response_bytes as usize];
    let timeout = Duration::from_nanos(config.timeout_ns);
    let mut closed_loop_count = 0_u64;
    loop {
        // The moment the request is due: on the grid of an open loop, or now in a closed one
        let due = match config.interval_ns {
            Some(interval) => {
                let number = next.fetch_add(1, Ordering::SeqCst);
                begin.saturating_add(number.saturating_mul(interval))
            }
            None => monotonic_ns(),
        };
        if due >= end || stop.load(Ordering::SeqCst) {
            return;
        }
        while monotonic_ns() < due {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_nanos(
                (due - monotonic_ns().min(due)).min(2_000_000),
            ));
        }
        let number = match config.interval_ns {
            Some(_) => 0,
            None => closed_loop_count,
        };
        closed_loop_count += 1;
        if request.len() >= 8 {
            request[0..8].copy_from_slice(&number.to_be_bytes());
        }
        // In a closed loop a request is due when it is sent: one reading, so that the two moments are the same
        let sent = monotonic_ns();
        let due = if config.interval_ns.is_none() {
            sent
        } else {
            due
        };
        let answered = write_all(stream, &request, stop).and_then(|written| {
            if !written {
                return Ok(Fill::Stopped);
            }
            fill(stream, &mut response, stop, timeout)
        });
        let arrived = monotonic_ns();
        let (status, ok) = match answered {
            Ok(Fill::Full) => (STATUS_OK, true),
            Ok(Fill::Stopped) => return,
            Ok(Fill::Closed) => {
                done.first_error
                    .get_or_insert("the server closed the connection".to_owned());
                (STATUS_FAILED, false)
            }
            Err(error) => {
                done.first_error.get_or_insert(error.to_string());
                (STATUS_FAILED, false)
            }
        };
        done.bytes_sent += u64::from(config.request_bytes);
        sink.put(&Record {
            kind: Kind::Request,
            status,
            connection,
            t: due,
            a: sent,
            b: arrived,
            c: u64::from(config.request_bytes),
            d: if ok {
                u64::from(config.response_bytes)
            } else {
                0
            },
        });
        if ok {
            done.requests += 1;
            done.bytes_received += u64::from(config.response_bytes);
        } else {
            // A connection that failed a request is not used again: its state is not known
            done.requests_failed += 1;
            done.failed = true;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streams;
    use std::sync::Arc;

    /// A file that the test can read back after the agent wrote it.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        fn records(&self) -> streams::File {
            streams::read(&self.0.lock().unwrap()[..]).unwrap()
        }
    }

    fn serve_config() -> ServeConfig {
        ServeConfig {
            bind: "127.0.0.1:0".parse().unwrap(),
            flow: 7,
            progress_interval_ns: 5_000_000,
            congestion: None,
            max_time_ns: Some(60_000_000_000),
            hello_patience_ns: DEFAULT_HELLO_PATIENCE_NS,
        }
    }

    fn common(target: SocketAddr, connections: u32) -> ClientConfig {
        ClientConfig {
            target,
            flow: 7,
            connections,
            congestion: None,
            start_at_ns: None,
            info_interval_ns: 20_000_000,
        }
    }

    /// Starts a server on a thread; the test stops it and gets what it wrote.
    fn with_server<T>(
        config: &ServeConfig,
        body: impl FnOnce(SocketAddr) -> T,
    ) -> (T, ServeSummary, streams::File) {
        let server = Server::bind(config).unwrap();
        let address = server.local_addr().unwrap();
        let file = Shared::default();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = {
            let (stop, file) = (Arc::clone(&stop), file.clone());
            thread::spawn(move || server.run(&stop, file).unwrap())
        };
        let result = body(address);
        stop.store(true, Ordering::SeqCst);
        let summary = handle.join().unwrap();
        (result, summary, file.records())
    }

    #[test]
    fn a_hello_has_these_exact_bytes_and_other_bytes_are_refused() {
        let hello = Hello {
            mode: Mode::Requests,
            connection: 3,
            request_bytes: 100,
            response_bytes: 1000,
        };
        let bytes = hello.to_bytes();
        let mut expected = b"NSTP".to_vec();
        expected.extend([1, 2, 0, 0, 0, 0, 0, 3, 0, 0, 0, 100, 0, 0, 3, 232]);
        expected.extend([0; 12]);
        assert_eq!(bytes.to_vec(), expected);
        assert_eq!(Hello::from_bytes(&bytes).unwrap(), hello);
        let mut wrong = bytes;
        wrong[0] = b'X';
        assert!(Hello::from_bytes(&wrong).unwrap_err().contains("NSTP"));
        let mut wrong = bytes;
        wrong[4] = 2;
        assert!(Hello::from_bytes(&wrong).unwrap_err().contains("version"));
        let mut wrong = bytes;
        wrong[5] = 9;
        assert!(Hello::from_bytes(&wrong).unwrap_err().contains("mode"));
        let mut wrong = bytes;
        wrong[31] = 1;
        assert!(Hello::from_bytes(&wrong).unwrap_err().contains("reserved"));
        let mut wrong = bytes;
        wrong[12..16].copy_from_slice(&0_u32.to_be_bytes());
        assert!(
            Hello::from_bytes(&wrong)
                .unwrap_err()
                .contains("at least one")
        );
    }

    /// The server counts the payload that it read, to the byte, and the client knows the transfer is complete only when the
    /// server has said so.
    #[test]
    fn a_bulk_transfer_arrives_to_the_byte_and_has_a_completion_and_samples() {
        let total = 3 * 1024 * 1024 + 17;
        let client_file = Shared::default();
        let (summary, served, server_file) = with_server(&serve_config(), |address| {
            let config = BulkConfig {
                common: common(address, 2),
                amount: Amount::bytes(total),
                chunk_bytes: 65_536,
                finish_timeout_ns: DEFAULT_FINISH_TIMEOUT_NS,
            };
            run_bulk(&config, &AtomicBool::new(false), client_file.clone()).unwrap()
        });
        assert_eq!(summary.connections, 2);
        assert_eq!(summary.failed, 0, "{:?}", summary.first_error);
        assert_eq!(summary.bytes_sent, 2 * total);
        assert_eq!(served.bytes_received, 2 * total);
        assert_eq!((served.connections, served.refused), (2, 0));
        // The progress records of the server add up to the same, and each is in an interval of the clock
        let progress: u64 = server_file
            .records
            .iter()
            .filter(|r| r.kind == Kind::Progress)
            .map(|r| r.a)
            .sum();
        assert_eq!(progress, 2 * total);
        assert!(
            server_file
                .records
                .iter()
                .filter(|r| r.kind == Kind::Progress)
                .all(|r| r.t % 5_000_000 == 0 && r.a > 0 && r.b > 0)
        );
        // The client has one connection record for each, completed in order, and a last sample for each
        let file = client_file.records();
        let connections: Vec<_> = file
            .records
            .iter()
            .filter(|r| r.kind == Kind::Connection)
            .collect();
        assert_eq!(connections.len(), 2);
        for connection in connections {
            assert_eq!(connection.status, STATUS_OK);
            assert!(connection.t <= connection.a && connection.a < connection.b);
            assert_eq!(connection.c, total);
        }
        let last: Vec<_> = file
            .records
            .iter()
            .filter(|r| r.kind == Kind::Info && r.status == STATUS_LAST)
            .collect();
        assert_eq!(last.len(), 2, "{:?}", summary.info_error);
        // The kernel counted at least the payload as acknowledged (and the SYN as one)
        assert!(last.iter().all(|r| r.d >= total && r.b > 0), "{last:?}");
        assert_eq!(
            summary.congestion.as_deref().map(str::is_empty),
            Some(false)
        );
    }

    #[test]
    fn a_duration_bulk_sends_for_that_long_and_then_ends() {
        let client_file = Shared::default();
        let started = Instant::now();
        let (summary, served, _) = with_server(&serve_config(), |address| {
            let config = BulkConfig {
                common: common(address, 1),
                amount: Amount::duration(300_000_000),
                chunk_bytes: 16_384,
                finish_timeout_ns: DEFAULT_FINISH_TIMEOUT_NS,
            };
            run_bulk(&config, &AtomicBool::new(false), client_file.clone()).unwrap()
        });
        assert!(started.elapsed() >= Duration::from_millis(300));
        assert_eq!(summary.failed, 0, "{:?}", summary.first_error);
        assert_eq!(served.bytes_received, summary.bytes_sent);
        assert!(summary.bytes_sent > 100_000, "{}", summary.bytes_sent);
        // Samples were taken during the transfer, not only at its end
        let samples = client_file
            .records()
            .records
            .iter()
            .filter(|r| r.kind == Kind::Info && r.status == STATUS_OK)
            .count();
        assert!(samples >= 3, "{samples}");
    }

    /// Requests and responses: each is answered, each record has the order of its moments, and the sizes are the asked ones.
    #[test]
    fn requests_are_answered_with_the_size_asked_for_in_a_closed_loop() {
        let client_file = Shared::default();
        let (summary, served, _) = with_server(&serve_config(), |address| {
            let config = RequestConfig {
                common: common(address, 2),
                request_bytes: 100,
                response_bytes: 5_000,
                duration_ns: 300_000_000,
                interval_ns: None,
                timeout_ns: DEFAULT_REQUEST_TIMEOUT_NS,
            };
            run_requests(&config, &AtomicBool::new(false), client_file.clone()).unwrap()
        });
        assert_eq!(summary.failed, 0, "{:?}", summary.first_error);
        assert!(summary.requests > 50, "{}", summary.requests);
        assert_eq!(served.requests, summary.requests);
        let file = client_file.records();
        let requests: Vec<_> = file
            .records
            .iter()
            .filter(|r| r.kind == Kind::Request)
            .collect();
        assert_eq!(requests.len() as u64, summary.requests);
        for r in &requests {
            assert_eq!(r.status, STATUS_OK);
            assert!(r.t <= r.a && r.a < r.b, "{r:?}");
            assert_eq!((r.c, r.d), (100, 5_000));
        }
        // A closed loop sends the next request after the response of the one before on that connection
        for connection in 0..2 {
            let own: Vec<_> = requests
                .iter()
                .filter(|r| r.connection == connection)
                .collect();
            assert!(own.len() > 10, "{}", own.len());
            assert!(own.windows(2).all(|w| w[0].b <= w[1].a));
        }
    }

    /// An open loop has its requests due on a grid, whatever the connections do.
    #[test]
    fn an_open_loop_has_its_requests_due_on_a_grid() {
        let client_file = Shared::default();
        let (summary, _, _) = with_server(&serve_config(), |address| {
            let config = RequestConfig {
                common: common(address, 3),
                request_bytes: 64,
                response_bytes: 64,
                duration_ns: 500_000_000,
                interval_ns: Some(20_000_000),
                timeout_ns: DEFAULT_REQUEST_TIMEOUT_NS,
            };
            run_requests(&config, &AtomicBool::new(false), client_file.clone()).unwrap()
        });
        assert_eq!(summary.failed, 0, "{:?}", summary.first_error);
        let file = client_file.records();
        let mut due: Vec<u64> = file
            .records
            .iter()
            .filter(|r| r.kind == Kind::Request)
            .map(|r| r.t)
            .collect();
        due.sort_unstable();
        // 500 ms every 20 ms is 25 requests, each due 20 ms after the one before
        assert_eq!(due.len(), 25, "{due:?}");
        assert!(due.windows(2).all(|w| w[1] - w[0] == 20_000_000), "{due:?}");
        // A request is never sent before it is due
        assert!(
            file.records
                .iter()
                .filter(|r| r.kind == Kind::Request)
                .all(|r| r.a >= r.t)
        );
    }

    /// Canary: a server that is not there is a failed connection with the reason, not an empty success.
    #[test]
    fn a_connection_that_cannot_be_made_is_a_failure_with_its_reason() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let client_file = Shared::default();
        let config = BulkConfig {
            common: common(address, 1),
            amount: Amount::bytes(1000),
            chunk_bytes: 1000,
            finish_timeout_ns: 1_000_000_000,
        };
        let summary = run_bulk(&config, &AtomicBool::new(false), client_file.clone()).unwrap();
        assert_eq!((summary.connections, summary.failed), (1, 1));
        assert!(summary.first_error.unwrap().contains("cannot connect"));
        let record = client_file.records().records[0];
        assert_eq!(
            (record.kind, record.status),
            (Kind::Connection, STATUS_FAILED)
        );
    }

    /// Canary: a congestion control that the machine lacks is refused with its name, and one it has is used and named.
    #[test]
    fn a_congestion_control_is_used_or_refused_by_name() {
        let client_file = Shared::default();
        let (summary, _, _) = with_server(&serve_config(), |address| {
            let mut common = common(address, 1);
            common.congestion = Some("reno".to_owned());
            let config = BulkConfig {
                common,
                amount: Amount::bytes(10_000),
                chunk_bytes: 5_000,
                finish_timeout_ns: DEFAULT_FINISH_TIMEOUT_NS,
            };
            run_bulk(&config, &AtomicBool::new(false), client_file.clone()).unwrap()
        });
        assert_eq!(
            summary.congestion.as_deref(),
            Some("reno"),
            "{:?}",
            summary.first_error
        );
        let (summary, _, _) = with_server(&serve_config(), |address| {
            let mut common = common(address, 1);
            common.congestion = Some("nonexistent-cc".to_owned());
            let config = BulkConfig {
                common,
                amount: Amount::bytes(10_000),
                chunk_bytes: 5_000,
                finish_timeout_ns: 1_000_000_000,
            };
            run_bulk(&config, &AtomicBool::new(false), Shared::default()).unwrap()
        });
        assert_eq!(summary.failed, 1);
        assert!(
            summary.first_error.unwrap().contains("nonexistent-cc"),
            "the name of the congestion control is in the reason"
        );
    }

    /// A hello that comes late is waited for: a connection whose first segments are lost again and again on a congested path says
    /// hello after the retransmissions, seconds late, and is not refused for it (the patience is long). With a short patience,
    /// the same late hello is refused, which shows that the patience is what decides.
    #[test]
    fn a_late_hello_is_waited_for_as_long_as_the_patience_lasts() {
        let late = |config: &ServeConfig| {
            let ((), served, _) = with_server(config, |address| {
                let mut client = TcpStream::connect(address).unwrap();
                thread::sleep(Duration::from_millis(600));
                let hello = Hello {
                    mode: Mode::Bulk,
                    connection: 0,
                    request_bytes: 0,
                    response_bytes: 0,
                };
                // The client may find the connection closed by now, which is the point of the other half of the test
                let _ = client.write_all(&hello.to_bytes());
            });
            served.refused
        };
        assert_eq!(late(&serve_config()), 0, "the late hello was refused");
        let mut impatient = serve_config();
        impatient.hello_patience_ns = 200_000_000;
        assert_eq!(late(&impatient), 1, "a hello after the patience is refused");
    }

    /// The retransmission timer of TCP doubles from 0.2 s: eight retransmissions take 51 s. A server that gives up on the hello
    /// before that refuses connections that TCP would still have delivered.
    #[test]
    fn the_default_patience_for_a_hello_outlasts_eight_retransmissions() {
        let eight_retransmissions_ns = (0..8).map(|n| 200_000_000_u64 << n).sum::<u64>();
        assert!(DEFAULT_HELLO_PATIENCE_NS > eight_retransmissions_ns);
    }

    /// A stranger that does not say hello is refused and does not stop the server.
    #[test]
    fn a_stranger_is_refused_and_the_server_goes_on() {
        let (outcome, served, _) = with_server(&serve_config(), |address| {
            let mut stranger = TcpStream::connect(address).unwrap();
            stranger.write_all(&[b'G'; 32]).unwrap();
            let mut byte = [0_u8; 1];
            // The server closes on it
            let closed = stranger.read(&mut byte).map_or(true, |n| n == 0);
            let config = BulkConfig {
                common: common(address, 1),
                amount: Amount::bytes(5_000),
                chunk_bytes: 5_000,
                finish_timeout_ns: DEFAULT_FINISH_TIMEOUT_NS,
            };
            let summary = run_bulk(&config, &AtomicBool::new(false), Shared::default()).unwrap();
            (closed, summary)
        });
        assert!(outcome.0, "the stranger was not closed on");
        assert_eq!(outcome.1.failed, 0);
        assert_eq!(
            (served.connections, served.refused, served.bytes_received),
            (2, 1, 5_000)
        );
    }

    /// With both limits a connection ends at whichever comes first: the time here, since the bytes would take far longer.
    #[test]
    fn a_connection_with_bytes_and_a_time_ends_at_the_one_that_comes_first() {
        let started = Instant::now();
        let (summary, served, _) = with_server(&serve_config(), |address| {
            let config = BulkConfig {
                common: common(address, 1),
                amount: Amount {
                    bytes: Some(1 << 40),
                    duration_ns: Some(200_000_000),
                },
                chunk_bytes: 16_384,
                finish_timeout_ns: DEFAULT_FINISH_TIMEOUT_NS,
            };
            run_bulk(&config, &AtomicBool::new(false), Shared::default()).unwrap()
        });
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(summary.failed, 0, "{:?}", summary.first_error);
        assert!(summary.bytes_sent > 0 && summary.bytes_sent < 1 << 40);
        assert_eq!(served.bytes_received, summary.bytes_sent);
        // And the bytes when they come first: 50,000 bytes are sent whole though the time is long
        let (summary, _, _) = with_server(&serve_config(), |address| {
            let config = BulkConfig {
                common: common(address, 1),
                amount: Amount {
                    bytes: Some(50_000),
                    duration_ns: Some(60_000_000_000),
                },
                chunk_bytes: 16_384,
                finish_timeout_ns: DEFAULT_FINISH_TIMEOUT_NS,
            };
            run_bulk(&config, &AtomicBool::new(false), Shared::default()).unwrap()
        });
        assert_eq!(summary.bytes_sent, 50_000);
    }

    #[test]
    fn a_stop_ends_a_transfer_early_and_says_so() {
        let stop = Arc::new(AtomicBool::new(false));
        let (summary, _, _) = with_server(&serve_config(), |address| {
            let config = BulkConfig {
                common: common(address, 1),
                amount: Amount::duration(30_000_000_000),
                chunk_bytes: 16_384,
                finish_timeout_ns: DEFAULT_FINISH_TIMEOUT_NS,
            };
            let stopper = {
                let stop = Arc::clone(&stop);
                thread::spawn(move || {
                    thread::sleep(Duration::from_millis(200));
                    stop.store(true, Ordering::SeqCst);
                })
            };
            let started = Instant::now();
            let summary = run_bulk(&config, &stop, Shared::default()).unwrap();
            stopper.join().unwrap();
            assert!(started.elapsed() < Duration::from_secs(10));
            summary
        });
        assert!(summary.stopped);
        assert_eq!(
            summary.failed, 1,
            "a transfer that was stopped did not complete"
        );
    }
}
