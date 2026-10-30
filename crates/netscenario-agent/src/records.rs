//! The file of records that the receiver writes (the record format, version 1).
//!
//! One record for each packet that arrived. A packet that did not arrive has no record: its absence is the
//! loss, and the sender says how many it sent. All numbers are in network byte order.
//!
//! The file starts with a header of 16 bytes and continues with records of 24 bytes:
//!
//! | Bytes | Field | Meaning |
//! | --- | --- | --- |
//! | 0 to 7 | magic | `NSRECORD` |
//! | 8 to 9 | version | `1` |
//! | 10 to 11 | record size | `24` |
//! | 12 to 15 | flow | The number of the workload that the receiver was started for. |
//!
//! | Record bytes | Field | Meaning |
//! | --- | --- | --- |
//! | 0 to 7 | sequence | From the packet |
//! | 8 to 15 | send time | From the packet, `CLOCK_MONOTONIC` in nanoseconds |
//! | 16 to 23 | receive time | `CLOCK_MONOTONIC` in nanoseconds, taken right after the kernel handed the packet over |
//!
//! Records are in the order of arrival. A reordered packet has a lower sequence than the one before it.

use std::io::{self, Read, Write};

/// The length of the header of the file.
pub const FILE_HEADER_LEN: usize = 16;

/// The length of one record.
pub const RECORD_LEN: usize = 24;

const MAGIC: &[u8; 8] = b"NSRECORD";
const VERSION: u16 = 1;

/// One packet that arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    /// The number of the packet in its stream.
    pub sequence: u64,
    /// When it was sent, on `CLOCK_MONOTONIC`.
    pub send_ns: u64,
    /// When it arrived, on `CLOCK_MONOTONIC`.
    pub receive_ns: u64,
}

impl Record {
    /// The one-way delay in nanoseconds. Zero if the clocks disagree (a receive time before the send time),
    /// which cannot happen on one machine and would be a bug, not a measurement.
    #[must_use]
    pub fn delay_ns(&self) -> u64 {
        self.receive_ns.saturating_sub(self.send_ns)
    }
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
        bytes[0..8].copy_from_slice(&record.sequence.to_be_bytes());
        bytes[8..16].copy_from_slice(&record.send_ns.to_be_bytes());
        bytes[16..24].copy_from_slice(&record.receive_ns.to_be_bytes());
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
}

/// What a file contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// The workload that the receiver was started for.
    pub flow: u32,
    /// The records, in the order of arrival.
    pub records: Vec<Record>,
}

/// Why a file cannot be read.
#[derive(Debug)]
pub enum ReadError {
    /// The file cannot be read at all.
    Io(io::Error),
    /// It does not start with the magic of a record file.
    NotARecordFile,
    /// A version or a record size this program does not read.
    Unsupported {
        /// The version in the file.
        version: u16,
        /// The record size in the file.
        record_size: u16,
    },
    /// The last record is cut off: the receiver was killed while it wrote.
    Truncated {
        /// How many whole records precede the cut.
        records: usize,
        /// How many bytes of the cut record are there.
        extra_bytes: usize,
    },
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Io(error) => write!(f, "cannot read the record file: {error}"),
            ReadError::NotARecordFile => {
                write!(
                    f,
                    "this is not a record file (it does not start with NSRECORD)"
                )
            }
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
/// A [`ReadError`] if it is not a record file of version 1, or ends inside a record. A truncated file is an
/// error, not a shorter file: a lost record would look like a lost packet.
pub fn read(mut input: impl Read) -> Result<File, ReadError> {
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes)?;
    let Some(header) = bytes.get(..FILE_HEADER_LEN) else {
        return Err(ReadError::NotARecordFile);
    };
    if &header[0..8] != MAGIC {
        return Err(ReadError::NotARecordFile);
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
    let mut records = Vec::with_capacity(body.len() / RECORD_LEN);
    let (chunks, remainder) = body.as_chunks::<RECORD_LEN>();
    let rest = remainder.len();
    for chunk in chunks {
        let number = |from: usize| {
            let mut eight = [0_u8; 8];
            eight.copy_from_slice(&chunk[from..from + 8]);
            u64::from_be_bytes(eight)
        };
        records.push(Record {
            sequence: number(0),
            send_ns: number(8),
            receive_ns: number(16),
        });
    }
    if rest != 0 {
        return Err(ReadError::Truncated {
            records: records.len(),
            extra_bytes: rest,
        });
    }
    Ok(File { flow, records })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(sequence: u64) -> Record {
        Record {
            sequence,
            send_ns: 1_000 * sequence,
            receive_ns: 1_000 * sequence + 40_000,
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
                sequence: 1,
                send_ns: 2,
                receive_ns: 3,
            }],
        );
        assert_eq!(
            bytes,
            [
                b'N', b'S', b'R', b'E', b'C', b'O', b'R', b'D', 0, 1, 0, 24, 0, 0, 0,
                7, // header
                0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0,
                3, // record
            ]
        );
    }

    #[test]
    fn records_survive_the_file() {
        let records: Vec<Record> = (0..100).map(record).collect();
        let file = read(&file_of(3, &records)[..]).unwrap();
        assert_eq!(file.flow, 3);
        assert_eq!(file.records, records);
        assert_eq!(file.records[5].delay_ns(), 40_000);
    }

    #[test]
    fn an_empty_file_is_a_valid_file_without_records() {
        let file = read(&file_of(0, &[])[..]).unwrap();
        assert_eq!(file.records.len(), 0);
    }

    /// Canary: a cut-off file is an error and not a shorter file, because a lost record would look like a
    /// lost packet.
    #[test]
    fn a_file_that_ends_inside_a_record_is_an_error() {
        let bytes = file_of(0, &[record(1), record(2)]);
        let error = read(&bytes[..bytes.len() - 5]).unwrap_err();
        assert!(
            matches!(
                error,
                ReadError::Truncated {
                    records: 1,
                    extra_bytes: 19
                }
            ),
            "{error}"
        );
        assert!(error.to_string().contains("1 whole records"));
    }

    #[test]
    fn other_files_are_refused_and_say_why() {
        assert!(matches!(
            read(&b"not a record file at all"[..]).unwrap_err(),
            ReadError::NotARecordFile
        ));
        assert!(matches!(
            read(&b"short"[..]).unwrap_err(),
            ReadError::NotARecordFile
        ));
        let mut bytes = file_of(0, &[record(1)]);
        bytes[9] = 2;
        let error = read(&bytes[..]).unwrap_err();
        assert!(matches!(
            error,
            ReadError::Unsupported {
                version: 2,
                record_size: 24
            }
        ));
        assert!(error.to_string().contains("version 2"));
    }

    #[test]
    fn the_delay_never_goes_below_zero() {
        let record = Record {
            sequence: 0,
            send_ns: 10,
            receive_ns: 5,
        };
        assert_eq!(record.delay_ns(), 0);
    }

    #[test]
    fn the_writer_counts() {
        let mut writer = Writer::new(Vec::new(), 0).unwrap();
        writer.write(&record(1)).unwrap();
        writer.write(&record(2)).unwrap();
        assert_eq!(writer.count(), 2);
    }
}
