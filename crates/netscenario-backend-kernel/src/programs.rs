//! The output of the programs in the nodes, and the sockets of a node (D-026, D-072).
//!
//! Each program has two threads that read its standard output and standard error to the end, whatever the program
//! does, so that it is never blocked by a full pipe. Standard output is cut into lines (a line is at most
//! [`LINE_LIMIT`] bytes; the rest of a longer one is dropped and counted), the first [`MAX_LINES`] unread lines are
//! kept for the caller, and the first [`LOG_LIMIT`] bytes of each stream are written to a file, if the caller named one.
//! What was dropped is counted and reported, never hidden.

use std::fs::File;
use std::io::{Read, Write};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};

use netscenario_core::{LINE_LIMIT, LOG_LIMIT, OutputStats};

/// The most lines of standard output that the supervisor keeps for a program that nobody asks about.
pub(crate) const MAX_LINES: usize = 10_000;

/// The most bytes of standard error that are kept for the caller: the end of it.
pub(crate) const STDERR_LIMIT: usize = 16 * 1024;

/// What the threads that read a program's output collect.
#[derive(Default)]
pub(crate) struct Output {
    state: Mutex<OutputState>,
    pub(crate) news: Condvar,
}

#[derive(Default)]
pub(crate) struct OutputState {
    pub(crate) lines: Vec<String>,
    pub(crate) kept: usize,
    pub(crate) truncated: bool,
    pub(crate) stdout_ended: bool,
    pub(crate) stderr_ended: bool,
    pub(crate) stderr: String,
    pub(crate) stats: OutputStats,
}

impl Output {
    pub(crate) fn lock(&self) -> MutexGuard<'_, OutputState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// A file that receives the first [`LOG_LIMIT`] bytes of a stream.
struct Log {
    file: Option<File>,
    written: u64,
}

impl Log {
    fn new(file: Option<File>) -> Self {
        Self { file, written: 0 }
    }

    /// Writes what fits and returns how many bytes did not (0 for a stream that has no file).
    fn write(&mut self, bytes: &[u8]) -> u64 {
        let Some(file) = &mut self.file else {
            return 0;
        };
        let room = LOG_LIMIT.saturating_sub(self.written);
        let fits = usize::try_from(room).map_or(bytes.len(), |room| room.min(bytes.len()));
        if fits > 0 && file.write_all(&bytes[..fits]).is_err() {
            // The file cannot be written (the disk is full): the rest is counted as dropped, the program goes on
            self.file = None;
            return bytes.len() as u64;
        }
        self.written += fits as u64;
        (bytes.len() - fits) as u64
    }
}

/// Reads standard output to its end: lines for the caller, bytes for the file.
pub(crate) fn read_stdout(mut pipe: impl Read, output: &Output, file: Option<File>) {
    let mut log = Log::new(file);
    let mut chunk = [0_u8; 8192];
    let mut line: Vec<u8> = Vec::new();
    let mut overlong = false;
    while let Ok(n) = pipe.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let dropped = log.write(&chunk[..n]);
        let mut done: Vec<String> = Vec::new();
        let mut long_lines = 0_u64;
        let mut start = 0;
        while start < n {
            let end = chunk[start..n]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(n, |at| start + at);
            let part = &chunk[start..end];
            if !overlong {
                let room = LINE_LIMIT - line.len();
                if part.len() <= room {
                    line.extend_from_slice(part);
                } else {
                    line.extend_from_slice(&part[..room]);
                    overlong = true;
                }
            }
            if end < n {
                // A program may write what is not UTF-8: it is shown with replacement characters, not dropped
                done.push(
                    String::from_utf8_lossy(&line)
                        .trim_end_matches('\r')
                        .to_owned(),
                );
                long_lines += u64::from(overlong);
                overlong = false;
                line.clear();
            }
            start = end + 1;
        }
        let mut state = output.lock();
        state.stats.stdout_bytes += n as u64;
        state.stats.stdout_dropped_bytes += dropped;
        state.stats.long_lines += long_lines;
        for text in done {
            if state.kept < MAX_LINES {
                state.lines.push(text);
                state.kept += 1;
            } else {
                state.truncated = true;
            }
        }
        output.news.notify_all();
    }
    let mut state = output.lock();
    // The last line of a program that did not end it with a line feed
    if !line.is_empty() {
        let text = String::from_utf8_lossy(&line)
            .trim_end_matches('\r')
            .to_owned();
        state.stats.long_lines += u64::from(overlong);
        if state.kept < MAX_LINES {
            state.lines.push(text);
            state.kept += 1;
        } else {
            state.truncated = true;
        }
    }
    state.stdout_ended = true;
    output.news.notify_all();
}

/// Reads standard error to its end: the end of it for the caller, the beginning for the file.
pub(crate) fn read_stderr(mut pipe: impl Read, output: &Output, file: Option<File>) {
    let mut log = Log::new(file);
    let mut buffer = [0_u8; 1024];
    while let Ok(n) = pipe.read(&mut buffer) {
        if n == 0 {
            break;
        }
        let dropped = log.write(&buffer[..n]);
        let mut state = output.lock();
        state.stats.stderr_bytes += n as u64;
        state.stats.stderr_dropped_bytes += dropped;
        state
            .stderr
            .push_str(&String::from_utf8_lossy(&buffer[..n]));
        if state.stderr.len() > STDERR_LIMIT {
            let cut = state.stderr.len() - STDERR_LIMIT;
            let cut = (cut..state.stderr.len())
                .find(|&i| state.stderr.is_char_boundary(i))
                .unwrap_or(0);
            state.stderr.drain(..cut);
        }
    }
    output.lock().stderr_ended = true;
    output.news.notify_all();
}

/// Whether a table of sockets of `/proc/net` (`udp`, `tcp`, `udp6`, `tcp6`) has a socket in `state` on `port` that can be
/// reached from another node: bound to any address or to a real one of the node, not to the loopback (S14). `state` is
/// `07` for a bound UDP socket and `0A` for a listening TCP socket.
pub(crate) fn socket_is_bound(table: &str, port: u16, state: &str) -> bool {
    table.lines().skip(1).any(|row| {
        let mut fields = row.split_whitespace();
        let (Some(local), Some(_remote), Some(found)) =
            (fields.nth(1), fields.next(), fields.next())
        else {
            return false;
        };
        let Some((address, hex_port)) = local.rsplit_once(':') else {
            return false;
        };
        found == state
            && u16::from_str_radix(hex_port, 16).is_ok_and(|bound| bound == port)
            && !is_loopback(address)
    })
}

/// `127.0.0.1` is `0100007F` in `/proc/net` (the bytes of the address as a little-endian number), `::1` is
/// `00000000000000000000000001000000`.
fn is_loopback(address: &str) -> bool {
    match address.len() {
        8 => address.ends_with("7F"),
        32 => address == "00000000000000000000000001000000",
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(input: &[u8]) -> (Vec<String>, OutputStats) {
        let output = Output::default();
        read_stdout(input, &output, None);
        let state = output.lock();
        (state.lines.clone(), state.stats)
    }

    #[test]
    fn lines_are_cut_at_the_line_feed_and_the_last_one_needs_none() {
        let (lines, stats) = read(b"one\r\ntwo\n\nlast");
        assert_eq!(lines, ["one", "two", "", "last"]);
        assert_eq!(stats.stdout_bytes, 14);
        assert_eq!(stats.long_lines, 0);
    }

    #[test]
    fn what_is_not_utf8_is_shown_with_replacement_characters() {
        let (lines, _) = read(b"a\xffb\n");
        assert_eq!(lines, ["a\u{fffd}b"]);
    }

    /// S14 found a reader without a limit that held 2 GB of a line without a line feed. A longer line is cut, counted, and
    /// the lines after it are intact; a line of exactly the limit is not long.
    #[test]
    fn a_line_longer_than_the_limit_is_cut_and_counted() {
        let mut input = vec![b'x'; LINE_LIMIT + 5000];
        input.extend_from_slice(b"\nnext\n");
        input.extend(vec![b'y'; LINE_LIMIT]);
        input.push(b'\n');
        let (lines, stats) = read(&input);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].len(), LINE_LIMIT);
        assert_eq!(lines[1], "next");
        assert_eq!(lines[2].len(), LINE_LIMIT);
        assert_eq!(stats.long_lines, 1);
    }

    /// A long line without any line feed at the end of the stream is counted too.
    #[test]
    fn a_long_last_line_is_counted() {
        let (lines, stats) = read(&vec![b'z'; LINE_LIMIT * 2]);
        assert_eq!(
            (lines.len(), lines[0].len(), stats.long_lines),
            (1, LINE_LIMIT, 1)
        );
    }

    #[test]
    fn lines_past_the_number_kept_are_dropped_and_said() {
        let input = (0..MAX_LINES + 3).fold(String::new(), |mut text, n| {
            text.push_str(&n.to_string());
            text.push('\n');
            text
        });
        let output = Output::default();
        read_stdout(input.as_bytes(), &output, None);
        let state = output.lock();
        assert_eq!(state.lines.len(), MAX_LINES);
        assert!(state.truncated);
    }

    #[test]
    fn the_file_gets_the_first_megabyte_and_the_rest_is_counted() {
        let directory =
            std::env::temp_dir().join(format!("netscenario-log-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("out.log");
        let size = usize::try_from(LOG_LIMIT).unwrap();
        let mut input = vec![b'a'; size];
        input.extend(vec![b'b'; 10_000]);
        let output = Output::default();
        read_stdout(
            input.as_slice(),
            &output,
            Some(File::create(&path).unwrap()),
        );
        let written = std::fs::read(&path).unwrap();
        assert_eq!(written.len(), size);
        assert!(written.iter().all(|byte| *byte == b'a'));
        let stats = output.lock().stats;
        assert_eq!(
            (stats.stdout_bytes, stats.stdout_dropped_bytes),
            (size as u64 + 10_000, 10_000)
        );
        // Standard error is cut the same way, and the caller still gets the end of it
        let errors = Output::default();
        let path = directory.join("err.log");
        read_stderr(
            input.as_slice(),
            &errors,
            Some(File::create(&path).unwrap()),
        );
        assert_eq!(std::fs::read(&path).unwrap().len(), size);
        let seen = errors.lock();
        assert_eq!(seen.stats.stderr_dropped_bytes, 10_000);
        assert_eq!(seen.stderr.len(), STDERR_LIMIT);
        assert!(seen.stderr.ends_with('b'));
        let _ = std::fs::remove_dir_all(directory);
    }

    const UDP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
  120: 00000000:1131 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 4242 2 0000000000000000 0
  121: 0200640A:1F90 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 4243 2 0000000000000000 0
  122: 0100007F:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 4244 2 0000000000000000 0
  123: 0200640A:2222 0100640A:0050 01 00000000:00000000 00:00000000 00000000     0        0 4245 2 0000000000000000 0
";

    #[test]
    fn a_port_is_bound_at_any_address_or_the_address_of_the_node_but_not_on_the_loopback() {
        // 0x1131 is 4401, bound to any address; 0x1F90 is 8080, bound to 10.100.0.2
        assert!(socket_is_bound(UDP, 4401, "07"));
        assert!(socket_is_bound(UDP, 8080, "07"));
        // 0x0035 is 53 on 127.0.0.1: not reachable from the other node
        assert!(!socket_is_bound(UDP, 53, "07"));
        // A connected socket (state 01) is not a bound one, and another port is not this port
        assert!(!socket_is_bound(UDP, 0x2222, "07"));
        assert!(!socket_is_bound(UDP, 4402, "07"));
        // The header is not a socket, and a table that is empty has none
        assert!(!socket_is_bound("  sl  local_address\n", 4401, "07"));
        assert!(!socket_is_bound("", 4401, "07"));
        // A listening TCP socket is state 0A, a bound UDP one is not
        assert!(!socket_is_bound(UDP, 4401, "0A"));
    }

    #[test]
    fn the_loopback_of_ipv6_is_not_reachable_either() {
        let table = "  sl local_address rem_address st\n   0: 00000000000000000000000001000000:115C 00000000000000000000000000000000:0000 0A\n   1: 00000000000000000000000000000000:115D 00000000000000000000000000000000:0000 0A\n";
        assert!(!socket_is_bound(table, 0x115C, "0A"));
        assert!(socket_is_bound(table, 0x115D, "0A"));
    }
}
