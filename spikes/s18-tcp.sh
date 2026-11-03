#!/usr/bin/env bash
# S18: TCP in the namespaces. Release 0.3 adds TCP bulk and request/response workloads (WRK-04, WRK-05) and the classic
# experiments (fairness over a dumbbell, throughput against loss and delay). Questions:
#   S18.1  Which congestion controls may a socket select in a node (reno, cubic, bbr)? What do the sysctls say?
#   S18.2  What do segmentation offloads do on a veth pair? A qdisc sees one 64 kB packet where the wire has 44: for the
#          rate and for the loss of netem that is wrong. Measured by the mean packet size in the qdisc statistics, with the
#          offloads on (the default) and off.
#   S18.3  Goodput of one flow through a shaper of 8 mbit, per second (time to steady state, efficiency), reno and cubic.
#   S18.4  Two flows over the same shaper: the shares, Jain's index, and how much it varies from run to run.
#   S18.5  TCP_INFO: which fields can be read, what does a sample cost?
#   S18.6  With netem: reno against the model of Mathis et al. (rate = MSS / RTT * 1.22 / sqrt(p)), offloads on and off.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
if [ "${1:-outer}" = outer ]; then
  enter_ns S18.0 "$0"
fi
. "$SPIKE_DIR/lab.sh"
trap lab_stop EXIT
TCPFLOW="python3 $SPIKE_DIR/tcpflow.py"
info S18.0 "kernel $(uname -r); $(tc -V 2>&1 | head -n 1); $(ethtool --version 2>&1 | head -n 1)"
command -v ethtool >/dev/null || { warn S18.0 "no ethtool: S18.2 and S18.6 are limited"; }

# a -- r -- b: the bottleneck is on the interface of the router towards b (l1a), so that the packets it holds belong to no
# socket of the sender (a queue on the sending node pushes back on the sender: S17.6)
pair() {
  lab_stop; node_new a; node_new r; node_new b
  link_new 0 a r 10.230.0.1/30 10.230.0.2/30; link_new 1 r b 10.230.1.1/30 10.230.1.2/30
  node_forward r
  node a ip route replace 10.230.1.0/30 via 10.230.0.2 dev l0a
  node b ip route replace 10.230.0.0/30 via 10.230.1.1 dev l1b
}
field() { python3 -c 'import json,sys; d=json.loads(sys.stdin.read()); print(eval(sys.argv[1], {"d": d}))' "$1" 2>/dev/null; }
shaper() { # RATE LIMITPACKETS
  node r tc qdisc add dev l1a root handle 1: tbf rate "$1" burst 12000 limit 2000000
  node r tc qdisc add dev l1a parent 1: handle 10: pfifo limit "$2"
}
offloads_off() { local n i
  for n in a r b; do for i in l0a l0b l1a l1b; do node "$n" ethtool -K "$i" gso off tso off gro off 2>/dev/null; done; done; }

# run_flows CC1 [CC2] SECONDS  -> SERVER_JSON, CLIENT_JSON (the first client)
run_flows() {
  local seconds="${@: -1}" ccs=("${@:1:$#-1}") out cl i
  out="$(mktemp)"; cl="$(mktemp)"
  node b $TCPFLOW server 10.230.1.2 7100 "${#ccs[@]}" "$seconds" >"$out" 2>&1 &
  local spid=$!
  sleep 0.5
  local cpids=()
  for i in "${!ccs[@]}"; do
    nsenter -t "${NODE_PID[a]}" -n $TCPFLOW client 10.230.1.2 7100 "$seconds" "${ccs[$i]}" >"$cl.$i" 2>&1 &
    cpids+=($!)
  done
  for i in "${cpids[@]}"; do wait "$i"; done
  wait "$spid"
  SERVER_JSON="$(tail -n 1 "$out")"; CLIENT_JSON="$(tail -n 1 "$cl.0")"
  rm -f "$out" "$cl" "$cl".*
}

# S18.1
pair
info S18.1 "sysctl in a node: $(node a sysctl -n net.ipv4.tcp_available_congestion_control net.ipv4.tcp_allowed_congestion_control net.ipv4.tcp_congestion_control 2>&1 | tr '\n' ';')"
info S18.1 "a socket in a node may select: $(node a $TCPFLOW cc)"
info S18.1 "tcp_rmem $(node a sysctl -n net.ipv4.tcp_rmem | tr '\t' ' '); tcp_wmem $(node a sysctl -n net.ipv4.tcp_wmem | tr '\t' ' ')"

# S18.2 offloads
pair
if command -v ethtool >/dev/null; then
  info S18.2 "veth features before: $(node a ethtool -k l0a 2>&1 | grep -E '^(tcp-segmentation-offload|generic-segmentation-offload|generic-receive-offload|tx-checksumming|rx-checksumming):' | tr '\n' ' ')"
fi
for state in default off; do
  pair
  [ "$state" = off ] && offloads_off
  node r tc qdisc add dev l1a root pfifo limit 100000
  run_flows reno 4
  stats="$(node r tc -s qdisc show dev l1a | awk '/Sent/ {print $2, $4; exit}')"
  set -- $stats
  info S18.2 "offloads $state, pfifo on the router: the qdisc saw $1 bytes in $2 packets, mean $(( ${2:-0} > 0 ? $1 / $2 : 0 )) bytes per packet (a frame on the wire has at most 1514)"
done

# S18.3 one flow, per second
for cc in reno cubic bbr; do
  pair; offloads_off; shaper 8mbit 80
  run_flows "$cc" 10
  info S18.3 "$cc through 8 mbit, queue 80 packets: kbit/s per second $(printf '%s' "$SERVER_JSON" | field '[round(b * 8 / 1000) for b in d["bytes_per_second"][0]]'); client: $(printf '%s' "$CLIENT_JSON" | field 'd["cc"] + " " + str(d["final"])')"
done

# S18.4 two flows
jain() { python3 -c '
import json, sys
d = json.loads(sys.stdin.read())["bytes_per_second"]
shares = [sum(f[3:]) for f in d]
total = sum(shares)
print("shares %s, total %d kbit/s, Jain %.4f" % ([round(100 * s / total) for s in shares], round(total * 8 / 1000 / (len(d[0]) - 3)), total * total / (len(shares) * sum(s * s for s in shares))))'; }
for pairing in "reno reno" "cubic cubic" "reno cubic"; do
  for run in 1 2 3; do
    pair; offloads_off; shaper 8mbit 80
    run_flows $pairing 12
    info S18.4 "$pairing, run $run: $(printf '%s' "$SERVER_JSON" | jain)"
  done
done

# S18.5 TCP_INFO
pair; offloads_off; shaper 8mbit 80
run_flows cubic 5
info S18.5 "TCP_INFO: $CLIENT_JSON"

# S18.6 Mathis
if ! has_netem=$(tc qdisc add dev lo root netem delay 1ms 2>&1 && tc qdisc del dev lo root); then
  info S18.6 "skipped: this kernel has no netem"
else
  for offload in on off; do
    for p in 0.5 1 2; do
      pair
      [ "$offload" = off ] && offloads_off
      node r tc qdisc add dev l1a root netem delay 20ms loss "$p%" limit 10000
      node r tc qdisc add dev l0b root netem delay 20ms limit 10000
      run_flows reno 20
      rate="$(printf '%s' "$SERVER_JSON" | field 'round(sum(d["bytes_per_second"][0][5:]) * 8 / 15 / 1000)')"
      model="$(python3 -c "import math; print(round(1448*8/0.0404*1.22/math.sqrt($p/100)/1000))")"
      info S18.6 "reno, offloads $offload, RTT 40 ms + serialisation, loss $p %: $rate kbit/s, Mathis $model kbit/s (ratio $(python3 -c "print(round($rate/$model,2))"))"
    done
  done
fi
