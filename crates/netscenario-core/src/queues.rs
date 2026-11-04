//! The look at the queues while the workloads go (PRB-10, D-082).

use netscenario_agent::clock::monotonic_ns;

use crate::backend::{Backend, Direction, LinkHandle};
use crate::error::Error;
use crate::result::QueueSample;

/// The time between two looks.
const INTERVAL_NS: u64 = 1_000_000_000;

/// One direction with a queue: where it is, and what was seen.
struct Watched {
    link: String,
    word: &'static str,
    handle: LinkHandle,
    direction: Direction,
    samples: Vec<QueueSample>,
}

/// Reads the counters of every queue once a second from the start of the measured phase. A backend that cannot say gets no
/// samples.
pub(crate) struct Sampler {
    watched: Vec<Watched>,
    interval_ns: u64,
    next_ns: u64,
}

impl Sampler {
    /// A sampler for the directions with a `queue` of the scenario: link, `forward` or `reverse`, handle and direction.
    pub(crate) fn new(
        ways: impl IntoIterator<Item = (String, &'static str, LinkHandle, Direction)>,
    ) -> Self {
        Self {
            watched: ways
                .into_iter()
                .map(|(link, word, handle, direction)| Watched {
                    link,
                    word,
                    handle,
                    direction,
                    samples: Vec::new(),
                })
                .collect(),
            interval_ns: INTERVAL_NS,
            next_ns: INTERVAL_NS,
        }
    }

    /// The same with another time between the looks, so that a test does not wait seconds.
    #[cfg(test)]
    pub(crate) fn with_interval(mut self, interval_ns: u64) -> Self {
        self.interval_ns = interval_ns;
        self.next_ns = interval_ns;
        self
    }

    /// Looks at the queues if a second has passed since the last look. `begin_ns` is the start of the measured phase on the
    /// clock of the records.
    pub(crate) fn look(&mut self, backend: &mut dyn Backend, begin_ns: u64) -> Result<(), Error> {
        if self.watched.is_empty() {
            return Ok(());
        }
        let Some(elapsed) = monotonic_ns().checked_sub(begin_ns) else {
            return Ok(());
        };
        if elapsed < self.next_ns {
            return Ok(());
        }
        self.next_ns = elapsed - elapsed % self.interval_ns + self.interval_ns;
        for watched in &mut self.watched {
            let Some(stats) = backend.queue_stats(watched.handle, watched.direction)? else {
                continue;
            };
            watched.samples.push(QueueSample {
                t_ns: elapsed,
                packets: stats.packets,
                drops: stats.drops,
                backlog_packets: stats.backlog_packets,
                backlog_bytes: stats.backlog_bytes,
            });
        }
        Ok(())
    }

    /// What was seen of a direction, by link and `forward` or `reverse`.
    pub(crate) fn samples_of(&self, link: &str, word: &str) -> Vec<QueueSample> {
        self.watched
            .iter()
            .find(|watched| watched.link == link && watched.word == word)
            .map(|watched| watched.samples.clone())
            .unwrap_or_default()
    }
}
