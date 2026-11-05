//! The file of records that the TCP agents write (the stream record format, version 1; D-086).
//!
//! A UDP flow has one record for each packet. A TCP flow has no packets that the agent can see, so its file holds the
//! events of its connections: the bytes that arrived in an interval, the samples of `TCP_INFO`, the life of a connection
//! and one record for each request that was answered. All numbers are in network byte order and every time is
//! `CLOCK_MONOTONIC` in nanoseconds, the clock of the records of the UDP agents.
//!
//! The file starts with a header of 16 bytes and continues with records of 48 bytes:
//!
//! | Bytes | Field | Meaning |
//! | --- | --- | --- |
//! | 0 to 7 | magic | `NSSTREAM` |
//! | 8 to 9 | version | `1` |
//! | 10 to 11 | record size | `48` |
//! | 12 to 15 | flow | The number of the workload that the agent was started for. |
//!
//! | Record bytes | Field | Meaning |
//! | --- | --- | --- |
//! | 0 | kind | 1 progress, 2 info, 3 connection, 4 request |
//! | 1 | status | By kind, see [`Kind`] |
//! | 2 to 3 | reserved | zero |
//! | 4 to 7 | connection | The number of the connection in its workload, from 0 |
//! | 8 to 15 | `t` | By kind |
//! | 16 to 23 | `a` | By kind |
//! | 24 to 31 | `b` | By kind |
//! | 32 to 39 | `c` | By kind |
//! | 40 to 47 | `d` | By kind |
//!
//! The records are in the order they were written, not in the order of their times: the agent writes from several threads.

use std::io::{self, Read, Write};

/// The length of the header of the file.
pub const FILE_HEADER_LEN: usize = 16;

/// The length of one record.
pub const RECORD_LEN: usize = 48;

const MAGIC: &[u8; 8] = b"NSSTREAM";
const VERSION: u16 = 1;

/// What a record says. The numbers are the ones in the file and never change; a new kind gets the next number and a new
/// version of the format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// The bytes that the receiving end read in one interval of the clock (written by the server). `t` is the start of the
    /// interval, a multiple of the interval on `CLOCK_MONOTONIC`; `a` is the bytes read in it; `b` how many reads it took.
    /// A connection that read nothing in an interval has no record for it. Status 0.
    Progress = 1,
    /// A sample of `TCP_INFO` of the sending socket (written by the client). `t` is the moment of the sample; `a` the
    /// smoothed round trip time in microseconds; `b` the congestion window in segments; `c` the retransmitted segments in all
    /// so far; `d` the bytes acknowledged so far as the kernel counts them (the SYN is one, so use differences). Status 0 for a sample of the interval, 1 for the last sample of the
    /// connection, which is taken after the peer closed.
    Info = 2,
    /// The life of one connection (written by the client). `t` is when the connect was started; `a` when it was
    /// established; `b` when the connection was done (the peer had closed after it read everything); `c` the bytes the
    /// client sent; `d` the bytes it received. Status 0 for a connection that did its work, 2 for one that failed (`b` is
    /// then the moment of the failure).
    Connection = 3,
    /// One request and its response (written by the client). `t` is the moment the request was due (for a closed loop the
    /// moment it was sent); `a` when it was sent; `b` when the last byte of the response was read; `c` the bytes of the
    /// request; `d` the bytes of the response. Status 0 for an answered request, 2 for one that failed (`b` is then the
    /// moment of the failure).
    Request = 4,
}

impl Kind {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Kind::Progress),
            2 => Some(Kind::Info),
            3 => Some(Kind::Connection),
            4 => Some(Kind::Request),
            _ => None,
        }
    }
}

/// The status of a record that did what it was meant to.
pub const STATUS_OK: u8 = 0;
/// The status of the last sample of a connection.
pub const STATUS_LAST: u8 = 1;
/// The status of a connection or a request that failed.
pub const STATUS_FAILED: u8 = 2;

/// One record. What `t`, `a`, `b`, `c` and `d` mean depends on the [`Kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// What the record says.
    pub kind: Kind,
    /// See the kind.
    pub status: u8,
    /// The connection, from 0.
    pub connection: u32,
    /// See the kind.
    pub t: u64,
    /// See the kind.
    pub a: u64,
    /// See the kind.
    pub b: u64,
    /// See the kind.
    pub c: u64,
    /// See the kind.
    pub d: u64,
}

/// Writes the file header and then the records.
#[derive(Debug)]
pub struct Writer<W: Write> {
    out: W,
    written: u64,
}

impl<W: Write> Writer<W> {
    /// Starts a file for `flow`.
    ///
    /// # Errors
    ///
    /// The error of `out`.
    pub fn new(mut out: W, flow: u32) -> io::Result<Self> {
        let mut header = [0_u8; FILE_HEADER_LEN];
        header[0..8].copy_from_slice(MAGIC);
        header[8..10].copy_from_slice(&VERSION.to_be_bytes());
        header[10..12].copy_from_slice(&u16::try_from(RECORD_LEN).unwrap_or(0).to_be_bytes());
        header[12..16].copy_from_slice(&flow.to_be_bytes());
        out.write_all(&header)?;
        Ok(Self { out, written: 0 })
    }

    /// Appends a record.
    ///
    /// # Errors
    ///
    /// The error of `out`.
    pub fn write(&mut self, record: &Record) -> io::Result<()> {
        let mut bytes = [0_u8; RECORD_LEN];
        bytes[0] = record.kind as u8;
        bytes[1] = record.status;
        bytes[4..8].copy_from_slice(&record.connection.to_be_bytes());
        bytes[8..16].copy_from_slice(&record.t.to_be_bytes());
        bytes[16..24].copy_from_slice(&record.a.to_be_bytes());
        bytes[24..32].copy_from_slice(&record.b.to_be_bytes());
        bytes[32..40].copy_from_slice(&record.c.to_be_bytes());
        bytes[40..48].copy_from_slice(&record.d.to_be_bytes());
        self.out.write_all(&bytes)?;
        self.written += 1;
        Ok(())
    }

    /// How many records were written.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.written
    }

    /// Flushes and returns the writer.
    ///
    /// # Errors
    ///
    /// The error of `out`.
    pub fn finish(mut self) -> io::Result<W> {
        self.out.flush()?;
        Ok(self.out)
    }

    /// Flushes what is buffered, so that a reader sees the records so far.
    ///
    /// # Errors
    ///
    /// The error of `out`.
    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }
}

/// What a file contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// The workload that the agent was started for.
    pub flow: u32,
    /// The records, in the order they were written.
    pub records: Vec<Record>,
}

/// Why a file cannot be read.
#[derive(Debug)]
pub enum ReadError {
    /// The file cannot be read at all.
    Io(io::Error),
    /// It does not start with the magic of a stream record file.
    NotAStreamFile,
    /// A version or a record size this program does not read.
    Unsupported {
        /// The version in the file.
        version: u16,
        /// The record size in the file.
        record_size: u16,
    },
    /// The last record is cut off: the agent was killed while it wrote.
    Truncated {
        /// How many whole records precede the cut.
        records: usize,
        /// How many bytes of the cut record are there.
        extra_bytes: usize,
    },
    /// A record of a kind that this version does not have. The file is not read on: a record that is skipped is a number
    /// that is missing from a result.
    UnknownKind {
        /// The number of the record, from 0.
        record: usize,
        /// The kind in it.
        kind: u8,
    },
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(error) => write!(f, "cannot read the stream file: {error}"),
            ReadError::NotAStreamFile => write!(
                f,
                "this is not a stream file (it does not start with NSSTREAM)"
            ),
            ReadError::Unsupported {
                version,
                record_size,
            } => write!(
                f,
                "version {version} with records of {record_size} bytes is not supported (this program reads version {VERSION} with {RECORD_LEN})"
            ),
            ReadError::Truncated {
                records,
                extra_bytes,
            } => write!(
                f,
                "the file ends inside a record ({records} whole records, then {extra_bytes} bytes)"
            ),
            ReadError::UnknownKind { record, kind } => write!(
                f,
                "record {record} is of the kind {kind}, which this version of the format does not have"
            ),
        }
    }
}

impl std::error::Error for ReadError {}

impl From<io::Error> for ReadError {
    fn from(error: io::Error) -> Self {
        ReadError::Io(error)
    }
}

/// Reads a whole file.
///
/// # Errors
///
/// A [`ReadError`] if it is not a stream file of version 1, ends inside a record, or holds a record of an unknown kind.
pub fn read(mut input: impl Read) -> Result<File, ReadError> {
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes)?;
    let Some(header) = bytes.get(..FILE_HEADER_LEN) else {
        return Err(ReadError::NotAStreamFile);
    };
    if &header[0..8] != MAGIC {
        return Err(ReadError::NotAStreamFile);
    }
    let version = u16::from_be_bytes([header[8], header[9]]);
    let record_size = u16::from_be_bytes([header[10], header[11]]);
    if version != VERSION || usize::from(record_size) != RECORD_LEN {
        return Err(ReadError::Unsupported {
            version,
            record_size,
        });
    }
    let flow = u32::from_be_bytes([header[12], header[13], header[14], header[15]]);
    let body = &bytes[FILE_HEADER_LEN..];
    let (chunks, remainder) = body.as_chunks::<RECORD_LEN>();
    let mut records = Vec::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let number = |from: usize| {
            let mut eight = [0_u8; 8];
            eight.copy_from_slice(&chunk[from..from + 8]);
            u64::from_be_bytes(eight)
        };
        let kind = Kind::from_byte(chunk[0]).ok_or(ReadError::UnknownKind {
            record: index,
            kind: chunk[0],
        })?;
        records.push(Record {
            kind,
            status: chunk[1],
            connection: u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
            t: number(8),
            a: number(16),
            b: number(24),
            c: number(32),
            d: number(40),
        });
    }
    if !remainder.is_empty() {
        return Err(ReadError::Truncated {
            records: records.len(),
            extra_bytes: remainder.len(),
        });
    }
    Ok(File { flow, records })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(kind: Kind, n: u64) -> Record {
        Record {
            kind,
            status: STATUS_OK,
            connection: u32::try_from(n).unwrap(),
            t: 1_000 * n,
            a: 2_000 * n,
            b: 3_000 * n,
            c: 4_000 * n,
            d: 5_000 * n,
        }
    }

    fn file_of(flow: u32, records: &[Record]) -> Vec<u8> {
        let mut writer = Writer::new(Vec::new(), flow).unwrap();
        for record in records {
            writer.write(record).unwrap();
        }
        writer.finish().unwrap()
    }

    #[test]
    fn a_file_has_these_exact_bytes() {
        let bytes = file_of(
            7,
            &[Record {
                kind: Kind::Request,
                status: STATUS_FAILED,
                connection: 5,
                t: 1,
                a: 2,
                b: 3,
                c: 4,
                d: 5,
            }],
        );
        let mut expected = b"NSSTREAM".to_vec();
        expected.extend([0, 1, 0, 48, 0, 0, 0, 7]);
        expected.extend([4, 2, 0, 0, 0, 0, 0, 5]);
        for n in 1_u8..=5 {
            expected.extend([0, 0, 0, 0, 0, 0, 0, n]);
        }
        assert_eq!(bytes, expected);
    }

    #[test]
    fn records_of_every_kind_survive_the_file() {
        let records: Vec<Record> = [Kind::Progress, Kind::Info, Kind::Connection, Kind::Request]
            .into_iter()
            .cycle()
            .zip(0..40)
            .map(|(kind, n)| record(kind, n))
            .collect();
        let file = read(&file_of(3, &records)[..]).unwrap();
        assert_eq!(file.flow, 3);
        assert_eq!(file.records, records);
    }

    #[test]
    fn an_empty_file_is_a_valid_file_without_records() {
        assert_eq!(read(&file_of(0, &[])[..]).unwrap().records, []);
    }

    /// Canary: a cut-off file is an error and not a shorter file, because a lost record would be a number that is missing.
    #[test]
    fn a_file_that_ends_inside_a_record_is_an_error() {
        let bytes = file_of(0, &[record(Kind::Info, 1), record(Kind::Info, 2)]);
        let error = read(&bytes[..bytes.len() - 5]).unwrap_err();
        assert!(
            matches!(
                error,
                ReadError::Truncated {
                    records: 1,
                    extra_bytes: 43
                }
            ),
            "{error}"
        );
    }

    /// Canary: a kind that is not in this version stops the reading and names the record.
    #[test]
    fn a_record_of_an_unknown_kind_is_an_error() {
        let mut bytes = file_of(0, &[record(Kind::Info, 1), record(Kind::Info, 2)]);
        bytes[FILE_HEADER_LEN + RECORD_LEN] = 9;
        let error = read(&bytes[..]).unwrap_err();
        assert!(
            matches!(error, ReadError::UnknownKind { record: 1, kind: 9 }),
            "{error}"
        );
        bytes[FILE_HEADER_LEN] = 0;
        assert!(matches!(
            read(&bytes[..]).unwrap_err(),
            ReadError::UnknownKind { record: 0, kind: 0 }
        ));
    }

    #[test]
    fn other_files_are_refused_and_say_why() {
        assert!(matches!(
            read(&b"not a stream file at all"[..]).unwrap_err(),
            ReadError::NotAStreamFile
        ));
        assert!(matches!(
            read(&b"short"[..]).unwrap_err(),
            ReadError::NotAStreamFile
        ));
        // A file of UDP records is another format and is named as not this one
        assert!(matches!(
            read(&b"NSRECORD\0\x01\0\x18\0\0\0\0"[..]).unwrap_err(),
            ReadError::NotAStreamFile
        ));
        let mut bytes = file_of(0, &[record(Kind::Info, 1)]);
        bytes[9] = 2;
        let error = read(&bytes[..]).unwrap_err();
        assert!(matches!(
            error,
            ReadError::Unsupported {
                version: 2,
                record_size: 48
            }
        ));
        assert!(error.to_string().contains("version 2"));
    }

    #[test]
    fn the_writer_counts() {
        let mut writer = Writer::new(Vec::new(), 0).unwrap();
        writer.write(&record(Kind::Progress, 1)).unwrap();
        writer.write(&record(Kind::Progress, 2)).unwrap();
        assert_eq!(writer.count(), 2);
    }
}
