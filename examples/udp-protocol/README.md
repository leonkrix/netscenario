# A UDP protocol on a bad mobile link

A real-time game sends the state of the player 80 times a second and gets the state of the world back. How does that stream fare on a mobile link with 60 ms of delay, a wide jitter, 2 % loss and a narrow uplink?

[`game-link.yaml`](game-link.yaml) describes the network (two nodes and one link, different values in each direction) and two streams of the shape of the game: 64 kbit/s in packets of 100 bytes from the player, 128 kbit/s in packets of 200 bytes from the server. Change `rate` and `packet-size` to those of your protocol.

```bash
netscenario validate game-link.yaml     # reads and checks it, runs nothing
netscenario run game-link.yaml
```

```text
running game-link for 10s ...
netscenario run: game-link
  backend kernel (user namespace, iproute2 6.15.0)
  seed 7

workload input  player -> server  64 kbit/s, 100 byte packets
  sent 800  received 784  lost 16 (2.00 %)  reordered 252
  delay  min 18.5 ms  median 60.8 ms  p95 85.0 ms  p99 97.1 ms  max 114.0 ms
  throughput  62.72 kbit/s on average over 10 s (11 bins of one second in result.json)

workload state  server -> player  128 kbit/s, 200 byte packets
  sent 800  received 800  lost 0 (0.00 %)  reordered 280
  delay  min 9.2 ms  median 60.1 ms  p95 88.0 ms  p99 97.1 ms  max 107.6 ms
  throughput  128 kbit/s on average over 10 s (11 bins of one second in result.json)

result: netscenario-results/game-link-20261030T201947Z/result.json
```

## What the numbers say

- **`reordered 252`** of 800: with a jitter of 15 ms and a gap of 12.5 ms between two packets, a packet often overtakes the one before it. A protocol that needs packets in order has to cope with this link, and now you can see how often. (The stages of a direction and why this happens: [SCENARIO-FORMAT.md](../../docs/SCENARIO-FORMAT.md).)
- **`lost 16 (2.00 %)`** in the uplink and none in the downlink, as the file says. The seed is in the file, so the same packets are lost in the next run on the same kernel and `tc` ([COVERAGE.md](../../docs/COVERAGE.md), "Restriction same seed"). `--seed 8` gives other ones.
- **`p99 97.1 ms`**: one packet in a hundred needs longer than that, 37 ms above the configured delay of 60 ms.

## Reading the result in a script

`result.json` has every number, in nanoseconds and ratios ([RESULTS.md](../../docs/RESULTS.md)). A line with `jq`:

```bash
netscenario run game-link.yaml --out result --json > /dev/null
jq '.workloads[] | {id, loss: .loss_ratio, p99_ms: (.delay.p99_ns / 1e6)}' result/result.json
```

```json
{
  "id": "input",
  "loss": 0.02,
  "p99_ms": 95.927305
}
{
  "id": "state",
  "loss": 0,
  "p99_ms": 97.623736
}
```

The loss is the same as in the run above, the delays are not: the seed fixes which packets are lost, but not the jitter, which the kernel does not promise to repeat ([COVERAGE.md](../../docs/COVERAGE.md)). So assert a bound on a percentile, not on an exact number.

`netscenario run` exits with 0 when the run finished and with 4 when it failed; judging the numbers is for your script or, in Python, for a test ([python-test/](../python-test/)).
