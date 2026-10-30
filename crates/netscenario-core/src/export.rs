//! The per-packet export as JSON Lines (RPT-01): one line for each packet of a workload, in the order of the
//! sequence numbers.
//!
//! ```text
//! {"workload":"media","sequence":0,"send_ns":0,"receive_ns":20016000,"delay_ns":20016000}
//! {"workload":"media","sequence":1,"lost":true}
//! {"workload":"media","sequence":2,"duplicate":true,"send_ns":9600000,"receive_ns":30000000,"delay_ns":20400000}
//! ```
//!
//! The times are nanoseconds since the first packet of the workload was sent, so that they mean something
//! outside the machine. A packet that did not arrive has only `lost`. A second arrival of a packet follows its
//! first line with `duplicate`.

use std::io::{self, Write};

use netscenario_agent::records::Record;
use serde::Serialize;

#[derive(Serialize)]
struct Line<'a> {
    workload: &'a str,
    sequence: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    lost: bool,
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
    mut out: impl Write,
    workload: &str,
    sent: u64,
    records: &[Record],
    start_ns: u64,
) -> io::Result<()> {
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
        duplicate,
        unexpected,
        send_ns: Some(relative(record.send_ns)),
        receive_ns: Some(relative(record.receive_ns)),
        delay_ns: Some(record.delay_ns()),
    };
    let mut next = 0_usize;
    for sequence in 0..sent {
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
