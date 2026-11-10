# Several paths

Between two nodes there may be more than one way: a fast one and a slow one, two ISPs, a cellular and a Wi-Fi link. `multipath` writes the paths, and `routes` with a `weight` says how connections are spread over them.

```yaml,scenario,run
# file: multipath.yaml
version: 0
name: multipath
seed: 31
duration: 6s
topology:
  multipath:
    from: client
    to: server
    paths:
      - routers: [slow]
        link: { both: { delay: 5ms, rate: 4mbit, queue: { limit: 30p } } }
      - routers: [wide]
        link: { both: { delay: 5ms, rate: 8mbit, queue: { limit: 30p } } }
  routes:
    - { node: client, to: server, via: client-slow, weight: 1 }
    - { node: client, to: server, via: client-wide, weight: 2 }
workloads:
  - { id: flows, kind: tcp-bulk, from: client, to: server, cc: reno, connections: 16, stop: 5s }
measure:
  series: { bin: 1s }
```

Sixteen TCP connections are spread over the two paths by their ports (the kernel hashes them, and the backend asks it to look at the ports). Each connection stays on one path, and each path is full as long as it has a connection, so together they carry the **sum** of the rates, 12 mbit, which no single path can:

```python,run
import netscenario as ns

result = ns.run(ns.Scenario.load("multipath.yaml"))
goodput = result.tcp["flows"].goodput.bits_per_second
steady = goodput[2:5]                        # after the start-up, before the stop
print(f"{sum(steady) / len(steady) / 1e6:.1f} mbit on average")
```

The weights 1 and 2 are the rates of the paths, so each path gets connections in proportion to what it can carry. With weights that do not match the rates, a path overflows while the other idles; [`scenarios/tcp-multipath-weights.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/tcp-multipath-weights.yaml) counts the share each path got with a capture.

Only the way from the client is listed here; the acknowledgements take the first path, which has room for them. A path can go down with the timeline ([Changes over time](timeline.md)) to see what the connections do when it does: [`scenarios/two-paths.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/two-paths.yaml) has a fast path that goes down and comes back.
