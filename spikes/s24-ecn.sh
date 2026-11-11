#!/usr/bin/env bash
# S24: does the kernel mark instead of drop, and does TCP in a namespace use the mark?
#
#   client namespace --veth-- server namespace, a shaper with a queue discipline on the client's interface
#
# For codel, fq_codel and pie with `ecn`, and for netem with `ecn`: one TCP flow with net.ipv4.tcp_ecn=1 (and a run with 0 for
# the contrast); the counters of the queue (`tc -s`: ecn_mark, drops) and of the receiver (CE-marked packets in /proc/net/netstat).
#   bash spikes/s24-ecn.sh
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$PATH:/usr/sbin:/sbin"
if [ "${1:-outer}" = outer ]; then
  if [ "$(id -u)" -eq 0 ]; then exec unshare -n bash "$0" inner; fi
  exec unshare -Urn bash "$0" inner
fi
secs="${SECS:-5}"
pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done; wait 2>/dev/null; }
trap cleanup EXIT
ns_new() { unshare -n sleep 3600 >/dev/null 2>&1 & pids+=($!); printf -v "$1" '%s' "$!"; for _ in $(seq 50); do nsenter -t "$!" -n true 2>/dev/null && return 0; sleep 0.1; done; return 1; }
nsmac() { nsenter -t "$1" -n ip -br link show dev "$2" | awk '{print $3}'; }
ns_new PC; ns_new PR; ns_new PS
for p in "$PC" "$PR" "$PS"; do nsenter -t "$p" -n sysctl -qw net.ipv6.conf.all.disable_ipv6=1; done
nsenter -t "$PR" -n sysctl -qw net.ipv4.ip_forward=1

run_case() {  # LABEL ECN(0|1) QDISC_ARGS...   the discipline sits under an htb class of 20 mbit on the router's interface to the server
  local label="$1" ecn="$2"; shift 2
  for n in "$PC:c0" "$PR:r0" "$PR:r1" "$PS:s0"; do nsenter -t "${n%%:*}" -n ip link del "${n##*:}" 2>/dev/null; done
  ip link add c0 type veth peer name r0 && ip link set c0 netns "$PC" && ip link set r0 netns "$PR"
  ip link add r1 type veth peer name s0 && ip link set r1 netns "$PR" && ip link set s0 netns "$PS"
  nsenter -t "$PC" -n ip addr add 10.9.0.1/24 dev c0; nsenter -t "$PR" -n ip addr add 10.9.0.2/24 dev r0
  nsenter -t "$PR" -n ip addr add 10.9.1.1/24 dev r1; nsenter -t "$PS" -n ip addr add 10.9.1.2/24 dev s0
  for x in "$PC:c0" "$PR:r0" "$PR:r1" "$PS:s0"; do nsenter -t "${x%%:*}" -n ip link set "${x##*:}" up gso_max_size 1514 gso_max_segs 1; done
  nsenter -t "$PC" -n ip route add 10.9.1.0/24 via 10.9.0.2; nsenter -t "$PS" -n ip route add 10.9.0.0/24 via 10.9.1.1
  for p in "$PC" "$PR" "$PS"; do nsenter -t "$p" -n sysctl -qw net.ipv4.tcp_ecn="$ecn"; done
  nsenter -t "$PR" -n tc qdisc add dev r1 root handle 1: htb default 1 r2q 1000
  nsenter -t "$PR" -n tc class add dev r1 parent 1: classid 1:1 htb rate 20mbit ceil 20mbit burst 3000
  nsenter -t "$PR" -n tc qdisc add dev r1 parent 1:1 handle 10: "$@" || { echo "  $label: tc refused: $*"; return; }
  nsenter -t "$PS" -n python3 "$here/tcpflow.py" server 10.9.1.2 9100 1 "$secs" >"$SCRATCH/server.json" &
  local sp=$!; sleep 0.5
  nsenter -t "$PC" -n python3 "$here/tcpflow.py" client 10.9.1.2 9100 "$((secs + 1))" cubic >"$SCRATCH/client.json" 2>&1
  wait "$sp"
  echo "=== $label  (tcp_ecn=$ecn, qdisc: $*)"
  python3 - "$SCRATCH/server.json" "$SCRATCH/client.json" <<'PY'
import json, sys
def last(path):
    lines = [l for l in open(path).read().strip().splitlines() if l.startswith("{")]
    return json.loads(lines[-1]) if lines else {}
server, client = last(sys.argv[1]), last(sys.argv[2])
rates = [x for r in server.get("bytes_per_second", []) for x in (r if isinstance(r, list) else [r])]
print("  goodput mbit/s:", round(sum(rates) * 8 / 1e6 / max(len(rates), 1), 1) if rates else None, " client info:", {k: client.get(k) for k in ("retrans", "total_retrans", "snd_cwnd", "rtt") if k in client})
PY
  nsenter -t "$PR" -n tc -s qdisc show dev r1 | tr '\n' ' ' | sed 's/  */ /g' | grep -o 'qdisc \(codel\|fq_codel\|pie\|netem\) .*' | cut -c1-420
  echo "  server side, packets with ECT(0) and with CE (IpExt):"
  nsenter -t "$PS" -n awk '/^IpExt:/ { if (!h) { for (i=1;i<=NF;i++) n[i]=$i; h=1 } else { for (i=2;i<=NF;i++) if (n[i]=="InCEPkts"||n[i]=="InECT0Pkts") printf "    %s %s\n", n[i], $i } }' /proc/net/netstat
  echo "  client side, TcpExt ECN counters:"
  nsenter -t "$PC" -n awk '/^TcpExt:/ { if (!h) { for (i=1;i<=NF;i++) n[i]=$i; h=1 } else { for (i=2;i<=NF;i++) if (n[i] ~ /ECN|CWR|Retrans$|TCPLostRetransmit/ && $i>0) printf "    %s %s\n", n[i], $i } }' /proc/net/netstat
}
SCRATCH="$(mktemp -d)"; trap 'cleanup; rm -rf "$SCRATCH"' EXIT
echo "# S24: kernel $(uname -r)"
run_case "codel with ecn, TCP uses ECN" 1 codel ecn
run_case "codel with ecn, TCP does not" 0 codel ecn
run_case "codel without ecn, TCP uses ECN" 1 codel noecn
run_case "fq_codel with ecn, TCP uses ECN" 1 fq_codel ecn
run_case "pie with ecn, TCP uses ECN" 1 pie ecn
run_case "pie without ecn, TCP uses ECN" 1 pie noecn
run_case "netem loss 1% with ecn, TCP uses ECN" 1 netem loss 1% ecn limit 1000
run_case "netem loss 1% without ecn, TCP uses ECN" 1 netem loss 1% limit 1000
