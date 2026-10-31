#!/usr/bin/env bash
# S13b: with corruption, which packets can be told apart? Compares what the sender sent, what the queue dropped,
# what reached the receiving interface (counter and a raw socket) and what reached the UDP socket.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
if [ "${1:-outer}" = outer ]; then enter_ns S13b.0 "$0"; fi
work="$(mktemp -d)"
trap 'ns_stop; rm -rf "$work"' EXIT
pair_up || { fail S13b.0 "set-up failed"; exit 0; }
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin="$here/rust-capture/target/release/spike-capture"
rx() { in_ns awk '$1=="v1:"{print $3}' /proc/net/dev; }
for size in 64 200 1200; do
  for netem in "corrupt 10%" "loss 10%" "loss 5% corrupt 5%"; do
    tc qdisc del dev v0 root 2>/dev/null
    tc qdisc add dev v0 root netem limit 100000 $netem seed 9
    in_ns "$bin" sink 900 "$work/delivered.txt" & s=$!
    in_ns "$bin" capture v1 "$work/c.pcap" 600 "$work/seen.txt" >"$work/c.txt" 2>&1 & o=$!
    sleep 0.4
    before=$(rx)
    "$bin" send 10.9.0.2 10000 2 "$size" >/dev/null
    sleep 0.3
    after=$(rx)
    wait $o $s
    dropped=$(tc -s qdisc show dev v0 | sed -n 's/.*dropped \([0-9]*\),.*/\1/p' | head -1)
    python3 - "$work" "$size" "$netem" "$((after-before))" "${dropped:-0}" "$(cat "$work/c.txt")" <<'PY'
import sys
work, size, netem, arrived, dropped, cap = sys.argv[1:]
sent = set(range(20000))
seen = [int(x) for x in open(f"{work}/seen.txt").read().split()]
delivered = [int(x) for x in open(f"{work}/delivered.txt").read().split()]
fields = dict(f.split("=", 1) for f in cap.split() if "=" in f)
seen_s, deliv_s = set(seen), set(delivered)
corrupt_identified = seen_s - deliv_s            # on the wire with a readable sequence, not delivered
not_seen = sent - seen_s                         # never seen with a readable sequence
frames = int(fields["frames"])
unreadable = frames - len(seen)                  # frames on the wire whose header or sequence was damaged
print(f"INFO S13b {netem}, {size} B: sent 20000; the queue dropped {dropped}; the interface counted {arrived} frames; raw socket saw {frames} frames "
      f"(otherhost {fields['otherhost']}, readable udp {len(seen)}); the UDP socket got {len(delivered)}")
lost_or_unreadable = len(not_seen - deliv_s)
print(f"INFO S13b   told apart: delivered {len(deliv_s)}, corrupted and identified by sequence {len(corrupt_identified)}, "
      f"corrupted with a damaged header or sequence {unreadable}, lost in the queue {20000 - int(arrived)}; "
      f"check: delivered+identified+unreadable = {len(deliv_s)+len(corrupt_identified)+unreadable} against {arrived} frames at the interface")
PY
  done
done
