#!/usr/bin/env bash
# S16: can the Kernel backend build networks of many nodes, with routers, static routes, several paths and IPv6?
#
# Release 0.3 makes the topology a graph. Questions this answers (no `netem` is needed except for S16.9):
#   S16.1  How long does it take to build a chain of 2, 6, 12 and 24 nodes?
#   S16.2  Do static routes and forwarding carry a UDP flow over the chain, and does it stop without forwarding (canary)?
#   S16.3  What does a router add to the delay (the overhead per hop, with no impairment)?
#   S16.4  Does a multipath route (ECMP) split flows over two paths, with the hash on the ports and not on the addresses?
#   S16.5  Failover with metric routes when a link goes down: which end of the link goes down, and does the kernel use the
#          route of the other path (ignore_routes_with_linkdown)?
#   S16.6  IPv6 over a chain with DAD, router advertisements and autoconf off: does it work, and what do the interfaces
#          send on their own (stray packets are what shifted a seeded loss pattern in S2)?
#   S16.7  The same for IPv4 (ARP is replaced by fixed neighbours).
#   S16.8  A dual stack node.
#   S16.9  With netem: does the seeded loss pattern stay the same when the network around it is bigger? (skipped without netem)
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
if [ "${1:-outer}" = outer ]; then
  enter_ns S16.0 "$0"
fi
. "$SPIKE_DIR/lab.sh"
trap lab_stop EXIT
FLOW="python3 $SPIKE_DIR/flow.py"
info S16.0 "kernel $(uname -r); $(ip -V 2>&1 | head -n 1)"

# build_chain N [v6]: nodes c0..c{N-1}, link i joins c{i} and c{i+1}; v4 10.200.i.0/30 (left .1, right .2),
# v6 fd00:200:i::/126 (left ::1, right ::2); every node but the ends forwards; static routes towards both ends.
build_chain() {
  local n="$1" v6="${2:-}" i j
  for ((i = 0; i < n; i++)); do node_new "c$i" "$v6"; done
  for ((i = 0; i < n - 1; i++)); do
    if [ -n "$v6" ]; then
      LINK_FLAGS=nodad link_new "$i" "c$i" "c$((i + 1))" "fd00:200:$i::1/126" "fd00:200:$i::2/126" || return 1
    else
      link_new "$i" "c$i" "c$((i + 1))" "10.200.$i.1/30" "10.200.$i.2/30" || return 1
    fi
  done
  for ((i = 1; i < n - 1; i++)); do node_forward "c$i"; done
  for ((i = 0; i < n; i++)); do
    for ((j = 0; j < n - 1; j++)); do
      if [ "$j" -ge "$((i + 1))" ]; then # a subnet to the right
        if [ -n "$v6" ]; then node "c$i" ip -6 route replace "fd00:200:$j::/126" via "fd00:200:$i::2" dev "l${i}a"
        else node "c$i" ip route replace "10.200.$j.0/30" via "10.200.$i.2" dev "l${i}a"; fi
      elif [ "$j" -le "$((i - 2))" ]; then # a subnet to the left
        if [ -n "$v6" ]; then node "c$i" ip -6 route replace "fd00:200:$j::/126" via "fd00:200:$((i - 1))::1" dev "l$((i - 1))b"
        else node "c$i" ip route replace "10.200.$j.0/30" via "10.200.$((i - 1)).1" dev "l$((i - 1))b"; fi
      fi
    done
  done
}

# send_flow FAMILY FROMNODE TONODE ADDR COUNT PPS [SRC_PORT]  prints the JSON line of the receiver
send_flow() {
  local fam="$1" from="$2" to="$3" addr="$4" count="$5" pps="$6" sport="${7:-0}" out
  out="$(mktemp)"
  node "$to" $FLOW server "$fam" "$addr" 7000 "$count" 1.5 >"$out" 2>&1 &
  local spid=$!
  sleep 0.4
  node "$from" $FLOW client "$fam" "$addr" 7000 "$count" "$pps" "$sport" >/dev/null 2>&1
  wait "$spid"
  tail -n 1 "$out"
  rm -f "$out"
}
field() { python3 -c 'import json,sys; d=json.loads(sys.stdin.read()); print(eval(sys.argv[1], {"d": d}))' "$1" 2>/dev/null; }

# S16.1 the time to build
for n in 2 6 12 24; do
  t0="$(now_ms)"
  if build_chain "$n"; then
    t1="$(now_ms)"
    info S16.1 "a chain of $n nodes ($((n - 1)) links, routes) is built in $((t1 - t0)) ms"
  else
    fail S16.1 "cannot build a chain of $n nodes"
  fi
  lab_stop
done

# S16.2 a flow over 3 routers (5 nodes, 4 hops), then without forwarding on one router
build_chain 5 || { fail S16.2 "no chain of 5"; exit 0; }
report="$(send_flow 4 c0 c4 10.200.3.2 300 500)"
received="$(printf '%s' "$report" | field 'd["received"]')"
if [ "${received:-0}" = 300 ]; then pass S16.2 "300 of 300 packets over 4 hops with static routes"; else fail S16.2 "received ${received:-?} of 300 :: $report"; fi
node c2 sysctl -qw net.ipv4.ip_forward=0
report="$(send_flow 4 c0 c4 10.200.3.2 100 500)"
received="$(printf '%s' "$report" | field 'd["received"]')"
if [ "${received:-x}" = 0 ]; then pass S16.2 "canary: without forwarding on a router nothing arrives"; else fail S16.2 "canary: $received packets arrived without forwarding"; fi
node c2 sysctl -qw net.ipv4.ip_forward=1

# S16.3 the overhead per hop: p50 and mean one-way delay over 1 hop (c0 -> c1) and over 4 hops
r1="$(send_flow 4 c0 c1 10.200.0.2 1000 1000)"
r4="$(send_flow 4 c0 c4 10.200.3.2 1000 1000)"
info S16.3 "one hop: p50 $(printf '%s' "$r1" | field '"%.4f" % d["delay_ms"]["p50"]') ms, p99 $(printf '%s' "$r1" | field '"%.4f" % d["delay_ms"]["p99"]') ms"
info S16.3 "four hops: p50 $(printf '%s' "$r4" | field '"%.4f" % d["delay_ms"]["p50"]') ms, p99 $(printf '%s' "$r4" | field '"%.4f" % d["delay_ms"]["p99"]') ms"
lab_stop

# S16.4 ECMP: a -- r1 -- b and a -- r2 -- b; 20 flows with different source ports
multipath() {
  node_new a; node_new r1; node_new r2; node_new b
  link_new 0 a r1 10.201.0.1/30 10.201.0.2/30
  link_new 1 r1 b 10.201.1.1/30 10.201.1.2/30
  link_new 2 a r2 10.201.2.1/30 10.201.2.2/30
  link_new 3 r2 b 10.201.3.1/30 10.201.3.2/30
  node_forward r1; node_forward r2
  node r1 ip route replace 10.201.3.0/30 via 10.201.1.2 dev l1a
  node r1 ip route replace 10.201.2.0/30 via 10.201.0.1 dev l0b
  node r2 ip route replace 10.201.1.0/30 via 10.201.3.2 dev l3a
  node r2 ip route replace 10.201.0.0/30 via 10.201.2.1 dev l2b
  node b ip route replace 10.201.0.0/30 via 10.201.1.1 dev l1b
  node b ip route replace 10.201.2.0/30 via 10.201.3.1 dev l3b
  # b is reached by its address on link 1 over r1 or by its address on link 3 over r2: the destination for the
  # multipath is an address both paths lead to, so b gets a service address on lo
  node b ip addr add 10.209.0.1/32 dev lo
  node a ip route replace 10.209.0.1/32 nexthop via 10.201.0.2 dev l0a nexthop via 10.201.2.2 dev l2a
  node r1 ip route replace 10.209.0.1/32 via 10.201.1.2 dev l1a
  node r2 ip route replace 10.209.0.1/32 via 10.201.3.2 dev l3a
  node b ip route replace 10.201.1.1/32 via 10.201.1.1 dev l1b 2>/dev/null
}
flows_over_two_paths() { # POLICY
  node a sysctl -qw "net.ipv4.fib_multipath_hash_policy=$1"
  local before0 before2 after0 after2 port
  before0="$(rx_tx a l0a | awk '{print $2}')"; before2="$(rx_tx a l2a | awk '{print $2}')"
  node b $FLOW server 4 10.209.0.1 7000 100000 1.0 >/dev/null 2>&1 &
  local spid=$!
  sleep 0.3
  for port in $(seq 20001 20020); do node a $FLOW client 4 10.209.0.1 7000 20 2000 "$port"; done
  sleep 0.3; kill "$spid" 2>/dev/null; wait "$spid" 2>/dev/null
  after0="$(rx_tx a l0a | awk '{print $2}')"; after2="$(rx_tx a l2a | awk '{print $2}')"
  echo "$((after0 - before0)) $((after2 - before2))"
}
multipath
split="$(flows_over_two_paths 1)"
set -- $split
if [ "$1" -gt 40 ] && [ "$2" -gt 40 ]; then pass S16.4 "hash on the ports: 20 flows of 20 packets went $1 over path 1 and $2 over path 2"; else warn S16.4 "hash on the ports: $1 and $2 packets"; fi
split="$(flows_over_two_paths 0)"
set -- $split
if [ "$1" -eq 0 ] || [ "$2" -eq 0 ]; then pass S16.4 "canary: hash on the addresses only: all 20 flows took one path ($1 and $2 packets)"; else warn S16.4 "address hash split the flows: $1 and $2"; fi

# S16.5 failover: metric routes a -> b over r1 (metric 10) and r2 (metric 20). A stream of 500 packets per second for
# 3 s; at 1 s a link of the first path goes down; which end, and with ignore_routes_with_linkdown 0 or 1?
failover() { # END (a|r1) IGNORE (0|1)
  local end="$1" ignore="$2" iface
  lab_stop; multipath
  node a ip route del 10.209.0.1/32 2>/dev/null
  node a ip route add 10.209.0.1/32 via 10.201.0.2 dev l0a metric 10
  node a ip route add 10.209.0.1/32 via 10.201.2.2 dev l2a metric 20
  node a sysctl -qw net.ipv4.conf.all.ignore_routes_with_linkdown="$ignore" net.ipv4.conf.l0a.ignore_routes_with_linkdown="$ignore"
  local out; out="$(mktemp)"
  node b $FLOW server 4 10.209.0.1 7000 1500 1.5 >"$out" 2>&1 &
  local spid=$!
  sleep 0.4
  node a $FLOW client 4 10.209.0.1 7000 1500 500 &
  local cpid=$!
  sleep 1.0
  if [ "$end" = a ]; then node a ip link set l0a down; else node r1 ip link set l0b down; fi
  wait "$cpid"; wait "$spid"
  REPORT="$(tail -n 1 "$out")"; rm -f "$out"
}
for end in a r1; do
  for ignore in 0 1; do
    failover "$end" "$ignore"
    lost="$(printf '%s' "$REPORT" | field 'd["missing_count"]')"
    first="$(printf '%s' "$REPORT" | field 'd["missing_first"][:3]')"
    info S16.5 "link of the first path down at the end '$end', ignore_routes_with_linkdown=$ignore: $lost of 1500 lost (first $first), i.e. an outage of about $(( ${lost:-0} * 2 )) ms"
  done
done
lab_stop

# S16.6 and S16.7 stray packets: what does a node send on its own after the links are up?
for fam in 4 6; do
  if [ "$fam" = 6 ]; then build_chain 3 v6; else build_chain 3; fi
  # the interfaces are up; the counters are read after 0.2 s, 1 s and 3 s of silence
  snap() { local k s=""; for k in l0a l0b l1a l1b; do
      case $k in l0a) set -- c0 "$k";; l0b) set -- c1 "$k";; l1a) set -- c1 "$k";; l1b) set -- c2 "$k";; esac
      s="$s $(rx_tx "$1" "$2" | tr ' ' '/')"; done; echo "$s"; }
  sleep 0.2; a="$(snap)"; sleep 0.8; b="$(snap)"; sleep 2; c="$(snap)"
  info S16.6 "IPv$fam chain of 3, rx/tx packets of the 4 interfaces at 0.2 s, 1 s, 3 s:$a |$b |$c"
  if [ "$b" = "$c" ]; then pass S16.6 "IPv$fam: the interfaces are quiet after the first second"; else warn S16.6 "IPv$fam: packets still appear between 1 s and 3 s"; fi
  if [ "$fam" = 6 ]; then
    report="$(send_flow 6 c0 c2 fd00:200:1::2 300 500)"
    received="$(printf '%s' "$report" | field 'd["received"]')"
    if [ "${received:-0}" = 300 ]; then pass S16.7 "IPv6: 300 of 300 packets over 2 hops"; else fail S16.7 "IPv6: received ${received:-?} of 300 :: $report"; fi
  fi
  lab_stop
done

# S16.8 a dual stack node pair
node_new d1 v6; node_new d2 v6
link_new 0 d1 d2 10.210.0.1/30 10.210.0.2/30
LINK_FLAGS=nodad link_new 10 d1 d2 fd00:210::1/126 fd00:210::2/126
r4="$(send_flow 4 d1 d2 10.210.0.2 100 500)"; r6="$(send_flow 6 d1 d2 fd00:210::2 100 500)"
info S16.8 "dual stack: v4 $(printf '%s' "$r4" | field 'd["received"]') of 100, v6 $(printf '%s' "$r6" | field 'd["received"]') of 100"
lab_stop

# S16.9 netem needed: the seeded loss pattern of the first hop with a small and with a big network around it
if ! tc qdisc add dev lo root netem delay 1ms 2>/dev/null; then
  info S16.9 "skipped: this kernel has no netem"
else
  tc qdisc del dev lo root 2>/dev/null
  pattern() { # EXTRA (0|1)
    lab_stop; build_chain 3
    if [ "$1" = 1 ]; then
      for ((k = 0; k < 12; k++)); do node_new "x$k" v6; done
      for ((k = 0; k < 11; k++)); do LINK_FLAGS=nodad link_new "$((50 + k))" "x$k" "x$((k + 1))" "fd00:2ff:$k::1/126" "fd00:2ff:$k::2/126"; done
    fi
    node c0 tc qdisc add dev l0a root netem loss 10% seed 1234
    sleep 1.5
    PATTERN="$(send_flow 4 c0 c2 10.200.1.2 2000 1000 | field 'd["missing_hash"] + " " + str(d["missing_count"])')"
  }
  pattern 0; p1="$PATTERN"; pattern 0; p2="$PATTERN"; pattern 1; p3="$PATTERN"
  info S16.9 "seed 1234, 10 % loss, 2000 packets: small network $p1 | again $p2 | with 12 idle IPv6 nodes around $p3"
  if [ "$p1" = "$p2" ] && [ "$p2" = "$p3" ]; then pass S16.9 "the pattern is the same with a bigger network around it"; else fail S16.9 "the pattern changed"; fi
  lab_stop
fi
