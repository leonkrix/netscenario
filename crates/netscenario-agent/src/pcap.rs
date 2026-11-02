//! The file format of a capture (PRB-09, D-047, D-073): classic pcap with nanosecond timestamps, linktype Ethernet.
//!
//! The header is 24 bytes (magic `0xa1b23c4d`, version 2.4, no time zone, snaplen 262,144, linktype 1) and each frame has a
//! record header of 16 bytes (seconds, nanoseconds, captured length, original length). The numbers are little-endian, so
//! the magic tells a reader both the byte order and the unit of the timestamps. `tcpdump`, Wireshark and `tshark` read it
//! without a warning. [`Reader`] reads it back, and refuses a file that ends inside a record: a capture that was cut short
//! must not pass for a whole one.

use std::io::{self, Read, Write};

/// The magic of a pcap file with nanosecond timestamps, as written in a little-endian file.
pub const MAGIC_NANOSECONDS: u32 = 0xa1b2_3c4d;
/// The length of the header of the file.
pub const HEADER_LEN: usize = 24;
/// The length of the header of a record.
pub const RECORD_HEADER_LEN: usize = 16;
/// The largest frame that a record may hold; the reader refuses more, which is the sign of a file that is not a pcap.
pub const SNAPLEN: u32 = 262_144;
/// The linktype of Ethernet.
pub const LINKTYPE_ETHERNET: u32 = 1;

/// Writes the header of the file.
///
/// # Errors
///
/// The error of the writer.
pub fn write_header(out: &mut impl Write) -> io::Result<()> {
    out.write_all(&MAGIC_NANOSECONDS.to_le_bytes())?;
    out.write_all(&[2, 0, 4, 0])?;
    out.write_all(&[0; 8])?;
    out.write_all(&SNAPLEN.to_le_bytes())?;
    out.write_all(&LINKTYPE_ETHERNET.to_le_bytes())
}

/// Writes one frame with the time `seconds` and `nanoseconds` since the Unix epoch.
///
/// # Errors
///
/// The error of the writer.
pub fn write_record(
    out: &mut impl Write,
    seconds: u32,
    nanoseconds: u32,
    frame: &[u8],
) -> io::Result<()> {
    let length = u32::try_from(frame.len()).unwrap_or(u32::MAX);
    out.write_all(&seconds.to_le_bytes())?;
    out.write_all(&nanoseconds.to_le_bytes())?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(&length.to_le_bytes())?;
    out.write_all(frame)
}

/// A frame read from a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The time in nanoseconds since the Unix epoch.
    pub time_ns: u64,
    /// The bytes of the frame, Ethernet header first.
    pub bytes: Vec<u8>,
}

/// What is wrong with a file.
#[derive(Debug)]
pub enum Error {
    /// The reader of the file failed.
    Io(io::Error),
    /// Not a pcap file with nanosecond timestamps in the byte order and linktype that this program writes.
    Header(String),
    /// The file ends inside a record: the capture was cut short.
    Truncated {
        /// How many whole records came before.
        records: u64,
    },
    /// A record that cannot be right (a length above the snaplen, or captured and original lengths that differ).
    Record {
        /// The number of the record, from 0.
        number: u64,
        /// What is wrong.
        why: String,
    },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "cannot read the capture: {error}"),
            Self::Header(why) => write!(f, "not a capture of this program: {why}"),
            Self::Truncated { records } => write!(
                f,
                "the capture ends inside a record, after {records} whole records"
            ),
            Self::Record { number, why } => write!(f, "record {number}: {why}"),
        }
    }
}

impl std::error::Error for Error {}

/// Reads the frames of a file one by one.
#[derive(Debug)]
pub struct Reader<R: Read> {
    input: R,
    records: u64,
}

impl<R: Read> Reader<R> {
    /// Opens a file: reads and checks its header.
    ///
    /// # Errors
    ///
    /// [`Error::Header`] for a file that is not a pcap of this program, [`Error::Io`] if it cannot be read.
    pub fn new(mut input: R) -> Result<Self, Error> {
        let mut header = [0_u8; HEADER_LEN];
        input.read_exact(&mut header).map_err(|error| {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                Error::Header("the file is shorter than the header of a pcap file".to_owned())
            } else {
                Error::Io(error)
            }
        })?;
        let number =
            |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap_or([0; 4]));
        if number(0) != MAGIC_NANOSECONDS {
            return Err(Error::Header(format!(
                "the magic is {:#010x}, not {MAGIC_NANOSECONDS:#010x} (little-endian, nanoseconds)",
                number(0)
            )));
        }
        if header[4..8] != [2, 0, 4, 0] {
            return Err(Error::Header("the version is not 2.4".to_owned()));
        }
        if number(20) != LINKTYPE_ETHERNET {
            return Err(Error::Header(format!(
                "the linktype is {}, not Ethernet",
                number(20)
            )));
        }
        Ok(Self { input, records: 0 })
    }

    /// How many whole records were read.
    #[must_use]
    pub fn records(&self) -> u64 {
        self.records
    }
}

impl<R: Read> Iterator for Reader<R> {
    type Item = Result<Frame, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut header = [0_u8; RECORD_HEADER_LEN];
        // A file that ends exactly between two records is whole; one that ends inside a header is not
        let mut got = 0;
        while got < RECORD_HEADER_LEN {
            match self.input.read(&mut header[got..]) {
                Ok(0) if got == 0 => return None,
                Ok(0) => {
                    return Some(Err(Error::Truncated {
                        records: self.records,
                    }));
                }
                Ok(n) => got += n,
                Err(error) => return Some(Err(Error::Io(error))),
            }
        }
        let number =
            |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap_or([0; 4]));
        let (seconds, nanoseconds, captured, original) =
            (number(0), number(4), number(8), number(12));
        if captured > SNAPLEN || captured != original || nanoseconds >= 1_000_000_000 {
            return Some(Err(Error::Record {
                number: self.records,
                why: format!(
                    "captured {captured} bytes of {original}, at {seconds}.{nanoseconds:09}"
                ),
            }));
        }
        let mut bytes = vec![0_u8; captured as usize];
        if let Err(error) = self.input.read_exact(&mut bytes) {
            return Some(Err(if error.kind() == io::ErrorKind::UnexpectedEof {
                Error::Truncated {
                    records: self.records,
                }
            } else {
                Error::Io(error)
            }));
        }
        self.records += 1;
        Some(Ok(Frame {
            time_ns: u64::from(seconds) * 1_000_000_000 + u64::from(nanoseconds),
            bytes,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(frames: &[(u32, u32, &[u8])]) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_header(&mut bytes).unwrap();
        for (seconds, nanoseconds, frame) in frames {
            write_record(&mut bytes, *seconds, *nanoseconds, frame).unwrap();
        }
        bytes
    }

    #[test]
    fn frames_come_back_with_their_times_to_the_nanosecond() {
        let bytes = file(&[
            (1_700_000_000, 123_456_789, b"abc"),
            (1_700_000_001, 0, b""),
        ]);
        let mut reader = Reader::new(bytes.as_slice()).unwrap();
        let frames: Vec<Frame> = reader.by_ref().map(Result::unwrap).collect();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].time_ns, 1_700_000_000_123_456_789);
        assert_eq!(frames[0].bytes, b"abc");
        assert_eq!(frames[1].time_ns, 1_700_000_001_000_000_000);
        assert_eq!(reader.records(), 2);
    }

    #[test]
    fn the_header_is_the_one_that_tcpdump_reads() {
        let mut bytes = Vec::new();
        write_header(&mut bytes).unwrap();
        assert_eq!(bytes.len(), HEADER_LEN);
        // Magic for nanoseconds in a little-endian file, version 2.4, snaplen 262144, Ethernet
        assert_eq!(&bytes[..4], &[0x4d, 0x3c, 0xb2, 0xa1]);
        assert_eq!(&bytes[4..8], &[2, 0, 4, 0]);
        assert_eq!(&bytes[16..20], &262_144_u32.to_le_bytes());
        assert_eq!(&bytes[20..24], &[1, 0, 0, 0]);
    }

    /// Canary: a capture that was cut short is an error at every length inside a record, and only a cut between records is a
    /// whole file.
    #[test]
    fn a_file_that_is_cut_short_is_refused_wherever_it_is_cut() {
        let whole = file(&[(1, 1, &[7; 40]), (2, 2, &[8; 60])]);
        let first_end = HEADER_LEN + RECORD_HEADER_LEN + 40;
        for cut in HEADER_LEN..whole.len() {
            let mut reader = Reader::new(&whole[..cut]).unwrap();
            let results: Vec<_> = reader.by_ref().collect();
            let complete = cut == HEADER_LEN || cut == first_end;
            assert_eq!(
                results.iter().all(Result::is_ok),
                complete,
                "cut at {cut} of {}",
                whole.len()
            );
            if !complete {
                let error = results.last().unwrap().as_ref().unwrap_err();
                assert!(matches!(error, Error::Truncated { .. }), "{error}");
            }
        }
        // Cut inside the header of the file
        assert!(matches!(Reader::new(&whole[..10]), Err(Error::Header(_))));
    }

    #[test]
    fn a_file_of_another_kind_is_named_as_such() {
        let mut other = file(&[]);
        other[0] ^= 1;
        assert!(
            Reader::new(other.as_slice())
                .unwrap_err()
                .to_string()
                .contains("magic")
        );
        let mut wifi = file(&[]);
        wifi[20] = 105;
        assert!(
            Reader::new(wifi.as_slice())
                .unwrap_err()
                .to_string()
                .contains("linktype")
        );
        // A record that says it holds more than the snaplen is not a record
        let mut bytes = file(&[(1, 1, b"x")]);
        bytes[HEADER_LEN + 8..HEADER_LEN + 12].copy_from_slice(&(SNAPLEN + 1).to_le_bytes());
        bytes[HEADER_LEN + 12..HEADER_LEN + 16].copy_from_slice(&(SNAPLEN + 1).to_le_bytes());
        let error = Reader::new(bytes.as_slice())
            .unwrap()
            .next()
            .unwrap()
            .unwrap_err();
        assert!(matches!(error, Error::Record { number: 0, .. }));
    }
}
