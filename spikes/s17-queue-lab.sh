#!/usr/bin/env bash
# S17: the queue lab. How does a direction look when the rate lives in a shaper with a queue discipline behind it?
#
# Release 0.3 adds queue limits and disciplines (LNK-09) and trace-driven rates (LMD-03). A `netem` with a `rate` fixes the
# moment a packet leaves when the packet enters (D-070), so a rate that drops while packets wait is applied late, and
# CoDel, FQ-CoDel and PIE need a queue of their own. Questions (S17.n; a refusal is a result, not a failure):
#   S17.1  Which trees does the kernel accept (tbf or htb with a discipline as leaf, a netem in front of a shaper)?
#   S17.2  What does `limit` mean and what does a shaper count (L3 or Ethernet bytes)? Tail drop against its model.
#   S17.3  Do codel, fq_codel and pie keep the delay near their target under an overload that does not answer?
#   S17.4  A rate that drops while packets wait: a tbf follows at once, a netem rate does not (D-070). Both measured.
#   S17.5  What does `tc -s -j` say about each qdisc (the fields a probe can read)? Does tbf need the whole state?
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
if [ "${1:-outer}" = outer ]; then
  enter_ns S17.0 "$0"
fi
. "$SPIKE_DIR/lab.sh"
trap lab_stop EXIT
FLOW="python3 $SPIKE_DIR/flow.py"
info S17.0 "kernel $(uname -r); $(tc -V 2>&1 | head -n 1)"

pair() { lab_stop; node_new a; node_new b; link_new 0 a b 10.220.0.1/30 10.220.0.2/30; }
field() { python3 -c 'import json,sys; d=json.loads(sys.stdin.read()); print(eval(sys.argv[1], {"d": d}))' "$1" 2>/dev/null; }
has_qdisc() { tc qdisc add dev lo root "$@" 2>/dev/null && { tc qdisc del dev lo root; return 0; }; return 1; }

# try ID text cmd...   a refusal is a result
try() { local id="$1" text="$2" err; shift 2
  if err="$("$@" 2>&1)"; then pass "$id" "$text: accepted"; else info "$id" "$text: REFUSED :: $(printf '%s' "$err" | head -n 1)"; fi; }

# S17.1 trees
for leaf in "pfifo limit 100" "bfifo limit 20000" "codel" "fq_codel" "pie" "sfq"; do
  pair
  node a tc qdisc add dev l0a root handle 1: tbf rate 1mbit burst 6000 limit 60000 || { fail S17.1 "no tbf"; continue; }
  try S17.1 "tbf root, leaf $leaf" node a tc qdisc add dev l0a parent 1: handle 10: $leaf
done
pair
node a tc qdisc add dev l0a root handle 1: htb default 10
node a tc class add dev l0a parent 1: classid 1:10 htb rate 1mbit burst 6000
try S17.1 "htb root, class with rate, leaf fq_codel" node a tc qdisc add dev l0a parent 1:10 handle 10: fq_codel
pair
node a tc qdisc add dev l0a root handle 1: htb default 10
node a tc class add dev l0a parent 1: classid 1:10 htb rate 1mbit burst 6000
try S17.1 "htb root, class with rate, leaf netem delay" node a tc qdisc add dev l0a parent 1:10 handle 10: netem delay 20ms
pair
if node a tc qdisc add dev l0a root handle 1: netem delay 20ms 2>/dev/null; then
  try S17.1 "netem root (delay), child tbf" node a tc qdisc add dev l0a parent 1: handle 2: tbf rate 1mbit burst 6000 limit 60000
  try S17.1 "netem root (delay), child tbf, leaf codel under the tbf" node a tc qdisc add dev l0a parent 2: handle 3: codel
else
  info S17.1 "netem root: skipped, this kernel has no netem"
fi
pair
node a tc qdisc add dev l0a root handle 1: tbf rate 1mbit burst 6000 limit 60000
try S17.1 "tbf root, child netem (delay after the rate, if it works)" node a tc qdisc add dev l0a parent 1: handle 2: netem delay 20ms

# overload run: SIZE bytes per datagram, PPS, COUNT; the receiver reports delays and bins into REPORT
overload() {  local size="$1" pps="$2" count="$3" out; out="$(mktemp)"
  node b $FLOW server 4 10.220.0.2 7000 "$count" 2.0 >"$out" 2>&1 &
  local spid=$!
  sleep 0.4
  FLOW_SIZE="$size" node a $FLOW client 4 10.220.0.2 7000 "$count" "$pps" >/dev/null 2>&1
  wait "$spid"
  REPORT="$(tail -n 1 "$out")"; rm -f "$out"
}

# S17.2 tail drop of a tbf: 1 mbit, limit 15000 bytes, 1200-byte datagrams at 400 per second for 3 s (3.9 mbit offered)
pair
node a tc qdisc add dev l0a root tbf rate 1mbit burst 6000 limit 15000
overload 1200 400 1200
rec="$(printf '%s' "$REPORT" | field 'd["received"]')"; dmax="$(printf '%s' "$REPORT" | field '"%.1f" % d["delay_max_ms"]')"
# Delivered bits per second in the steady part: the received count minus the burst and the queue (about 6000 + 15000 bytes)
info S17.2 "tbf 1 mbit limit 15000: $rec of 1200 arrived, largest delay $dmax ms (model: queue 15000 B at 1 mbit = 120 ms, plus one burst of 6000 B = 48 ms at most); in 3 s a shaper at 1 mbit of L3 bytes passes about $(( (1000000 * 3 / 8 + 15000 + 6000) / 1228 )) datagrams of 1228 B, of Ethernet frames about $(( (1000000 * 3 / 8 + 15000 + 6000) / 1242 ))"

# S17.3 disciplines under an overload that does not answer: 1200 B at 400/s for 4 s into 1 mbit
for leaf in "pfifo limit 100" "codel" "fq_codel" "pie"; do
  pair
  node a tc qdisc add dev l0a root handle 1: tbf rate 1mbit burst 6000 limit 400000 || continue
  if ! node a tc qdisc add dev l0a parent 1: handle 10: $leaf 2>/dev/null; then info S17.3 "$leaf: this kernel has no such qdisc"; continue; fi
  overload 1200 400 1600
  info S17.3 "tbf 1 mbit + $leaf: $(printf '%s' "$REPORT" | field '"received %d of 1600, delay p50 %.1f ms, p99 %.1f ms, max %.1f ms" % (d["received"], d["delay_ms"]["p50"], d["delay_ms"]["p99"], d["delay_max_ms"])')"
done

# S17.4 a rate that drops while packets wait. 3.2 mbit of 1000-byte datagrams (400/s) for 9 s; the rate is 4 mbit, 1.2 mbit from
# 3 s to 5 s, then 4 mbit again. Kilobits delivered in each second, by arrival time since the first packet.
steps() { # ARM
  local arm="$1"
  sleep 3.0
  if [ "$arm" = tbf ]; then node a tc qdisc change dev l0a root tbf rate 1200kbit burst 6000 limit 400000
  else node a tc qdisc change dev l0a root netem limit 1000 rate 1200kbit; fi
  sleep 2.0
  if [ "$arm" = tbf ]; then node a tc qdisc change dev l0a root tbf rate 4mbit burst 6000 limit 400000
  else node a tc qdisc change dev l0a root netem limit 1000 rate 4mbit; fi
}
for arm in tbf netem; do
  pair
  if [ "$arm" = tbf ]; then node a tc qdisc add dev l0a root tbf rate 4mbit burst 6000 limit 400000
  elif ! node a tc qdisc add dev l0a root netem limit 1000 rate 4mbit 2>/dev/null; then info S17.4 "arm netem: skipped, no netem"; continue; fi
  out="$(mktemp)"
  node b $FLOW server 4 10.220.0.2 7000 3600 2.5 >"$out" 2>&1 &
  spid=$!
  sleep 0.4
  steps "$arm" &
  stpid=$!
  FLOW_SIZE=1000 node a $FLOW client 4 10.220.0.2 7000 3600 400 >/dev/null 2>&1
  wait "$stpid"; wait "$spid"
  REPORT="$(tail -n 1 "$out")"; rm -f "$out"
  per_second="$(printf '%s' "$REPORT" | field '" ".join(str(sum(d["bins"][i:i+10]) * 8) for i in range(0, len(d["bins"]), 10))')"
  info S17.4 "arm $arm, kbit delivered in seconds 0, 1, 2, ... (packets of 1042 B on the wire count 8.3 kbit): $per_second; delay max $(printf '%s' "$REPORT" | field '"%.0f" % d["delay_max_ms"]') ms"
done

# S17.5 the statistics and the whole state
pair
node a tc qdisc add dev l0a root handle 1: tbf rate 1mbit burst 6000 limit 60000
node a tc qdisc add dev l0a parent 1: handle 10: fq_codel 2>/dev/null || node a tc qdisc add dev l0a parent 1: handle 10: pfifo limit 100
overload 1200 400 400
info S17.5 "tc -s -j qdisc show: $(node a tc -s -j qdisc show dev l0a 2>&1 | python3 -c '
import json,sys
try:
    for q in json.load(sys.stdin):
        print(q.get("kind"), sorted(k for k in q if k not in ("kind","handle","parent","root")), end=" | ")
except Exception as e:
    print("unreadable:", e)')"
try S17.5 "tbf change without limit or latency (does it keep the old one?)" node a tc qdisc change dev l0a root handle 1: tbf rate 2mbit burst 6000
try S17.5 "tbf change with the whole state" node a tc qdisc change dev l0a root handle 1: tbf rate 2mbit burst 6000 limit 60000
info S17.5 "after the change: $(node a tc qdisc show dev l0a | tr '\n' ';')"

# S17.6 does the sender feel the queue? A datagram stays charged to the socket of the sender (SO_SNDBUF) until it has left
# the queue of the interface, so a queue on the interface of the sending node backpressures `sendto`: the sender is slowed
# down instead of the queue dropping. 1200-byte datagrams at 400 per second for 4 s into a tbf of 1 mbit with a queue of
# 1000 packets: if the queue were the limit the send would take 4 s; the receiver sees the delay.
net_core="$(node a sysctl -n net.core.wmem_default net.core.wmem_max 2>/dev/null | tr '\n' ' ')"
info S17.6 "net.core.wmem_default and wmem_max in a node: $net_core"
for variant in "default:" "SNDBUF=4MiB:FLOW_SNDBUF=4194304" "SNDBUFFORCE=4MiB:FLOW_SNDBUFFORCE=4194304"; do
  label="${variant%%:*}"; envs="${variant#*:}"
  pair
  node a tc qdisc add dev l0a root handle 1: tbf rate 1mbit burst 6000 limit 2000000
  node a tc qdisc add dev l0a parent 1: handle 10: pfifo limit 1000
  out="$(mktemp)"; cl="$(mktemp)"
  node b $FLOW server 4 10.220.0.2 7000 1600 2.0 >"$out" 2>&1 &
  spid=$!
  sleep 0.4
  env FLOW_SIZE=1200 $envs nsenter -t "${NODE_PID[a]}" -n $FLOW client 4 10.220.0.2 7000 1600 400 >"$cl" 2>&1
  wait "$spid"
  dropped="$(node a tc -s qdisc show dev l0a | awk '/pfifo/ {f=1} f && /dropped/ {print $7; exit}')"
  info S17.6 "$label: the client took $(tail -n 1 "$cl" | field 'str(d["client_elapsed_s"]) + " s of " + str(d["planned_s"]) + ", sndbuf " + str(d["sndbuf"]) + " " + d["note"]'); receiver $(tail -n 1 "$out" | field '"got %d, delay max %.0f ms" % (d["received"], d["delay_max_ms"])'); pfifo dropped $dropped"
  rm -f "$out" "$cl"
done
