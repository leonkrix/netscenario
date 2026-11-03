# A link that changes during a run

[`bandwidth-drop.yaml`](bandwidth-drop.yaml) runs a stream of 2 mbit over a link of 4 mbit with 50 ms of delay for 14 seconds and **changes the link while it runs**: at 3 s the rate drops to 1.2 mbit (less than the stream needs), at 5 s it is back, and from 9 s to 11 s an outage cuts off the receiver. The scenario is the one of the tests of the timeline ([docs/SCENARIO-FORMAT.md](../../docs/SCENARIO-FORMAT.md#the-timeline)), and the numbers in [`test_bandwidth_drop.py`](test_bandwidth_drop.py) are the ones that those tests derive and check.

```bash
pytest -rA                      # in this folder
netscenario run bandwidth-drop.yaml --out results/drop
netscenario report results/drop --format html --out drop.html
```

What the run shows, second by second ([docs/RESULTS.md](../../docs/RESULTS.md)):

- **The drop is not a loss.** While the link is slow about 58 % of the stream arrives each second, and nothing is lost: the rest waits in the queue of the link, so the delay of the packets of the fourth second is above half a second. Seen from the sender the stream is back at its level at once; seen from the receiver (`recovery_basis="arrival"`) it takes three seconds, because the queue is still being emptied.
- **The outage is a window at the receiver.** Nothing arrives in the eleventh second (bin 10), and the packets that are lost were *sent* from just before 9 s to just before 11 s, which is not where the silence is. Both views are in the series: `bin.by_send` counts a packet in the bin in which it was sent, `bin.by_arrival` in the bin in which it arrived.
- **Every step says how late it was** (`result.steps`, with `lateness`), and the time to recover of each event is in `result["media"].recoveries`.

The HTML report draws the throughput, the delay and the lost packets over time with the events as dashed lines. The first test reads the bins, the second the times to recover, and both are plain `assert`s on numbers in SI units.
