//! A `tc` that lives for the whole run (D-038, D-063): one `tc -force -batch -` in the network namespace of a node,
//! which takes the lines of a change on its standard input. A change costs 0.03 ms through it, against 1 to 3 ms for
//! a process that is started each time, and the step of a timeline is what a packet waits for at a barrier.
//!
//! **How the end of a change is known.** `tc` talks to the kernel for each line and waits for its answer before it
//! reads the next one, so when the line after a change has been done, the kernel has the change. The line after a
//! change is a `qdisc show` of a device that does not exist, which `tc` answers on its standard error with
//! `Cannot find device "<mark>"`. That answer is the acknowledgement, and it comes on the stream that also carries
//! the errors of the lines before it, so an error and the end of the change cannot be told apart in the wrong order.
//! (`tc -force` goes on after a line that failed. The obvious acknowledgement, `qdisc show dev <interface>` on the
//! standard output, does not work: `tc` keeps the filter of the last `show` between the lines of a batch and prints
//! other interfaces and a number of lines that is not the number of queues.)
//!
//! **What can go wrong, and what then.** `tc` refuses a line ([`Failure::Refused`]): that is the answer to the
//! change, as it is for a process. The batch does not answer within the time limit, ends, or cannot be written to
//! ([`Failure::Broken`]): the caller ends it and makes the change with a `tc` process, which is slower and just as
//! exact, because a change writes the whole state. S11 saw a batch that did not answer on a runner, so a limit on
//! every change is part of the design.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

/// How long a batch may take to answer, for its start and for each change. A change takes 0.03 ms to 2 ms on the
/// machines that were measured (S11, with every processor busy), so two seconds is a hang and not a busy machine,
/// and it is far below the ten seconds that a sender waits at a barrier before it goes on without an answer.
pub(crate) const ANSWER_WITHIN: Duration = Duration::from_secs(2);

/// Why a change through the batch did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// `tc` answered the line with an error. A process would have said the same.
    Refused(String),
    /// The batch is not working: it did not answer in time, it ended, or it cannot be written to.
    Broken(String),
}

/// What `tc` says to a line that names a device that is not there.
const NO_DEVICE: &str = "Cannot find device";

/// What `tc` says after a line that failed, in a batch.
const FAILED: &str = "Command failed";

/// A `tc -force -batch -` and the way to talk to it.
#[derive(Debug)]
pub(crate) struct Batch {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<String>,
    /// The number of the next acknowledgement.
    serial: u64,
    /// The acknowledgement of the last change is followed by one more line (`Command failed ...`) that has not been
    /// read yet.
    tail_owed: bool,
}

impl Batch {
    /// Starts `program arguments` as a `tc -batch` and asks it once, so that a batch that does not work is known
    /// before the run depends on it.
    ///
    /// # Errors
    ///
    /// If the program cannot be started, or does not answer within `within`.
    pub(crate) fn start(
        program: &Path,
        arguments: &[String],
        within: Duration,
    ) -> Result<Self, String> {
        let mut child = Command::new(program)
            .args(arguments)
            .env("LC_ALL", "C")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start `{}`: {error}", program.display()))?;
        let (Some(stdin), Some(stderr)) = (child.stdin.take(), child.stderr.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("cannot connect to the pipes of the batch".to_owned());
        };
        let (sender, lines) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let mut batch = Self {
            child,
            stdin,
            lines,
            serial: 0,
            tail_owed: false,
        };
        match batch.run(&[], within) {
            Ok(()) => Ok(batch),
            Err(Failure::Refused(message) | Failure::Broken(message)) => {
                batch.end();
                Err(message)
            }
        }
    }

    /// Gives `tc` the lines of a change and returns when it has done them all.
    ///
    /// # Errors
    ///
    /// [`Failure::Refused`] with what `tc` said if a line failed; [`Failure::Broken`] if the batch did not answer
    /// within `within`, has ended, or could not be written to. After a `Broken` the batch is not to be used again.
    pub(crate) fn run(&mut self, commands: &[String], within: Duration) -> Result<(), Failure> {
        if commands.iter().any(|line| line.contains(['\n', '\r'])) {
            return Err(Failure::Refused(
                "a line of a batch cannot hold a line break".to_owned(),
            ));
        }
        let deadline = Instant::now() + within;
        self.skip_tail(deadline)?;
        let mark = mark(self.serial);
        self.serial += 1;
        let mut text = String::new();
        for line in commands {
            text.push_str(line);
            text.push('\n');
        }
        text.push_str("qdisc show dev ");
        text.push_str(&mark);
        text.push('\n');
        self.stdin
            .write_all(text.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|error| Failure::Broken(format!("the batch cannot be written to: {error}")))?;
        let acknowledgement = format!("{NO_DEVICE} \"{mark}\"");
        let mut said = Vec::new();
        loop {
            let line = self.next_line(deadline)?;
            if line == acknowledgement {
                self.tail_owed = true;
                return if said.is_empty() {
                    Ok(())
                } else {
                    Err(Failure::Refused(said.join("; ")))
                };
            }
            // The line that follows a failure repeats it without saying more
            if !line.starts_with(FAILED) {
                said.push(line.trim().to_owned());
            }
        }
    }

    /// Ends the batch. It can be called at any point.
    pub(crate) fn end(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Reads the `Command failed` line that follows the acknowledgement of the call before, if there is one.
    fn skip_tail(&mut self, deadline: Instant) -> Result<(), Failure> {
        if !self.tail_owed {
            return Ok(());
        }
        self.tail_owed = false;
        // It comes right behind the acknowledgement; if it is not there at once it is not coming, and a line that
        // is not it is left for the call to judge
        match self.lines.recv_timeout(
            Duration::from_millis(50).min(deadline.saturating_duration_since(Instant::now())),
        ) {
            Ok(line) if line.starts_with(FAILED) => Ok(()),
            Ok(line) => Err(Failure::Refused(line.trim().to_owned())),
            Err(RecvTimeoutError::Timeout) => Ok(()),
            Err(RecvTimeoutError::Disconnected) => Err(self.ended()),
        }
    }

    fn next_line(&mut self, deadline: Instant) -> Result<String, Failure> {
        match self
            .lines
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            Ok(line) => Ok(line),
            Err(RecvTimeoutError::Timeout) => Err(Failure::Broken(
                "the batch did not answer in time".to_owned(),
            )),
            Err(RecvTimeoutError::Disconnected) => Err(self.ended()),
        }
    }

    fn ended(&mut self) -> Failure {
        let status = self
            .child
            .try_wait()
            .ok()
            .flatten()
            .map_or_else(String::new, |status| format!(" ({status})"));
        Failure::Broken(format!("the batch ended{status}"))
    }
}

/// The name of the device that a change is followed by. `tc` cuts the name of a device at 15 characters (the size of
/// an interface name), so the mark is short, and a million acknowledgements later it starts again: only the one
/// before it could be mistaken for it, and that one has been read.
fn mark(serial: u64) -> String {
    format!("ack{}", serial % 1_000_000)
}

impl Drop for Batch {
    fn drop(&mut self) {
        self.end();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for `tc -batch`: it reads a line at a time and answers as `tc` does. A line `ack` is a line that
    /// names a device that is not there; `bad` is a line `tc` refuses; `hang` makes it stop answering; `die` ends it.
    const FAKE: &str = r#"
while read -r line; do
  case "$line" in
    *dev\ ack[0-9]*) echo "Cannot find device \"${line##* }\"" >&2; echo "Command failed -:1" >&2 ;;
    bad*) echo 'Illegal "latency"' >&2; echo "Command failed -:1" >&2 ;;
    hang*) sleep 30 ;;
    die*) exit 3 ;;
  esac
done
"#;

    fn fake() -> Result<Batch, String> {
        Batch::start(
            Path::new("sh"),
            &["-c".to_owned(), FAKE.to_owned()],
            Duration::from_secs(5),
        )
    }

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| (*line).to_owned()).collect()
    }

    const SHORT: Duration = Duration::from_millis(400);

    #[test]
    fn a_change_returns_when_the_batch_has_done_it_and_the_next_one_is_not_confused_by_the_tail() {
        let mut batch = fake().unwrap();
        for _ in 0..5 {
            assert_eq!(
                batch.run(&lines(&["qdisc change one", "qdisc change two"]), SHORT),
                Ok(())
            );
            assert_eq!(batch.run(&[], SHORT), Ok(()));
        }
    }

    /// `tc` cuts the name of a device at 15 characters (the size of an interface name), so a mark longer than that
    /// is the same for every change and the acknowledgement of a change would be taken for the next one's.
    #[test]
    fn the_mark_fits_the_name_of_an_interface_whatever_the_number_of_changes() {
        let mut batch = fake().unwrap();
        batch.serial = 999_999;
        assert_eq!(batch.run(&[], SHORT), Ok(()));
        assert_eq!(batch.run(&[], SHORT), Ok(()));
        assert!(mark(u64::MAX).len() <= 15);
        assert_ne!(mark(0), mark(1));
    }

    #[test]
    fn a_line_that_tc_refuses_is_the_answer_and_the_batch_goes_on() {
        let mut batch = fake().unwrap();
        let refused = batch.run(&lines(&["bad"]), SHORT);
        assert_eq!(
            refused,
            Err(Failure::Refused("Illegal \"latency\"".to_owned()))
        );
        // The line after the refused one was done, and so is the next change
        assert_eq!(batch.run(&lines(&["qdisc change one"]), SHORT), Ok(()));
    }

    #[test]
    fn canary_an_error_before_the_acknowledgement_is_not_taken_for_a_success() {
        // If the error of a line were read as the end of the change, the second call would take the acknowledgement
        // of the first for its own and say Ok to a change that was never done
        let mut batch = fake().unwrap();
        assert!(matches!(
            batch.run(&lines(&["bad", "bad"]), SHORT),
            Err(Failure::Refused(_))
        ));
        assert!(batch.run(&lines(&["bad"]), SHORT).is_err());
        assert_eq!(batch.run(&lines(&["qdisc change one"]), SHORT), Ok(()));
    }

    #[test]
    fn a_batch_that_does_not_answer_is_broken_after_the_time_limit() {
        let mut batch = fake().unwrap();
        let before = Instant::now();
        let failure = batch.run(&lines(&["hang"]), SHORT).unwrap_err();
        assert_eq!(
            failure,
            Failure::Broken("the batch did not answer in time".to_owned())
        );
        let waited = before.elapsed();
        assert!(
            waited >= SHORT && waited < Duration::from_secs(5),
            "{waited:?}"
        );
    }

    #[test]
    fn a_batch_that_ends_is_broken_and_says_how() {
        let mut batch = fake().unwrap();
        let failure = batch
            .run(&lines(&["die"]), Duration::from_secs(5))
            .unwrap_err();
        let Failure::Broken(message) = failure else {
            panic!("{failure:?}");
        };
        assert!(message.starts_with("the batch ended"), "{message}");
        // Once it has ended nothing can be written to it
        assert!(matches!(batch.run(&[], SHORT), Err(Failure::Broken(_))));
    }

    #[test]
    fn a_line_break_inside_a_line_is_refused_before_anything_is_written() {
        let mut batch = fake().unwrap();
        let failure = batch.run(&lines(&["qdisc change one\nqdisc del dev x root"]), SHORT);
        assert!(matches!(failure, Err(Failure::Refused(_))), "{failure:?}");
        assert_eq!(batch.run(&[], SHORT), Ok(()));
    }

    #[test]
    fn a_program_that_does_not_exist_or_does_not_answer_is_no_batch() {
        assert!(Batch::start(Path::new("/nonexistent/tc"), &[], SHORT).is_err());
        let silent = Batch::start(
            Path::new("sh"),
            &["-c".to_owned(), "sleep 30".to_owned()],
            SHORT,
        );
        assert_eq!(silent.unwrap_err(), "the batch did not answer in time");
    }
}
