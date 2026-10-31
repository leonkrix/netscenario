#!/usr/bin/env bash
# S13: capture in the agent. Builds the scratch crate and runs it in two namespaces joined by a veth pair (lib.sh).
#   bash spikes/rust-capture/run.sh
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [ "${1:-outer}" = outer ]; then
  . "$here/../lib.sh"
  command -v cargo >/dev/null || { fail S13.0 "no cargo"; exit 0; }
  cargo build --release --quiet --manifest-path "$here/Cargo.toml" || { fail S13.0 "the crate does not build"; exit 0; }
  enter_ns S13.0 "$here/run.sh"
fi
. "$here/../lib.sh"
work="$(mktemp -d)"
busy_pids=()
cleanup() {
  [ "${#busy_pids[@]}" -gt 0 ] && kill "${busy_pids[@]}" 2>/dev/null
  ns_stop
  rm -rf "$work"
}
trap cleanup EXIT
if ! pair_up; then
  fail S13.0 "the two namespaces and the veth pair could not be set up"
  exit 0
fi
bin="$here/target/release/spike-capture"
info S13.0 "rmem_max $(cat /proc/sys/net/core/rmem_max); $(tcpdump --version 2>&1 | head -n 1)"

# run_case NAME NETEM RATE SECONDS SIZE: captures on v1 (the receiving node) and v0 (the sending node) while a stream is sent
run_case() {
  local name="$1" netem="$2" rate="$3" seconds="$4" size="$5" sent
  tc qdisc replace dev v0 root netem limit 100000 $netem seed 5 || { fail S13.1 "$name: netem refused"; return; }
  in_ns "$bin" sink 900 &
  local listening=$!
  in_ns "$bin" capture v1 "$work/v1.pcap" 600 >"$work/v1.txt" 2>&1 &
  local receiving=$!
  "$bin" capture v0 "$work/v0.pcap" 600 >"$work/v0.txt" 2>&1 &
  local sending=$!
  sleep 0.4
  sent="$("$bin" send 10.9.0.2 "$rate" "$seconds" "$size")"
  wait "$receiving" "$sending" "$listening" 2>/dev/null
  info S13.1 "$name: $sent"
  info S13.1 "   v1, the receiving node: $(cat "$work/v1.txt")"
  info S13.1 "   v0, the sending node:   $(cat "$work/v0.txt")"
}

# 1. Where the capture sits: the sending node's interface shows what is left after the impairment
run_case "loss 50% at 5,000 pps for 1 s" "loss 50%" 5000 1 200
sent_in=5000
v0_udp="$(sed -n 's/.* udp=\([0-9]*\) .*/\1/p' "$work/v0.txt")"
v1_udp="$(sed -n 's/.* udp=\([0-9]*\) .*/\1/p' "$work/v1.txt")"
if [ "${v0_udp:-0}" -lt "$((sent_in * 6 / 10))" ] && [ "${v1_udp:-0}" -lt "$((sent_in * 6 / 10))" ]; then
  pass S13.2 "the capture on the sending interface shows $v0_udp of $sent_in packets, the one on the receiving interface $v1_udp: both see what is left after the impairment, never the packets the queue dropped"
else
  fail S13.2 "the capture on the sending interface showed $v0_udp, on the receiving one $v1_udp of $sent_in: expected about half"
fi

# 2. How completely the capture sees a stream, by rate (no impairment, 200 byte payload, the default buffer limit)
for rate in 10000 50000 100000 200000; do
  run_case "no impairment at $rate pps for 3 s" "delay 1ms" "$rate" 3 200
done

# 3. The same with a busy machine
for _ in $(seq 1 "$(nproc)"); do
  (while :; do :; done) &
  busy_pids+=("$!")
done
run_case "no impairment at 50000 pps for 3 s, every processor busy" "delay 1ms" 50000 3 200
kill "${busy_pids[@]}" 2>/dev/null
wait "${busy_pids[@]}" 2>/dev/null
busy_pids=()

# 4. Against tcpdump on the same interface and the same stream
tc qdisc replace dev v0 root netem limit 100000 delay 1ms seed 5
in_ns "$bin" sink 1500 &
sinking=$!
in_ns tcpdump -Z root -i v1 -U -nn -w "$work/ref.pcap" 'udp port 5000' >/dev/null 2>&1 &
reference=$!
in_ns "$bin" capture v1 "$work/ours.pcap" 600 >"$work/ours.txt" 2>&1 &
ours=$!
sleep 1
"$bin" send 10.9.0.2 20000 2 200 >/dev/null
sleep 0.8
kill -TERM "$reference" 2>/dev/null
wait "$ours" "$reference" "$sinking" 2>/dev/null
count_ref="$(tcpdump -Z root -nn -r "$work/ref.pcap" 2>/dev/null | wc -l)"
count_ours="$(tcpdump -Z root -nn -r "$work/ours.pcap" 2>"$work/ours.err" | wc -l)"
warnings="$(grep -vc '^reading from file' "$work/ours.err" || true)"
if [ "$count_ref" -eq "$count_ours" ] && [ "$count_ours" -gt 0 ]; then
  pass S13.3 "tcpdump reads our pcap without a complaint ($warnings warnings) and counts $count_ours packets, the same as the capture of tcpdump itself ($count_ref)"
else
  fail S13.3 "tcpdump counts $count_ours packets in our pcap and $count_ref in its own capture (warnings: $warnings)"
fi
# the timestamps are those of the kernel, so the same packet has the same time in both files
paste <(tcpdump -Z root -tt -nn -r "$work/ref.pcap" 2>/dev/null | awk '{print $1}') <(tcpdump -Z root -tt -nn -r "$work/ours.pcap" 2>/dev/null | awk '{print $1}') |
  awk 'BEGIN{m=0} {d=($1>$2?$1-$2:$2-$1); if (d>m) m=d} END{printf "INFO S13.3   largest difference between the two timestamps of the same packet: %.9f s over %d packets\n", m, NR}' | tee -a "$RESULTS"
tcpdump -Z root -nn -r "$work/ours.pcap" -c 2 2>/dev/null | sed 's/^/INFO S13.3   /' | tee -a "$RESULTS"
