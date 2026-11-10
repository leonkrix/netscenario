# A chain and a star

More than two nodes means routers. A node with `role: router` forwards packets; a host does not. The routes are computed by the shortest path, and you can list your own.

## A chain

A client and a server with two routers between them, each hop with its own values. The `chain` shortcut writes the nodes and the links for you:

```yaml,scenario,run
# file: chain.yaml
version: 0
name: chain
seed: 1
duration: 4s
topology:
  chain:
    nodes: [client, edge, core, server]
    links:
      - both: { delay: 5ms, rate: 50mbit }        # client - edge
      - both: { delay: 15ms, rate: 10mbit }       # edge - core
      - both: { delay: 5ms }                      # core - server
workloads:
  - { id: stream, kind: udp-cbr, from: client, to: server, rate: 2mbit, packet-size: 1000b }
```

The delay of the stream is the sum of the three hops, 25 ms, and its rate is held to the smallest rate on the path. `netscenario show chain.yaml` prints the nodes, the links, the routes and the path the stream takes; `netscenario expand chain.yaml` prints the long form.

## A star

Three hosts around a hub with their own values, and a call from one of them to another through the hub:

```yaml,scenario,run
# file: star.yaml
version: 0
name: star
seed: 2
duration: 4s
topology:
  star:
    hub: hub
    leaves: [phone, laptop, server]
    links:
      - both: { delay: 30ms, jitter: { amount: 5ms, distribution: normal }, rate: 20mbit }
      - both: { delay: 3ms, rate: 100mbit }
      - both: { delay: 1ms }
workloads:
  - { id: call, kind: udp-cbr, from: phone, to: server, rate: 1mbit, packet-size: 800b }
```

The call crosses two links, the phone's and the server's, so its delay is near 31 ms. The laptop is in the network but carries nothing; add a workload from it and both flows share the hub.

## By hand

The shortcuts are only a way to write nodes and links. Any network of hosts and routers can be written in the long form, with `routes` to set a route by hand over the computed ones (`routing: manual` turns the computed ones off). The files [`scenarios/three-hops.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/three-hops.yaml) and [`scenarios/manual-routes.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/manual-routes.yaml) show both, and [`scenarios/ipv6.yaml`](https://github.com/leonkrix/netscenario/blob/main/scenarios/ipv6.yaml) the same with IPv6 addresses.
