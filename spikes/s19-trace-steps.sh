#!/usr/bin/env bash
# S19: how well does a shaper follow a trace of rates, and at which resolution? (LMD-03, release 0.3)
#
# A staircase between 6 mbit and 2 mbit with steps of 100, 20, 5, 2 and 1 ms is applied by one persistent `tc -batch -` at
# absolute moments (S11: 0.03 ms per change). The path is a -- r -- b with the shaper on the interface of the router, a
# sender that offers 7.5 mbit all the time (so the shaper's queue is never empty and the output is its rate), and the
# arrivals recorded per packet. For each step the bytes that arrived are compared with rate x time. Arms: a `tbf` root
# (the rate is applied to the next packet; a change refills the bucket), `tbf-min` (the smallest burst), `htb` (a class is changed;
# its tokens are kept) and a `netem` root with a rate (D-070: applied to the packets that enter later).
# What the numbers can say about a packet of 1,042 bytes: at 6 mbit it takes 1.4 ms, so a step of 1 ms cannot be followed
# packet by packet and the error is large by construction; the point is where it starts to grow.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
if [ "${1:-outer}" = outer ]; then
  enter_ns S19.0 "$0"
fi
. "$SPIKE_DIR/lab.sh"
trap lab_stop EXIT
FLOW="python3 $SPIKE_DIR/flow.py"
STEPS="python3 $SPIKE_DIR/trace_steps.py"
info S19.0 "kernel $(uname -r); $(tc -V 2>&1 | head -n 1); $(nproc) CPUs"
field() { python3 -c 'import json,sys; d=json.loads(sys.stdin.read()); print(eval(sys.argv[1], {"d": d}))' "$1" 2>/dev/null; }

pair() {
  lab_stop; node_new a; node_new r; node_new b
  link_new 0 a r 10.240.0.1/30 10.240.0.2/30; link_new 1 r b 10.240.1.1/30 10.240.1.2/30
  node_forward r
  node a ip route replace 10.240.1.0/30 via 10.240.0.2 dev l0a
  node b ip route replace 10.240.0.0/30 via 10.240.1.1 dev l1b
}
SECONDS_RUN=6
for arm in tbf tbf-min htb netem; do
  for step in 100 20 5 2 1; do
    pair
    if [ "$arm" = tbf ]; then
      node r tc qdisc add dev l1a root handle 1: tbf rate 6000kbit burst 3500 limit 400000 || continue
    elif [ "$arm" = tbf-min ]; then
      node r tc qdisc add dev l1a root handle 1: tbf rate 6000kbit burst 1600 limit 400000 || continue
    elif [ "$arm" = htb ]; then
      node r tc qdisc add dev l1a root handle 1: htb default 10 || continue
      node r tc class add dev l1a parent 1: classid 1:10 htb rate 6000kbit ceil 6000kbit burst 1600 cburst 1600 || continue
    elif ! node r tc qdisc add dev l1a root handle 1: netem limit 100000 rate 6000kbit 2>/dev/null; then
      info S19.1 "arm netem: skipped, no netem"; break
    fi
    dump="$(mktemp)"; t0="$(mktemp)"; out="$(mktemp)"; drv="$(mktemp)"
    count=$((720 * (SECONDS_RUN + 3)))
    FLOW_DUMP="$dump" node b $FLOW server 4 10.240.1.2 7000 "$count" 2.0 >"$out" 2>&1 &
    spid=$!
    sleep 0.4
    FLOW_SIZE=1000 node a $FLOW client 4 10.240.1.2 7000 "$count" 720 >/dev/null 2>&1 &
    cpid=$!
    $STEPS drive "${NODE_PID[r]}" l1a "$arm" "$step" "$SECONDS_RUN" 2000 6000 "$t0" >"$drv" 2>&1
    wait "$cpid"; wait "$spid"
    info S19.1 "arm $arm, steps of $step ms: $($STEPS analyze "$dump" "$t0" "$step" "$SECONDS_RUN" 2000 6000 1042 2>&1 | tail -n 1); the writes of the steps: $(tail -n 1 "$drv" | field 'd["write_lateness_ms"]')"
    rm -f "$dump" "$t0" "$out" "$drv"
  done
done
