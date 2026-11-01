//! The per-packet export as JSON Lines (RPT-01): one line for each packet of a workload, in the order of the
//! sequence numbers.
//!
//! ```text
//! {"workload":"media","sequence":0,"send_ns":0,"receive_ns":20016000,"delay_ns":20016000}
//! {"workload":"media","sequence":1,"lost":true}
//! {"workload":"media","sequence":4,"lost":true,"corrupted":true}
//! {"workload":"media","sequence":2,"duplicate":true,"send_ns":9600000,"receive_ns":30000000,"delay_ns":20400000}
//! ```
//!
//! The times are nanoseconds since the first packet of the workload was sent, so that they mean something
//! outside the machine. A packet that did not arrive has only `lost`. A second arrival of a packet follows its
//! first line with `duplicate`. A lost packet that the receiving node saw on the wire and dropped as damaged has
//! `corrupted` as well (D-040). A packet that the kernel refused to take (the interface was down) has only
//! `refused`: it was never sent, so it is not lost (D-064).

use std::io::{self, Write};

use netscenario_agent::records::Record;
use serde::Serialize;

// The flags of a line of JSON: each is written only when it is true
#[allow(clippy::struct_excessive_bools)]
#[derive(Serialize)]
struct Line<'a> {
    workload: &'a str,
    sequence: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    lost: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    refused: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    corrupted: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    duplicate: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    unexpected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    send_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    receive_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    delay_ns: Option<u64>,
}

/// Writes the lines for a workload that sent `sent` packets and recorded `records`, counting times from
/// `start_ns`.
///
/// # Errors
///
/// The error of `out`.
pub fn write_packets(
    out: impl Write,
    workload: &str,
    sent: u64,
    records: &[Record],
    start_ns: u64,
) -> io::Result<()> {
    write_packets_marked(out, workload, sent, records, start_ns, &[])
}

/// As [`write_packets`], and the lost packets whose sequence numbers are in `damaged` (ascending, once each) are
/// marked `corrupted`.
///
/// # Errors
///
/// The error of `out`.
pub fn write_packets_marked(
    out: impl Write,
    workload: &str,
    sent: u64,
    records: &[Record],
    start_ns: u64,
    damaged: &[u64],
) -> io::Result<()> {
    write_packets_refused(out, workload, sent, &[], records, start_ns, damaged)
}

/// As [`write_packets_marked`], for a sender that used `slots` sequence numbers of which the kernel refused those in
/// `refused` (runs `[first, last]`, ascending and apart): they get a line with `refused` and nothing else.
///
/// # Errors
///
/// The error of `out`.
pub fn write_packets_refused(
    mut out: impl Write,
    workload: &str,
    slots: u64,
    refused: &[[u64; 2]],
    records: &[Record],
    start_ns: u64,
    damaged: &[u64],
) -> io::Result<()> {
    let sent = slots;
    // The arrivals in the order of the sequence numbers, and in the order of arrival for the same number
    let mut order: Vec<usize> = (0..records.len()).collect();
    order.sort_by_key(|&index| (records[index].sequence, index));
    let mut write = |line: &Line<'_>| -> io::Result<()> {
        serde_json::to_writer(&mut out, line).map_err(io::Error::from)?;
        out.write_all(b"\n")
    };
    let relative = |ns: u64| ns.saturating_sub(start_ns);
    let arrived = |record: &Record, duplicate: bool, unexpected: bool| Line {
        workload,
        sequence: record.sequence,
        lost: false,
        refused: false,
        corrupted: false,
        duplicate,
        unexpected,
        send_ns: Some(relative(record.send_ns)),
        receive_ns: Some(relative(record.receive_ns)),
        delay_ns: Some(record.delay_ns()),
    };
    let mut next = 0_usize;
    for sequence in 0..sent {
        let place = refused.partition_point(|[_, last]| *last < sequence);
        if refused
            .get(place)
            .is_some_and(|[first, _]| *first <= sequence)
        {
            write(&Line {
                workload,
                sequence,
                lost: false,
                refused: true,
                corrupted: false,
                duplicate: false,
                unexpected: false,
                send_ns: None,
                receive_ns: None,
                delay_ns: None,
            })?;
            // An arrival with the number of a packet that was never sent cannot happen; it is written as unexpected
            while next < order.len() && records[order[next]].sequence == sequence {
                write(&arrived(&records[order[next]], false, true))?;
                next += 1;
            }
            continue;
        }
        let mut first = true;
        while next < order.len() && records[order[next]].sequence == sequence {
            write(&arrived(&records[order[next]], !first, false))?;
            first = false;
            next += 1;
        }
        if first {
            write(&Line {
                workload,
                sequence,
                lost: true,
                refused: false,
                corrupted: damaged.binary_search(&sequence).is_ok(),
                duplicate: false,
                unexpected: false,
                send_ns: None,
                receive_ns: None,
                delay_ns: None,
            })?;
        }
    }
    // What is left has a sequence number the sender never used
    for &index in &order[next..] {
        write(&arrived(&records[index], false, true))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A refused packet has a line of its own that says so, is not lost, and the packets after it keep their numbers.
    #[test]
    fn a_refused_packet_is_written_as_refused_and_not_as_lost() {
        let mut out = Vec::new();
        write_packets_refused(
            &mut out,
            "w",
            5,
            &[[1, 2]],
            &[record(0, 0, 10), record(4, 40, 50)],
            0,
            &[],
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5, "{text}");
        assert_eq!(lines[1], r#"{"workload":"w","sequence":1,"refused":true}"#);
        assert_eq!(lines[2], r#"{"workload":"w","sequence":2,"refused":true}"#);
        assert!(lines[3].contains(r#""sequence":3"#) && lines[3].contains(r#""lost":true"#));
        assert!(!text.contains(r#""sequence":1,"lost""#));
    }

    fn record(sequence: u64, send: u64, receive: u64) -> Record {
        Record {
            sequence,
            send_ns: send,
            receive_ns: receive,
        }
    }

    fn lines(sent: u64, records: &[Record]) -> Vec<String> {
        let mut out = Vec::new();
        write_packets(&mut out, "media", sent, records, 1_000).unwrap();
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn arrivals_lost_packets_and_duplicates_are_told_apart() {
        let records = [
            record(2, 1_000 + 20, 1_000 + 70),
            record(0, 1_000, 1_000 + 50),
            record(2, 1_000 + 20, 1_000 + 90),
        ];
        assert_eq!(
            lines(3, &records),
            [
                r#"{"workload":"media","sequence":0,"send_ns":0,"receive_ns":50,"delay_ns":50}"#,
                r#"{"workload":"media","sequence":1,"lost":true}"#,
                r#"{"workload":"media","sequence":2,"send_ns":20,"receive_ns":70,"delay_ns":50}"#,
                r#"{"workload":"media","sequence":2,"duplicate":true,"send_ns":20,"receive_ns":90,"delay_ns":70}"#,
            ]
        );
    }

    #[test]
    fn a_number_the_sender_never_used_is_marked() {
        let out = lines(1, &[record(0, 1_000, 1_001), record(5, 1_000, 1_002)]);
        assert_eq!(out.len(), 2);
        assert!(
            out[1].contains(r#""sequence":5,"unexpected":true"#),
            "{}",
            out[1]
        );
    }

    /// Canary: when nothing arrived, every packet is a line that says lost, so a missing file cannot pass for a
    /// perfect run.
    #[test]
    fn a_workload_without_arrivals_is_all_lost() {
        let out = lines(3, &[]);
        assert_eq!(out.len(), 3);
        assert!(out.iter().all(|l| l.contains(r#""lost":true"#)));
        assert_eq!(lines(0, &[]), Vec::<String>::new());
    }
}
