//! Ctrl-C and `kill` during a run (D-066): the first signal stops the workloads and keeps what the run has recorded, the
//! second quits at once.
//!
//! The handler of the signal is the one of `signal-hook`; this crate has no `unsafe`. A thread waits for SIGINT and
//! SIGTERM and tells the run through its [`CancelToken`]. The supervisor, the nodes and the agents are in a process
//! group of their own, so the signal that a terminal sends for Ctrl-C reaches this program only and the run stops in an
//! orderly way. If this program is killed in some other way, the pipe of the supervisor closes and it removes the
//! network (D-022).

use std::io::Write;
use std::os::fd::AsFd;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};
use std::thread::JoinHandle;

use netscenario_core::CancelToken;
use signal_hook::consts::{SIGINT, SIGTERM};
use signal_hook::iterator::{Handle, Signals};

/// The exit code of a program that was stopped by the signal number `signal`, as a shell counts it.
#[must_use]
pub(crate) fn code_of(signal: i32) -> u8 {
    u8::try_from(128 + signal).unwrap_or(130)
}

/// Waits for the signals while a run goes. Dropping it ends the thread.
pub(crate) struct Watch {
    signal: Arc<AtomicI32>,
    handle: Handle,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Starts to watch. The first signal cancels `token` and says so on standard error (`on_terminal`: the line of
    /// progress is erased first); the second quits the program at once. `None` if the signals cannot be watched, in
    /// which case Ctrl-C ends the program as it always did.
    pub(crate) fn start(token: CancelToken, on_terminal: bool) -> Option<Self> {
        // The thread writes through a copy of the descriptor of standard error and not through `std::io::stderr()`: the
        // program holds the lock of that while it runs, and a thread that asked for it would wait until the run is over
        let mut err = std::fs::File::from(std::io::stderr().as_fd().try_clone_to_owned().ok()?);
        let mut signals = Signals::new([SIGINT, SIGTERM]).ok()?;
        let handle = signals.handle();
        let signal = Arc::new(AtomicI32::new(0));
        let seen = Arc::clone(&signal);
        let thread = std::thread::spawn(move || {
            for number in signals.forever() {
                if on_terminal {
                    let _ = write!(err, "\r\x1b[2K");
                }
                if seen.swap(number, Ordering::SeqCst) == 0 {
                    token.cancel();
                    let _ = writeln!(
                        err,
                        "netscenario: interrupted; stopping the workloads and keeping what was recorded (once more to quit at once)"
                    );
                } else {
                    let _ = writeln!(
                        err,
                        "netscenario: quitting at once; the network is removed all the same"
                    );
                    std::process::exit(i32::from(code_of(number)));
                }
            }
        });
        Some(Self {
            signal,
            handle,
            thread: Some(thread),
        })
    }

    /// The number of the signal that interrupted the run, if one did.
    pub(crate) fn signal(&self) -> Option<i32> {
        Some(self.signal.load(Ordering::SeqCst)).filter(|number| *number != 0)
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_code_of_a_signal_is_128_plus_its_number() {
        assert_eq!(code_of(SIGINT), 130);
        assert_eq!(code_of(SIGTERM), 143);
    }
}
