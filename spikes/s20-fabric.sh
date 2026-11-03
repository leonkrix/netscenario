#!/usr/bin/env bash
# S20: a link through a fabric namespace. Release 0.3 needs a queue discipline or a rate that changes (LNK-09, LMD-03) on a
# link, and S17/S19 showed that the rate stage cannot live in `netem` for that, and that a shaper with `netem` as its
# child puts the delay before the rate. A link built as  node A -- veth -- FABRIC -- veth -- node B  has two egress
# queues per direction: the one at A (loss, then the shaper and the queue discipline) and the one in the fabric on the way
# to B (delay, jitter, reordering). That is the order of D-030 with a real queue. The fabric joins its two ports with a
# bridge or with a `mirred` redirect. Questions:
#   S20.1  Do packets cross it, and what does the extra hop cost (delay, TTL, MACs)? Bridge against mirred.
#   S20.2  Is the order exact: with a jitter and a saturated shaper, is the jitter at the output the configured one
#          (the direct tree of D-030 against the fabric against the delay-first tree)? (needs netem)
#   S20.3  Does the sender feel the queue (S17.6)? With a netem as the root at A, and with an htb root without it.
#   S20.4  Is the seeded loss pattern the same as on a direct link? (needs netem)
#   S20.5  What do the nodes see when the fabric port is taken down: carrier, and the error of the sender?
#   S20.6  Does a rate that steps follow in the fabric (htb at A)? Throughput of TCP with no limit: what does the hop cost?
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
if [ "${1:-outer}" = outer ]; then
  enter_ns S20.0 "$0"
fi
. "$SPIKE_DIR/lab.sh"
trap lab_stop EXIT
FLOW="python3 $SPIKE_DIR/flow.py"
TCPFLOW="python3 $SPIKE_DIR/tcpflow.py"
info S20.0 "kernel $(uname -r); $(tc -V 2>&1 | head -n 1)"
field() { python3 -c 'import json,sys; d=json.loads(sys.stdin.read()); print(eval(sys.argv[1], {"d": d}))' "$1" 2>/dev/null; }
HAS_NETEM=0
if tc qdisc add dev lo root netem delay 1ms 2>/dev/null; then tc qdisc del dev lo root; HAS_NETEM=1; fi

# direct_pair: a -- b joined by one veth pair (l0a in a, l0b in b)
direct_pair() { lab_stop; node_new a; node_new b; link_new 0 a b 10.250.0.1/30 10.250.0.2/30; AIF=l0a; BIF=l0b; FIF_TOWARDS_B=; }
# fabric_pair MODE (bridge|mirred): a -- f -- b; a has fa0 (ip .1), b has fb0 (ip .2); f has pa (towards a) and pb (towards b)
fabric_pair() {
  local mode="$1"
  lab_stop; node_new a; node_new b; node_new f
  ip link add fa0 type veth peer name pa; ip link set fa0 netns "${NODE_PID[a]}"; ip link set pa netns "${NODE_PID[f]}"
  ip link add fb0 type veth peer name pb; ip link set fb0 netns "${NODE_PID[b]}"; ip link set pb netns "${NODE_PID[f]}"
  node a ip addr add 10.250.0.1/30 dev fa0; node b ip addr add 10.250.0.2/30 dev fb0
  local ma mb
  ma="$(node a ip -br link show dev fa0 | awk '{print $3}')"; mb="$(node b ip -br link show dev fb0 | awk '{print $3}')"
  node a ip neigh replace 10.250.0.2 lladdr "$mb" dev fa0 nud permanent
  node b ip neigh replace 10.250.0.1 lladdr "$ma" dev fb0 nud permanent
  if [ "$mode" = bridge ]; then
    node f ip link add br0 type bridge
    node f ip link set pa master br0; node f ip link set pb master br0
    node f ip link set br0 up
  else
    node f tc qdisc add dev pa ingress; node f tc qdisc add dev pb ingress
    node f tc filter add dev pa parent ffff: protocol all prio 1 u32 match u32 0 0 action mirred egress redirect dev pb
    node f tc filter add dev pb parent ffff: protocol all prio 1 u32 match u32 0 0 action mirred egress redirect dev pa
  fi
  node f ip link set pa up; node f ip link set pb up
  node a ip link set fa0 up; node b ip link set fb0 up
  AIF=fa0; BIF=fb0; FIF_TOWARDS_B=pb
}

# first_stage DEV: the egress queue at the node, a netem root that holds the loss and, so that the packet is released from the
# socket of the sender, a delay of 1 microsecond (S17.6: the kernel orphans a packet in netem only when the netem has a delay,
# a jitter or a rate), then an htb class with a pfifo as its leaf
first_stage() { # NODE DEV RATE LIMIT [netem arguments]
  local n="$1" dev="$2" rate="$3" limit="$4"; shift 4
  node "$n" tc qdisc add dev "$dev" root handle 1: netem limit 100000 "$@"
  node "$n" tc qdisc add dev "$dev" parent 1: handle 2: htb default 10
  node "$n" tc class add dev "$dev" parent 2: classid 2:10 htb rate "$rate" ceil "$rate" burst 1600 cburst 1600
  node "$n" tc qdisc add dev "$dev" parent 2:10 handle 10: pfifo limit "$limit"
}
# send ADDR COUNT PPS SIZE  -> REPORT (the receiver's JSON), CLIENT (the sender's JSON)
send() {
  local count="$2" pps="$3" size="$4" out cl
  out="$(mktemp)"; cl="$(mktemp)"
  node b $FLOW server 4 "$1" 7000 "$count" 2.0 >"$out" 2>&1 &
  local spid=$!
  sleep 0.4
  FLOW_SIZE="$size" nsenter -t "${NODE_PID[a]}" -n $FLOW client 4 "$1" 7000 "$count" "$pps" >"$cl" 2>&1
  wait "$spid"
  REPORT="$(tail -n 1 "$out")"; CLIENT="$(tail -n 1 "$cl")"; rm -f "$out" "$cl"
}

# S20.1 crossing and cost
for build in direct bridge mirred; do
  case "$build" in direct) direct_pair;; *) fabric_pair "$build";; esac
  send 10.250.0.2 1000 1000 200
  info S20.1 "$build: $(printf '%s' "$REPORT" | field '"%d of 1000, p50 %.4f ms, p99 %.4f ms" % (d["received"], d["delay_ms"]["p50"], d["delay_ms"]["p99"])')"
done
fabric_pair bridge
info S20.1 "bridge: the neighbour entries of a and b stay as set: $(node a ip neigh show dev fa0 | tr '\n' ';'); bridge ports: $(node f bridge link 2>/dev/null | wc -l)"

# S20.3 does the sender feel the queue? 1200-byte datagrams at 400/s for 4 s into 1 mbit; the elapsed time of the client says it
backpressure() { # LABEL SETUP_COMMANDS_FUNCTION
  direct_pair
  "$2"
  send 10.250.0.2 1600 400 1200
  info S20.3 "$1: the client took $(printf '%s' "$CLIENT" | field 'str(d["client_elapsed_s"]) + " s of " + str(d["planned_s"])'); got $(printf '%s' "$REPORT" | field 'd["received"]') of 1600"
}
tree_htb_only() { node a tc qdisc add dev l0a root handle 1: htb default 10; node a tc class add dev l0a parent 1: classid 1:10 htb rate 1mbit ceil 1mbit burst 3500; node a tc qdisc add dev l0a parent 1:10 handle 10: pfifo limit 1000; }
info S20.3 "net.core.wmem_max in a node: $(node a cat /proc/sys/net/core/wmem_max 2>&1); writable: $(node a sh -c 'echo 8388608 > /proc/sys/net/core/wmem_max' 2>&1 && echo yes || echo no); now $(node a cat /proc/sys/net/core/wmem_max 2>&1)"
backpressure "htb root, pfifo leaf (no netem)" tree_htb_only
if [ "$HAS_NETEM" = 1 ]; then
  tree_netem_root() { node a tc qdisc add dev l0a root handle 1: netem limit 1000; node a tc qdisc add dev l0a parent 1: handle 2: htb default 10; node a tc class add dev l0a parent 2: classid 2:10 htb rate 1mbit ceil 1mbit burst 3500; node a tc qdisc add dev l0a parent 2:10 handle 10: pfifo limit 1000; }
  backpressure "netem root (no impairment), htb child, pfifo leaf" tree_netem_root
  tree_netem_only() { node a tc qdisc add dev l0a root netem limit 1000 rate 1mbit; }
  backpressure "netem root with a rate only (what 0.1 and 0.2 build)" tree_netem_only
  tree_delay_1us() { first_stage a l0a 1mbit 1000 delay 1us; }
  backpressure "netem root with delay 1us, htb child, pfifo leaf" tree_delay_1us
  tree_rate_big() { first_stage a l0a 1mbit 1000 rate 100gbit; }
  backpressure "netem root with rate 100gbit, htb child, pfifo leaf" tree_rate_big
  tree_jitter_0() { first_stage a l0a 1mbit 1000 delay 1us 0us; }
  backpressure "netem root with delay 1us 0us, htb child, pfifo leaf" tree_jitter_0
else
  info S20.3 "netem trees: skipped, no netem"
fi

if [ "$HAS_NETEM" = 1 ]; then
  # S20.2 the order of the stages with a jitter and a saturated shaper: 1 mbit, delay 20 ms, jitter 5 ms (normal), 1200-byte
  # datagrams at 120 per second (1.19 mbit on the wire, a little over), a queue of 15000 bytes, 1000 packets. The standard
  # deviation of the delays of the second half is the jitter plus the (small) changes of the queue.
  for build in direct fabric-mirred delay-first; do
    case "$build" in
      direct) direct_pair
        node a tc qdisc add dev l0a root handle 1: netem limit 12 rate 1mbit
        node a tc qdisc add dev l0a parent 1: handle 2: netem limit 1000 delay 20ms 5ms distribution normal;;
      fabric-mirred) fabric_pair mirred
        first_stage a fa0 1mbit 12 delay 1us
        node f tc qdisc add dev pb root netem limit 1000 delay 20ms 5ms distribution normal;;
      delay-first) direct_pair
        node a tc qdisc add dev l0a root handle 1: netem limit 1000 delay 20ms 5ms distribution normal
        node a tc qdisc add dev l0a parent 1: handle 2: htb default 10
        node a tc class add dev l0a parent 2: classid 2:10 htb rate 1mbit ceil 1mbit burst 1600 cburst 1600
        node a tc qdisc add dev l0a parent 2:10 handle 10: pfifo limit 12;;
    esac
    send 10.250.0.2 1200 120 1200
    info S20.2 "$build: $(printf '%s' "$REPORT" | field '"received %d of 1200, delay p50 %.1f ms, stdev of the delays (second half) %.2f ms, stdev of the gaps between arrivals %.2f ms (a shaper after the delay gives gaps of 10 ms exactly; configured jitter 5 ms), reordered %d" % (d["received"], d["delay_ms"]["p50"], d["stdev_tail_ms"], d["gap_stdev_ms"], d["reordered"])') ; the client took $(printf '%s' "$CLIENT" | field 'str(d["client_elapsed_s"]) + " s of " + str(d["planned_s"])')"
  done

  # S20.4 the seeded loss pattern: direct against fabric, 10 % loss, seed 1234, 2000 packets
  for build in direct fabric-mirred-with-shaper; do
    case "$build" in
      direct) direct_pair; node a tc qdisc add dev l0a root netem loss 10% seed 1234;;
      fabric-mirred-with-shaper) fabric_pair mirred; first_stage a fa0 100mbit 1000 loss 10% seed 1234 delay 1us;;
    esac
    send 10.250.0.2 2000 1000 200
    info S20.4 "$build: lost $(printf '%s' "$REPORT" | field 'str(d["missing_count"]) + " " + d["missing_hash"]')"
  done
fi

# S20.5 take the fabric port towards b down: what do a and b see, and what does a sender get?
fabric_pair bridge
node f ip link set pb down
info S20.5 "pb down: b's interface: $(node b ip -br link show dev fb0 | awk '{print $2, $3}'); a's interface: $(node a ip -br link show dev fa0 | awk '{print $2}'); the bridge port: $(node f bridge link 2>/dev/null | grep -c 'state forwarding') forwarding"
send 10.250.0.2 200 200 200
info S20.5 "pb down, a sends 200 packets: client says $(printf '%s' "$CLIENT" | cut -c1-120); received $(printf '%s' "$REPORT" | field 'd["received"]')"
node f ip link set pb up; sleep 0.5
send 10.250.0.2 200 200 200
info S20.5 "pb up again: received $(printf '%s' "$REPORT" | field 'd["received"]') of 200"

# S20.5b the node-side interface down (what 0.2 does): what does the sender get?
fabric_pair mirred
node a ip link set fa0 down
send 10.250.0.2 200 200 200
info S20.5 "mirred fabric, fa0 down at a: $(printf '%s' "$CLIENT" | field '"sender errors %d (%s)" % (d["send_errors"], d["first_error"])'); received $(printf '%s' "$REPORT" | field 'd["received"]'); b's interface: $(node b ip -br link show dev fb0 | awk '{print $2}')"

# S20.6 TCP with no limit across the direct link and across the fabric: what does the hop cost?
for build in direct bridge mirred; do
  case "$build" in direct) direct_pair;; *) fabric_pair "$build";; esac
  for n in "${!NODE_PID[@]}"; do for i in l0a l0b fa0 fb0 pa pb; do node "$n" ethtool -K "$i" gso off tso off gro off >/dev/null 2>&1; done; done
  out="$(mktemp)"
  node b $TCPFLOW server 10.250.0.2 7100 1 4 >"$out" 2>&1 &
  spid=$!
  sleep 0.5
  nsenter -t "${NODE_PID[a]}" -n $TCPFLOW client 10.250.0.2 7100 4 reno >/dev/null 2>&1
  wait "$spid"
  info S20.6 "$build, one TCP flow with no limit, offloads off: $(tail -n 1 "$out" | field 'round(sum(d["bytes_per_second"][0][1:]) * 8 / 3 / 1e6)') Mbit/s"
  rm -f "$out"
done

# S20.7 does a rate that steps follow in the fabric build (the htb class at the node, below a netem root with 1 us), with the sender not
# pushed back? 6 mbit and 2 mbit alternating in steps of 100 and 20 ms (as S19), 1000-byte datagrams at 720 per second
if [ "$HAS_NETEM" = 1 ]; then
  STEPS="python3 $SPIKE_DIR/trace_steps.py"
  for step in 100 20; do
    fabric_pair mirred
    first_stage a fa0 6000kbit 1000 delay 1us
    node f tc qdisc add dev pb root netem limit 1000 delay 20ms
    dump="$(mktemp)"; t0="$(mktemp)"; out="$(mktemp)"; drv="$(mktemp)"; cl="$(mktemp)"
    count=$((720 * 9))
    FLOW_DUMP="$dump" node b $FLOW server 4 10.250.0.2 7000 "$count" 2.0 >"$out" 2>&1 &
    spid=$!
    sleep 0.4
    FLOW_SIZE=1000 nsenter -t "${NODE_PID[a]}" -n $FLOW client 4 10.250.0.2 7000 "$count" 720 >"$cl" 2>&1 &
    cpid=$!
    $STEPS drive "${NODE_PID[a]}" fa0 htb2 "$step" 6 2000 6000 "$t0" >"$drv" 2>&1
    wait "$cpid"; wait "$spid"
    info S20.7 "fabric build, htb at the node, steps of $step ms: $($STEPS analyze "$dump" "$t0" "$step" 6 2000 6000 1042 2>&1 | tail -n 1); the sender took $(tail -n 1 "$cl" | field 'str(d["client_elapsed_s"]) + " s of " + str(d["planned_s"])')"
    rm -f "$dump" "$t0" "$out" "$drv" "$cl"
  done
else
  info S20.7 "skipped, no netem"
fi
