# A dumbbell

Two senders on the left, two receivers on the right, and one **bottleneck** between two routers that the senders share. It is the classic shape for the question "do two flows share a link fairly?", and the shortcut makes it in a few lines:

```yaml,scenario,run
# file: fairness.yaml
version: 0
name: fairness
seed: 11
duration: 9s
topology:
  dumbbell:
    left: [alice, bob]
    right: [carol, dave]
    access:
      both: { delay: 2ms, rate: 100mbit }
    bottleneck:
      forward: { delay: 5ms, rate: 10mbit, queue: { discipline: fifo, limit: 30p } }
      reverse: { delay: 5ms }
workloads:
  - { id: first, kind: tcp-bulk, from: alice, to: carol, cc: reno, stop: 8s }
  - { id: second, kind: tcp-bulk, from: bob, to: dave, cc: reno, stop: 8s }
measure:
  series: { bin: 1s }
```

The two flows start together, have the same round trip and send until `stop`. The bottleneck is 10 mbit with a queue of 30 packets, about two bandwidth-delay products. Read the goodput of each flow after the start-up and judge the share with the Jain index (1 is perfectly fair, 0.5 is one flow taking everything):

```python,run
import netscenario as ns

result = ns.run(ns.Scenario.load("fairness.yaml"))
window = slice(2, 8)                         # leave the start-up out
a = result.tcp["first"].goodput.bits_per_second[window]
b = result.tcp["second"].goodput.bits_per_second[window]
share_a, share_b = sum(a) / len(a), sum(b) / len(b)
jain = (share_a + share_b) ** 2 / (2 * (share_a**2 + share_b**2))
print(f"{share_a / 1e6:.2f} and {share_b / 1e6:.2f} mbit, Jain index {jain:.2f}")
assert share_a + share_b > 8e6               # together they fill the bottleneck
```

What to take from it:

- **Together the flows fill the bottleneck**, and nothing more: the sum of the goodputs is the rate of the link less the headers.
- **One run is one sample.** The imbalance between two flows varies between runs, so a claim about fairness is made on several runs: `--repetitions 6` and a bound on the mean of the index. The test of this release does exactly that, with the bounds worked out in the file.
- **Change one thing and the shape changes.** Give one access link 22 ms more delay and the flow with the shorter round trip takes most of the bottleneck ([`scenarios/tcp-fairness-unequal-rtt.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/tcp-fairness-unequal-rtt.yaml)). Swap `fifo` for `codel` and the queue keeps its delay near a target ([`scenarios/tcp-queue.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/tcp-queue.yaml)).
