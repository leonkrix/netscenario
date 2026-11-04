//! The trace of a `trace-replay` workload: when each packet leaves and how large it is (WRK-03, D-085).
//!
//! In a scenario file a trace is named by a file, `trace: { file: call.csv }` or `trace: { file: call.pcap }`. The file is read
//! when the scenario is loaded (D-083), and what is made of it is written into the scenario: the moments and the sizes of the
//! packets, packed (a LEB128 number for the time since the packet before and one for the size, as hexadecimal), with the SHA-256
//! of the file. A recorded scenario replays without the file.

use std::fmt::Write as _;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::units::{Duration, Size};

/// The most packets in a trace. A scenario holds the trace, and a trace of this many is about a megabyte of text.
pub const MAX_TRACE_PACKETS: usize = 100_000;

/// The format of the file of a trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TraceFormat {
    /// A text file with one packet on each line: the time since the start and the payload, each with its unit
    /// (`12.5ms,1200b`).
    Csv,
    /// A capture of the packets of a real flow (`pcap` or `pcapng`): the transport payload of the packets of one flow, at the
    /// moments they were captured.
    Pcap,
}

impl TraceFormat {
    /// The word of the file.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Pcap => "pcap",
        }
    }

    /// The format that the name of a file suggests.
    #[must_use]
    pub fn of_file(name: &str) -> Option<Self> {
        let extension = std::path::Path::new(name)
            .extension()?
            .to_string_lossy()
            .to_ascii_lowercase();
        match extension.as_str() {
            "csv" | "txt" => Some(Self::Csv),
            "pcap" | "pcapng" | "cap" => Some(Self::Pcap),
            _ => None,
        }
    }
}

/// The transport protocol of a flow in a capture.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum FlowProtocol {
    /// UDP.
    Udp,
    /// TCP.
    Tcp,
}

/// Which flow of a capture is the trace: the packets that go from `src` to `dst`. What is left out is not asked about. When the
/// capture has only one flow nothing needs to be said; when it has several, the message lists them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FlowSelection {
    /// The address the packets come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<std::net::IpAddr>,
    /// The address the packets go to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst: Option<std::net::IpAddr>,
    /// The port the packets come from.
    #[serde(default, rename = "src-port", skip_serializing_if = "Option::is_none")]
    pub src_port: Option<u16>,
    /// The port the packets go to.
    #[serde(default, rename = "dst-port", skip_serializing_if = "Option::is_none")]
    pub dst_port: Option<u16>,
    /// `udp` or `tcp`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<FlowProtocol>,
}

/// The trace of a workload: the file it comes from, and, once the file is read, the packets.
///
/// In a scenario file only `file` is written (and `format` and `select` for a capture). When the scenario is loaded the rest is
/// filled in. `sha256` may be written as well: then the file must have exactly that content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PacketTrace {
    /// The file, a path relative to the directory of the scenario file. Read when the scenario is loaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// `csv` or `pcap`. Left out, it is taken from the name of the file (`.csv`, `.txt`, `.pcap`, `.pcapng`, `.cap`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<TraceFormat>,
    /// For a capture with more than one flow: which one is the trace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<FlowSelection>,
    /// The SHA-256 of the file. Written by the loader; if it is in the file, the content must match it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// How many packets of a capture were raised to the smallest payload that a packet of the agent can have (it carries a
    /// header), such as an acknowledgement without data. Written by the loader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub padded: Option<u32>,
    /// The packets. Written by the loader; a scenario with them needs no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packets: Option<TraceData>,
}

impl PacketTrace {
    /// A reference to a file that is not read yet.
    #[must_use]
    pub fn from_file(file: &str) -> Self {
        Self {
            file: Some(file.to_owned()),
            format: None,
            select: None,
            sha256: None,
            padded: None,
            packets: None,
        }
    }

    /// A trace that holds its packets, without a file.
    #[must_use]
    pub fn from_data(data: TraceData) -> Self {
        Self {
            file: None,
            format: None,
            select: None,
            sha256: None,
            padded: None,
            packets: Some(data),
        }
    }

    /// Whether the packets are there.
    #[must_use]
    pub fn is_resolved(&self) -> bool {
        self.packets.is_some()
    }

    /// The format of the file: the one written, or the one the name suggests.
    #[must_use]
    pub fn format_of_file(&self) -> Option<TraceFormat> {
        self.format
            .or_else(|| self.file.as_deref().and_then(TraceFormat::of_file))
    }
}

/// The moments and the sizes of the packets of a trace.
#[derive(Clone, PartialEq, Eq)]
pub struct TraceData {
    moments_ns: Vec<u64>,
    sizes: Vec<u32>,
}

impl std::fmt::Debug for TraceData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TraceData({} packets)", self.moments_ns.len())
    }
}

impl TraceData {
    /// A trace of packets that leave at `moments_ns` (from the start, not falling) with the payloads `sizes`.
    ///
    /// # Errors
    ///
    /// A sentence if the lists differ in length, are empty or longer than [`MAX_TRACE_PACKETS`], if a moment is before the one
    /// before it, or a size is zero.
    pub fn new(moments_ns: Vec<u64>, sizes: Vec<u32>) -> Result<Self, String> {
        if moments_ns.len() != sizes.len() {
            return Err(format!(
                "{} moments and {} sizes",
                moments_ns.len(),
                sizes.len()
            ));
        }
        if moments_ns.is_empty() {
            return Err("the trace has no packets".to_owned());
        }
        if moments_ns.len() > MAX_TRACE_PACKETS {
            return Err(format!(
                "{} packets are more than the {MAX_TRACE_PACKETS} that a trace may have",
                moments_ns.len()
            ));
        }
        if let Some(at) = moments_ns.windows(2).position(|pair| pair[1] < pair[0]) {
            return Err(format!(
                "packet {} leaves before packet {at}: the moments must not fall",
                at + 1
            ));
        }
        if let Some(at) = sizes.iter().position(|size| *size == 0) {
            return Err(format!("packet {at} has no payload"));
        }
        Ok(Self { moments_ns, sizes })
    }

    /// How many packets.
    #[must_use]
    pub fn len(&self) -> usize {
        self.moments_ns.len()
    }

    /// Whether there are no packets (never, for a trace made with [`TraceData::new`]).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.moments_ns.is_empty()
    }

    /// The moments, in nanoseconds from the start.
    #[must_use]
    pub fn moments_ns(&self) -> &[u64] {
        &self.moments_ns
    }

    /// The payloads, in bytes.
    #[must_use]
    pub fn sizes(&self) -> &[u32] {
        &self.sizes
    }

    /// The largest payload in bytes.
    #[must_use]
    pub fn largest(&self) -> u64 {
        self.sizes.iter().copied().max().map_or(0, u64::from)
    }

    /// The smallest payload in bytes.
    #[must_use]
    pub fn smallest(&self) -> u64 {
        self.sizes.iter().copied().min().map_or(0, u64::from)
    }

    /// The moment of the last packet.
    #[must_use]
    pub fn span_ns(&self) -> u64 {
        self.moments_ns.last().copied().unwrap_or(0)
    }

    fn pack(&self) -> String {
        let mut bytes = Vec::with_capacity(self.len() * 4);
        let mut before = 0_u64;
        for (moment, size) in self.moments_ns.iter().zip(&self.sizes) {
            leb128_write(&mut bytes, moment - before);
            leb128_write(&mut bytes, u64::from(*size));
            before = *moment;
        }
        let mut text = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            let _ = write!(text, "{byte:02x}");
        }
        text
    }

    fn unpack(text: &str) -> Result<Self, String> {
        if !text.len().is_multiple_of(2) || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("the packets of a trace are hexadecimal digits in pairs".to_owned());
        }
        let bytes: Vec<u8> = text
            .as_bytes()
            .chunks(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).unwrap_or("00"), 16).unwrap_or(0)
            })
            .collect();
        let (mut moments, mut sizes) = (Vec::new(), Vec::new());
        let (mut at, mut moment) = (0_usize, 0_u64);
        while at < bytes.len() {
            let gap = leb128_read(&bytes, &mut at)?;
            let size = leb128_read(&bytes, &mut at)?;
            moment = moment
                .checked_add(gap)
                .ok_or_else(|| "a moment of the trace is too large".to_owned())?;
            moments.push(moment);
            sizes.push(
                u32::try_from(size).map_err(|_| "a size of the trace is too large".to_owned())?,
            );
            if moments.len() > MAX_TRACE_PACKETS {
                return Err(format!("more than {MAX_TRACE_PACKETS} packets"));
            }
        }
        Self::new(moments, sizes)
    }
}

fn leb128_write(bytes: &mut Vec<u8>, mut value: u64) {
    loop {
        // The low seven bits; the top bit says that more follow
        let low = u8::try_from(value & 0x7f).unwrap_or(0);
        value >>= 7;
        if value == 0 {
            bytes.push(low);
            return;
        }
        bytes.push(low | 0x80);
    }
}

fn leb128_read(bytes: &[u8], at: &mut usize) -> Result<u64, String> {
    let (mut value, mut shift) = (0_u64, 0_u32);
    loop {
        let byte = *bytes
            .get(*at)
            .ok_or_else(|| "the packets of the trace end in the middle of a number".to_owned())?;
        *at += 1;
        if shift >= 64 || (shift == 63 && byte & 0x7e != 0) {
            return Err("a number of the trace is too large".to_owned());
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
}

impl Serialize for TraceData {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.pack())
    }
}

impl<'de> Deserialize<'de> for TraceData {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::unpack(&text).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for TraceData {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "TraceData".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "pattern": "^([0-9a-fA-F]{2})+$",
            "description": "The packets of a trace: for each packet the time since the packet before and the payload in bytes, each as a LEB128 number, written as hexadecimal. Written by the loader from the file of the trace."
        })
    }
}

/// What a file of a trace gave: the packets, and how many of them had to be raised to the smallest payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadTrace {
    /// The packets.
    pub data: TraceData,
    /// How many packets were raised to the smallest payload.
    pub padded: u32,
}

/// Reads the CSV file of a trace: one packet on each line, `time,size`, each with its unit (`0ms,1200b`, `12.5ms,80b`). A first
/// line `time,size` is a header and is skipped; lines that start with `#` and empty lines are skipped. The time is since the start
/// of the workload and must not fall; a size is the payload of the UDP packet, from `smallest` bytes on.
///
/// # Errors
///
/// A sentence with the line if something is not a time and a size, a size is below `smallest`, or the packets do not make a trace.
pub fn read_csv(text: &str, smallest: u64) -> Result<ReadTrace, String> {
    let (mut moments, mut sizes) = (Vec::new(), Vec::new());
    let mut header = true;
    for (index, line) in text.lines().enumerate() {
        let number = index + 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        if header {
            header = false;
            if fields.len() == 2
                && fields[0].eq_ignore_ascii_case("time")
                && fields[1].eq_ignore_ascii_case("size")
            {
                continue;
            }
        }
        let [time, size] = fields.as_slice() else {
            return Err(format!(
                "line {number}: '{line}' is not 'time,size' (for example 12.5ms,1200b)"
            ));
        };
        let time =
            Duration::from_str(time).map_err(|why| format!("line {number}: the time {why}"))?;
        let size = Size::from_str(size).map_err(|why| format!("line {number}: the size {why}"))?;
        if size.bytes() < smallest {
            return Err(format!(
                "line {number}: a packet of {} bytes is below the smallest payload of {smallest} bytes (the packet carries the header of the agent)",
                size.bytes()
            ));
        }
        moments.push(time.as_nanos());
        sizes.push(
            u32::try_from(size.bytes())
                .map_err(|_| format!("line {number}: the size {size} is too large"))?,
        );
        if moments.len() > MAX_TRACE_PACKETS {
            return Err(format!(
                "more than {MAX_TRACE_PACKETS} packets (a trace may have at most that many)"
            ));
        }
    }
    let data = TraceData::new(moments, sizes)?;
    Ok(ReadTrace { data, padded: 0 })
}

impl crate::scenario::Scenario {
    /// The files that tables and traces of this scenario still have to read, each once, in the order of the scenario.
    #[must_use]
    pub fn unresolved_files(&self) -> Vec<String> {
        let mut files = self.unresolved_table_files();
        for workload in &self.workloads {
            if let crate::Workload::TraceReplay(replay) = workload
                && replay.trace.packets.is_none()
                && let Some(file) = &replay.trace.file
                && !files.contains(file)
            {
                files.push(file.clone());
            }
        }
        files
    }

    /// Fills in everything that refers to a file (D-083): the tables of the jitters ([`Scenario::resolve_tables`](crate::Scenario::resolve_tables)) and the traces of
    /// the `trace-replay` workloads ([`Scenario::resolve_traces`](crate::Scenario::resolve_traces)). `read` gives the content of a file by the path written in the
    /// scenario. Returns the problems of both; the scenario is left as it was when there are any.
    pub fn resolve_files(
        &mut self,
        mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
    ) -> Vec<crate::error::Problem> {
        let mut work = self.clone();
        let mut problems = work.resolve_tables(&mut read);
        problems.extend(work.resolve_traces(&mut read));
        if problems.is_empty() {
            *self = work;
        }
        problems
    }

    /// Reads the files of the traces of the `trace-replay` workloads (D-085) and puts the packets, the number of packets that a
    /// capture had to raise to the smallest payload and the SHA-256 of the file into the scenario.
    ///
    /// Returns the problems found, with the path of each file in the scenario; the scenario is left as it was when there are any.
    pub fn resolve_traces(
        &mut self,
        mut read: impl FnMut(&str) -> Result<Vec<u8>, String>,
    ) -> Vec<crate::error::Problem> {
        use crate::error::Problem;
        let mut problems = Vec::new();
        let mut resolved = self.clone();
        for (index, workload) in resolved.workloads.iter_mut().enumerate() {
            let crate::Workload::TraceReplay(replay) = workload else {
                continue;
            };
            let smallest = if replay.udp_checksum.is_on() {
                crate::MIN_PACKET_SIZE
            } else {
                crate::MIN_PACKET_SIZE_WITHOUT_CHECKSUM
            };
            let trace = &mut replay.trace;
            let (Some(file), None) = (trace.file.clone(), &trace.packets) else {
                continue;
            };
            let Some(format) = trace.format_of_file() else {
                continue;
            };
            let path = format!("workloads[{index}].trace.file");
            let fail = |why: String| Problem::new(why).at_path(path.clone());
            let bytes = match read(&file) {
                Ok(bytes) => bytes,
                Err(why) => {
                    problems.push(fail(why));
                    continue;
                }
            };
            let hash = crate::hash::sha256_hex(&bytes);
            if let Some(pinned) = &trace.sha256
                && !pinned.eq_ignore_ascii_case(&hash)
            {
                problems.push(
                    fail(format!(
                        "'{file}' is not the file the scenario was written for: its SHA-256 is {hash}, the scenario says {pinned}"
                    ))
                    .with_help("the file changed; check it, then write the new `sha256` or take it away"),
                );
                continue;
            }
            let made = match format {
                TraceFormat::Csv => String::from_utf8(bytes)
                    .map_err(|_| "it is not text in UTF-8".to_owned())
                    .and_then(|text| read_csv(&text, smallest)),
                TraceFormat::Pcap => crate::trace_pcap::read_capture(
                    &bytes,
                    &trace.select.clone().unwrap_or_default(),
                    smallest,
                ),
            };
            match made {
                Ok(read) => {
                    trace.format = Some(format);
                    trace.sha256 = Some(hash);
                    trace.padded = (read.padded > 0).then_some(read.padded);
                    trace.packets = Some(read.data);
                }
                Err(why) => problems.push(
                    fail(format!("'{file}': {why}")).with_help(match format {
                        TraceFormat::Csv => "a trace file has one packet on each line: the time and the payload, each with its unit (12.5ms,1200b)",
                        TraceFormat::Pcap => "a capture of the packets of one flow; `select` names the flow when the capture has several",
                    }),
                ),
            }
        }
        if problems.is_empty() {
            problems.extend(resolved.validate());
        }
        if problems.is_empty() {
            *self = resolved;
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_packets_are_packed_into_hexadecimal_and_unpacked_to_the_same_trace() {
        let data = TraceData::new(
            vec![0, 1_500, 1_500, 2_000_000_000, 90_000_000_000_000],
            vec![20, 1472, 16, 300, 65_507],
        )
        .unwrap();
        let json = serde_json::to_string(&data).unwrap();
        let back: TraceData = serde_json::from_str(&json).unwrap();
        assert_eq!(back, data);
        // Small numbers take a byte each: the first packet is `00 14`
        assert!(json.starts_with("\"0014"), "{json}");
    }

    #[test]
    fn a_trace_that_is_not_a_schedule_is_refused_with_the_reason() {
        assert!(TraceData::new(vec![0, 5], vec![10, 10]).is_ok());
        assert!(
            TraceData::new(vec![0, 5], vec![10])
                .unwrap_err()
                .contains("2 moments and 1 sizes")
        );
        assert!(
            TraceData::new(vec![], vec![])
                .unwrap_err()
                .contains("no packets")
        );
        assert!(
            TraceData::new(vec![0, 5, 4], vec![10, 10, 10])
                .unwrap_err()
                .contains("packet 2 leaves before packet 1")
        );
        assert!(
            TraceData::new(vec![0, 5], vec![10, 0])
                .unwrap_err()
                .contains("packet 1 has no payload")
        );
        let long = vec![0_u64; MAX_TRACE_PACKETS + 1];
        assert!(
            TraceData::new(long, vec![1; MAX_TRACE_PACKETS + 1])
                .unwrap_err()
                .contains("more than")
        );
    }

    #[test]
    fn packed_packets_that_are_cut_or_wrong_are_refused() {
        let cut: Result<TraceData, _> = serde_json::from_str("\"0080\"");
        assert!(
            cut.unwrap_err()
                .to_string()
                .contains("end in the middle of a number")
        );
        let odd: Result<TraceData, _> = serde_json::from_str("\"001\"");
        assert!(odd.unwrap_err().to_string().contains("in pairs"));
        let none: Result<TraceData, _> = serde_json::from_str("\"\"");
        assert!(none.unwrap_err().to_string().contains("no packets"));
        // Ten bytes that all say "more follows" are a number that does not fit
        let big: Result<TraceData, _> = serde_json::from_str(&format!("\"{}\"", "ff".repeat(11)));
        assert!(big.unwrap_err().to_string().contains("too large"));
    }

    #[test]
    fn a_csv_file_is_read_with_units_and_a_header() {
        let text = "# a call\ntime,size\n0ms,100b\n20ms, 1200b\n20.5ms,16b\n1s,1.4kb\n";
        let read = read_csv(text, 16).unwrap();
        assert_eq!(
            read.data.moments_ns(),
            [0, 20_000_000, 20_500_000, 1_000_000_000]
        );
        assert_eq!(read.data.sizes(), [100, 1200, 16, 1400]);
        assert_eq!(read.padded, 0);
        // Without the header it is the same
        assert_eq!(
            read_csv("0ms,100b\n20ms,1200b\n20.5ms,16b\n1s,1.4kb\n", 16).unwrap(),
            read
        );
    }

    /// Each fault is named by its line, and each is a canary: the same file without it is read.
    #[test]
    fn a_csv_file_with_a_fault_is_refused_with_the_line() {
        assert!(read_csv("0ms,100b\n", 16).is_ok());
        let error = |text: &str| read_csv(text, 16).unwrap_err();
        assert!(
            error("0ms,100b\n5,100b\n").contains("line 2: the time"),
            "{}",
            error("0ms,100b\n5,100b\n")
        );
        assert!(error("0ms,100\n").contains("line 1: the size"));
        assert!(error("0ms;100b\n").contains("line 1: '0ms;100b' is not 'time,size'"));
        assert!(error("0ms,100b,x\n").contains("is not 'time,size'"));
        assert!(
            error("0ms,8b\n")
                .contains("line 1: a packet of 8 bytes is below the smallest payload of 16")
        );
        assert!(error("5ms,100b\n1ms,100b\n").contains("packet 1 leaves before packet 0"));
        assert!(error("# nothing\n").contains("no packets"));
        // Below the smallest payload for a stream without the checksum, which carries more header
        assert!(read_csv("0ms,18b\n", 16).is_ok());
        assert!(
            read_csv("0ms,18b\n", 20)
                .unwrap_err()
                .contains("below the smallest payload of 20")
        );
    }
}
