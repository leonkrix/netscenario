//! The arguments of `tc ... netem` for the impairments of one direction.

use netscenario_model::{Duration, Impairments, JitterDistribution, MAX_PACKETS_PER_SECOND};

/// The most packets a queue may hold. A queue this long at the largest packet size takes 1.4 GB, so a sender
/// that overruns a slow link cannot use all the memory of the machine.
const MAX_QUEUE: u64 = 1_000_000;

/// The packets that `netem` may hold for the delay of a link.
///
/// `netem` drops a packet when its queue is full, and its default is 1000 packets. With a delay of 20 ms and
/// 100,000 packets per second (the most a workload may send, `MAX_PACKETS_PER_SECOND`) 2000 packets are in
/// flight, so the default would lose packets that nobody asked to lose. The limit covers the packets in flight
/// at the largest rate and leaves the default on top for queueing. The user's control of the queue comes with
/// release 0.3 (`docs/COVERAGE.md`, "Queue limit").
pub(crate) fn queue_limit(delay: Option<Duration>, jitter: Option<Duration>) -> u64 {
    let nanoseconds = delay.map_or(0, Duration::as_nanos) + jitter.map_or(0, Duration::as_nanos);
    let in_flight = u128::from(MAX_PACKETS_PER_SECOND) * u128::from(nanoseconds) / 1_000_000_000;
    u64::try_from(in_flight)
        .unwrap_or(MAX_QUEUE)
        .saturating_add(1000)
        .min(MAX_QUEUE)
}

/// A time as `netem` takes it: whole microseconds.
///
/// # Errors
///
/// Text if the time is not a whole number of microseconds: `netem` would round it.
pub(crate) fn microseconds(time: Duration) -> Result<String, String> {
    let nanoseconds = time.as_nanos();
    if nanoseconds.is_multiple_of(1000) {
        Ok(format!("{}us", nanoseconds / 1000))
    } else {
        Err(format!(
            "netem works in whole microseconds and {time} is not a multiple of 1us"
        ))
    }
}

/// A probability as a percentage with the six decimals that the format allows.
fn percent(micro_percent: u32) -> String {
    format!(
        "{}.{:06}%",
        micro_percent / 1_000_000,
        micro_percent % 1_000_000
    )
}

/// The arguments that follow `netem`, or `None` if `impairments` impairs nothing.
///
/// `seed` is used only with loss: it seeds the random loss events (D-013).
///
/// # Errors
///
/// Text if a value cannot be applied exactly.
pub(crate) fn arguments(
    impairments: &Impairments,
    seed: Option<u64>,
) -> Result<Option<Vec<String>>, String> {
    if impairments.delay.is_none() && impairments.loss.is_none() && impairments.rate.is_none() {
        return Ok(None);
    }
    let jitter = impairments.jitter.as_ref();
    let mut arguments = vec![
        "limit".to_owned(),
        queue_limit(impairments.delay, jitter.map(|j| j.amount)).to_string(),
    ];
    if let Some(delay) = impairments.delay {
        arguments.push("delay".to_owned());
        arguments.push(microseconds(delay)?);
        if let Some(jitter) = jitter {
            arguments.push(microseconds(jitter.amount)?);
            if jitter.distribution == JitterDistribution::Normal {
                arguments.push("distribution".to_owned());
                arguments.push("normal".to_owned());
            }
        }
    }
    if let Some(loss) = &impairments.loss {
        arguments.push("loss".to_owned());
        arguments.push("random".to_owned());
        arguments.push(percent(loss.p.micro_percent()));
        if let Some(seed) = seed {
            arguments.push("seed".to_owned());
            arguments.push(seed.to_string());
        }
    }
    if let Some(rate) = impairments.rate {
        arguments.push("rate".to_owned());
        arguments.push(format!("{}bit", rate.bits_per_second()));
    }
    Ok(Some(arguments))
}

#[cfg(test)]
mod tests {
    use super::*;
    use netscenario_model::Scenario;

    /// The impairments of the first link of a scenario with the text `both`.
    fn impairments(both: &str) -> Impairments {
        let yaml = format!(
            "version: 0\nname: t\nduration: 10s\ntopology:\n  nodes: [a, b]\n  links:\n    - id: l\n      between: [a, b]\n      both: {both}\nworkloads:\n  - {{ id: w, kind: udp-cbr, from: a, to: b, rate: 1mbit, packet-size: 1200b }}\n"
        );
        Scenario::from_yaml(&yaml, "t.yaml").unwrap().topology.links[0]
            .both
            .clone()
            .unwrap()
    }

    fn text(both: &str, seed: Option<u64>) -> String {
        arguments(&impairments(both), seed)
            .unwrap()
            .unwrap()
            .join(" ")
    }

    #[test]
    fn delay_is_in_whole_microseconds() {
        assert_eq!(text("{ delay: 20ms }", None), "limit 3000 delay 20000us");
        assert_eq!(text("{ delay: 1500us }", None), "limit 1150 delay 1500us");
    }

    #[test]
    fn jitter_names_its_distribution_only_when_it_is_not_uniform() {
        assert_eq!(
            text("{ delay: 20ms, jitter: { amount: 5ms } }", None),
            "limit 3500 delay 20000us 5000us"
        );
        assert_eq!(
            text(
                "{ delay: 20ms, jitter: { amount: 5ms, distribution: normal } }",
                None
            ),
            "limit 3500 delay 20000us 5000us distribution normal"
        );
    }

    #[test]
    fn loss_has_six_decimals_and_a_seed_only_when_there_is_one() {
        assert_eq!(
            text("{ loss: { model: random, p: 0.5% } }", None),
            "limit 1000 loss random 0.500000%"
        );
        assert_eq!(
            text("{ loss: { model: random, p: 100% } }", Some(7)),
            "limit 1000 loss random 100.000000% seed 7"
        );
        assert_eq!(
            text("{ loss: { model: random, p: 0.000001% } }", None),
            "limit 1000 loss random 0.000001%"
        );
    }

    #[test]
    fn a_seed_without_loss_is_not_passed() {
        assert_eq!(text("{ delay: 5ms }", Some(7)), "limit 1500 delay 5000us");
    }

    #[test]
    fn the_rate_is_in_bits() {
        assert_eq!(text("{ rate: 2mbit }", None), "limit 1000 rate 2000000bit");
    }

    #[test]
    fn nothing_to_impair_gives_no_arguments() {
        assert_eq!(arguments(&Impairments::new(), None), Ok(None));
    }

    /// Canary: a time that `netem` would round is refused instead.
    #[test]
    fn a_time_below_a_microsecond_is_refused() {
        let error = arguments(&impairments("{ delay: 1500ns }"), None).unwrap_err();
        assert!(
            error.contains("whole microseconds") && error.contains("1500ns"),
            "{error}"
        );
    }

    #[test]
    fn the_queue_covers_the_packets_in_flight() {
        let ms = |n| Duration::from_millis(n);
        assert_eq!(queue_limit(None, None), 1000);
        assert_eq!(queue_limit(ms(20), None), 3000);
        assert_eq!(queue_limit(ms(500), ms(100)), 61_000);
        assert_eq!(queue_limit(Duration::from_secs(3600), None), 1_000_000);
    }
}
