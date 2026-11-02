//! The captures of the interfaces of the links (`measure.capture`; PRB-09, D-047, D-073).
//!
//! For every link in the list, both ends: an agent in the node opens a packet socket on the interface and writes a pcap file
//! of the run directory, `capture-<link>-<node>.pcap`. They start before everything else of the run (the programs, the
//! receivers, the senders), so that no frame is missed, and stop after the packets have had the time to leave the links. Each
//! agent counts the frames of the interface while it runs and says how many of them are not in its file; the result has it.

use std::path::PathBuf;
use std::time::Instant;

use netscenario_agent::events::{CaptureSummary, Event};
use netscenario_model::Scenario;

use crate::backend::{Backend, ProgramHandle};
use crate::error::Error;
use crate::provision::Provisioned;
use crate::result::CaptureResult;
use crate::run::{
    Control, END_MARGIN, RunOptions, agent_program, event, failure, strings, wait_for_end,
    wait_for_line,
};

/// A capture that is going.
struct Open {
    link: String,
    node: String,
    interface: String,
    file: String,
    program: ProgramHandle,
    receive_buffer_bytes: u64,
}

/// The captures of a run.
#[derive(Default)]
pub(crate) struct Captures {
    open: Vec<Open>,
    summaries: Vec<(usize, CaptureSummary)>,
}

impl Captures {
    /// Starts the capture of both ends of each link of `measure.capture`, and waits until each one is open.
    ///
    /// # Errors
    ///
    /// [`Error::Run`] if an agent cannot capture (most likely `CAP_NET_RAW` is missing in the node) or does not say it is ready;
    /// [`Error::Backend`] and [`Error::Cancelled`].
    pub(crate) fn start(
        &mut self,
        backend: &mut dyn Backend,
        scenario: &Scenario,
        provisioned: &Provisioned,
        options: &RunOptions,
        control: &Control<'_>,
    ) -> Result<(), Error> {
        for id in &scenario.measure.capture {
            let (index, link) = scenario
                .topology
                .links
                .iter()
                .enumerate()
                .find(|(_, link)| link.id == *id)
                .ok_or_else(|| {
                    failure("capture a link", format!("the link '{id}' does not exist"))
                })?;
            let handle = provisioned
                .links
                .iter()
                .find(|(name, _)| name == id)
                .map(|(_, handle)| *handle)
                .ok_or_else(|| {
                    failure(
                        "capture a link",
                        format!("the link '{id}' was not built (the {index}th)"),
                    )
                })?;
            for end in 0..2 {
                control.checkpoint()?;
                let node_name = &link.between[end];
                let node = provisioned
                    .nodes
                    .iter()
                    .find(|(name, _)| name == node_name)
                    .map(|(_, handle)| *handle)
                    .ok_or_else(|| {
                        failure(
                            "capture a link",
                            format!("the node '{node_name}' was not built"),
                        )
                    })?;
                let interface = backend.interface_name(handle, end)?;
                self.start_one(
                    backend,
                    id.as_str(),
                    node_name.as_str(),
                    node,
                    &interface,
                    options,
                    control,
                )?;
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn start_one(
        &mut self,
        backend: &mut dyn Backend,
        link: &str,
        node_name: &str,
        node: crate::backend::NodeHandle,
        interface: &str,
        options: &RunOptions,
        control: &Control<'_>,
    ) -> Result<(), Error> {
        let file = format!("capture-{link}-{node_name}.pcap");
        let path: PathBuf = options.out_dir.join(&file);
        let flow = self.open.len();
        let arguments = strings(&[
            "capture",
            "--interface",
            interface,
            "--out",
            &path.display().to_string(),
            "--flow",
            &flow.to_string(),
        ]);
        let what = format!("capture link '{link}' in node '{node_name}'");
        let program = backend
            .start_program(node, &agent_program(options, &arguments))
            .map_err(|error| failure(what.clone(), error.to_string()))?;
        let first_line = wait_for_line(backend, program, options.ready_timeout, &what, control)?;
        let receive_buffer_bytes = match event(&first_line, &what)? {
            Event::CaptureReady {
                receive_buffer_bytes,
                ..
            } => receive_buffer_bytes,
            other => {
                return Err(failure(
                    what,
                    format!("expected the capture to be ready, the agent said {other:?}"),
                ));
            }
        };
        self.open.push(Open {
            link: link.to_owned(),
            node: node_name.to_owned(),
            interface: interface.to_owned(),
            file,
            program,
            receive_buffer_bytes,
        });
        Ok(())
    }

    /// Tells every capture to stop and reads what each one wrote.
    ///
    /// # Errors
    ///
    /// [`Error::Run`] if an agent does not end in time or ends without its summary.
    pub(crate) fn stop_all(
        &mut self,
        backend: &mut dyn Backend,
        control: &Control<'_>,
    ) -> Result<(), Error> {
        for (position, capture) in self.open.iter().enumerate() {
            control.checkpoint()?;
            let what = format!(
                "stop the capture of link '{}' in node '{}'",
                capture.link, capture.node
            );
            backend.send_line(capture.program, "stop")?;
            let lines = wait_for_end(
                backend,
                capture.program,
                Instant::now() + END_MARGIN,
                &what,
                control,
            )?;
            match lines.last().map(|line| event(line, &what)).transpose()? {
                Some(Event::Captured(summary)) => self.summaries.push((position, summary)),
                other => {
                    return Err(failure(
                        what,
                        format!("the capture should end with 'captured', it said {other:?}"),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Ends the captures at once, after an error.
    pub(crate) fn kill_all(&mut self, backend: &mut dyn Backend) {
        for capture in &self.open {
            let _ = backend.send_line(capture.program, "stop");
        }
    }

    /// The result of each capture, with times in nanoseconds from the start of the measured phase (`begin`, on
    /// `CLOCK_MONOTONIC`).
    pub(crate) fn into_results(self, begin: u64) -> Vec<CaptureResult> {
        let relative = |time_ns: u64, offset: i64| -> i64 {
            let monotonic = i128::from(time_ns) - i128::from(offset);
            i64::try_from(monotonic - i128::from(begin)).unwrap_or(i64::MAX)
        };
        self.summaries
            .into_iter()
            .map(|(position, summary)| {
                let open = &self.open[position];
                CaptureResult {
                    link: open.link.clone(),
                    node: open.node.clone(),
                    interface: open.interface.clone(),
                    file: open.file.clone(),
                    packets: summary.packets,
                    bytes: summary.bytes,
                    first_ns: summary
                        .first_time_ns
                        .map(|time| relative(time, summary.realtime_minus_monotonic_ns)),
                    last_ns: summary
                        .last_time_ns
                        .map(|time| relative(time, summary.realtime_minus_monotonic_ns)),
                    realtime_minus_monotonic_ns: summary.realtime_minus_monotonic_ns,
                    receive_buffer_bytes: open
                        .receive_buffer_bytes
                        .max(summary.receive_buffer_bytes),
                    interface_frames: summary.interface_frames,
                    missing: summary.missing,
                }
            })
            .collect()
    }
}
