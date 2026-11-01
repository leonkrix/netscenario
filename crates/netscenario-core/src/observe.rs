//! Watching a run and stopping it: the events that `run` reports and the token that cancels it (D-031).
//!
//! A run takes seconds to minutes. The command line wants to show how far it is, a test framework wants to
//! stop it on a timeout or an interrupt, and a program that embeds the library wants both. They are not
//! options of one front end but parts of the library, so that every front end gets them.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A request to stop a run. Clones share the request: cancel one and the run that holds another sees it.
///
/// The run looks at the token at least every 200 milliseconds, tears the network down and returns
/// [`Error::Cancelled`](crate::Error::Cancelled). The token cannot be reset: a new run takes a new token.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    /// A token that has not been cancelled.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks the run to stop. It may be called from any thread, also from the observer, and more than once.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether [`cancel`](Self::cancel) has been called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// What a run reports about itself, in the order in which it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunEvent {
    /// The network is built: the nodes and links exist and the impairments are applied.
    Provisioned {
        /// How many nodes.
        nodes: usize,
        /// How many links.
        links: usize,
    },
    /// Every receiver listens. The senders start next.
    ReceiversReady {
        /// How many receivers.
        count: usize,
    },
    /// The senders are started and the workloads begin shortly (they all start at one moment).
    Started {
        /// How long the workloads run, from the start of the first to the end of the last.
        duration: Duration,
    },
    /// The workloads are running. Reported once a second, and a last time with `elapsed` equal to `total`.
    Progress {
        /// How long they have been running.
        elapsed: Duration,
        /// How long they run, the same as in [`RunEvent::Started`].
        total: Duration,
    },
    /// The senders are done; the run waits for the packets that are still in the network.
    Settling {
        /// How long it waits.
        wait: Duration,
    },
    /// The receivers are stopped and their records are read.
    Collecting,
    /// The network is torn down. Reported on every path, also after an error or a cancel.
    TearingDown,
    /// The run is done and its result is written. Not reported after an error or a cancel.
    Finished,
    /// A step of the timeline was done: an impairment of a link changed. Reported while the workloads run, between
    /// [`RunEvent::Started`] and [`RunEvent::Settling`]. It is the last variant so that the numbers of the others do not
    /// change (`cargo-semver-checks` compares them even for a variant with fields).
    StepDone {
        /// The place of the event in `timeline`.
        event: usize,
        /// How late it was, from the moment the file gives to the moment the backend had done it.
        lateness: Duration,
    },
    /// The run was cancelled while the workloads were running and was asked to keep what it has
    /// ([`RunOptions::partial_on_cancel`](crate::RunOptions::partial_on_cancel), D-066): the senders are told to
    /// stop, and the run goes on with the packets still in the network and with collecting the records. It is the last
    /// variant for the same reason as [`RunEvent::StepDone`].
    Interrupted,
}

/// Someone who wants to know how a run is going.
///
/// `event` is called on the thread that runs, between two steps of the run, and the run waits for it: it
/// should return quickly. It is `Send + Sync` so that a front end can keep it in an `Arc` and a caller can
/// cancel from it. Any `Fn(&RunEvent)` that is `Send + Sync` is an observer.
pub trait RunObserver: Send + Sync {
    /// Called for each [`RunEvent`].
    fn event(&self, event: &RunEvent);
}

impl<F> RunObserver for F
where
    F: Fn(&RunEvent) + Send + Sync,
{
    fn event(&self, event: &RunEvent) {
        self(event);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn a_clone_sees_the_cancel_of_another() {
        let token = CancelToken::new();
        let seen_elsewhere = token.clone();
        assert!(!token.is_cancelled() && !seen_elsewhere.is_cancelled());
        seen_elsewhere.cancel();
        assert!(token.is_cancelled());
        // and again is no harm
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn a_token_can_be_cancelled_from_another_thread() {
        let token = CancelToken::new();
        let other = token.clone();
        std::thread::spawn(move || other.cancel()).join().unwrap();
        assert!(token.is_cancelled());
    }

    #[test]
    fn a_closure_is_an_observer() {
        let seen = Mutex::new(Vec::new());
        let observer = |event: &RunEvent| seen.lock().unwrap().push(event.clone());
        let observer: &dyn RunObserver = &observer;
        observer.event(&RunEvent::Collecting);
        observer.event(&RunEvent::Finished);
        assert_eq!(
            *seen.lock().unwrap(),
            [RunEvent::Collecting, RunEvent::Finished]
        );
    }
}
