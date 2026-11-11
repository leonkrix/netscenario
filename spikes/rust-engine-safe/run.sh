#!/usr/bin/env bash
# S21: a packet engine in (nearly) safe Rust: the rate it keeps up with, the lateness of its releases for five ways to wait, one
# thread or one per link, a jitter that reorders, the same seed twice, and a TCP flow through it.
#
#   client_k namespace --veth-- ENGINE (here) --veth-- server_k namespace        for k = 1..N
#
# Everything happens in namespaces that this script creates (an unprivileged user namespace, or root without touching the host).
#   bash spikes/rust-engine-safe/run.sh                 everything (about 12 minutes)
#   SECTION=timers|links|jitter|loss|tcp bash ...        one part
#   QUICK=1 bash ...                                     short runs
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$PATH:/usr/sbin:/sbin"

if [ "${1:-outer}" = outer ]; then
  cargo build --release --quiet --manifest-path "$here/Cargo.toml" || exit 1
  cargo build --release --quiet --manifest-path "$here/../rust-engine/Cargo.toml" || exit 1
  if [ "$(id -u)" -eq 0 ]; then exec unshare -n bash "$0" inner; fi
  exec unshare -Urn bash "$0" inner
fi

engine="$here/target/release/spike-engine-safe"
tool="$here/../rust-engine/target/release/spike-engine"   # the sender and the receiver of S8
secs="${SECS:-4}"; [ -n "${QUICK:-}" ] && secs=2
section="${SECTION:-all}"
# One physical core each for the engine, the receiver and the sender (a Ryzen 7 5800X has 8; the runners have 4: set them)
CE="${CORE_ENGINE:-2}"; CR="${CORE_RECV:-3}"; CS="${CORE_SEND:-4}"
cpus="$(nproc)"
want() { [ "$section" = all ] || [ "$section" = "$1" ]; }

pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done; wait 2>/dev/null; }
trap cleanup EXIT
ns_new() {  # VARIABLE: a namespace held by a sleeping process
  unshare -n sleep 3600 >/dev/null 2>&1 &
  pids+=($!)
  printf -v "$1" '%s' "$!"
  for _ in $(seq 50); do nsenter -t "$!" -n true 2>/dev/null && return 0; sleep 0.1; done
  return 1
}
nsmac() { nsenter -t "$1" -n ip -br link show dev "$2" | awk '{print $3}'; }

sysctl -qw net.ipv6.conf.default.disable_ipv6=1 net.ipv6.conf.all.disable_ipv6=1
declare -A PC PS
for k in 1 2 3 4; do ns_new "PC$k"; ns_new "PS$k"; done
for k in 1 2 3 4; do
  for v in PC$k PS$k; do nsenter -t "${!v}" -n sysctl -qw net.ipv6.conf.default.disable_ipv6=1; done
done

# topology MODE N: engine = client_k -- ec_k [engine] es_k -- server_k ; direct = client_k -- server_k
topology() {
  local mode="$1" n="$2" k
  for k in 1 2 3 4; do
    ip link del "ec$k" 2>/dev/null; ip link del "es$k" 2>/dev/null
    nsenter -t "$(eval echo \$PC$k)" -n ip link del "c$k" 2>/dev/null
    nsenter -t "$(eval echo \$PS$k)" -n ip link del "s$k" 2>/dev/null
  done
  for k in $(seq "$n"); do
    local pc pcs; pc="$(eval echo \$PC$k)"; pcs="$(eval echo \$PS$k)"
    if [ "$mode" = engine ]; then
      ip link add "ec$k" type veth peer name "c$k" && ip link set "c$k" netns "$pc"
      ip link add "es$k" type veth peer name "s$k" && ip link set "s$k" netns "$pcs"
      ip link set "ec$k" up; ip link set "es$k" up
    else
      ip link add "c$k" type veth peer name "s$k" && ip link set "c$k" netns "$pc" && ip link set "s$k" netns "$pcs"
    fi
    nsenter -t "$pc" -n ip addr add "10.9.$k.1/24" dev "c$k"
    nsenter -t "$pcs" -n ip addr add "10.9.$k.2/24" dev "s$k"
    nsenter -t "$pc" -n ip neigh replace "10.9.$k.2" lladdr "$(nsmac "$pcs" "s$k")" dev "c$k" nud permanent
    nsenter -t "$pcs" -n ip neigh replace "10.9.$k.1" lladdr "$(nsmac "$pc" "c$k")" dev "s$k" nud permanent
    nsenter -t "$pc" -n ip link set "c$k" up
    nsenter -t "$pcs" -n ip link set "s$k" up
  done
}
offload_off() {  # N: the node ends of N links
  local k
  for k in $(seq "$1"); do
    nsenter -t "$(eval echo \$PC$k)" -n python3 "$here/ethtool_off.py" "c$k" >/dev/null
    nsenter -t "$(eval echo \$PS$k)" -n python3 "$here/ethtool_off.py" "s$k" >/dev/null
  done
}

SCRATCH="$(mktemp -d)"; trap 'cleanup; rm -rf "$SCRATCH"' EXIT

# udp_case LABEL N THREADS TIMER PPS_PER_LINK DELAY_US JITTER_US LOSS SEED
udp_case() {
  local label="$1" n="$2" threads="$3" timer="$4" pps="$5" delay="$6" jitter="$7" loss="$8" seed="$9"
  local links="" k epid=""
  for k in $(seq "$n"); do links="$links,ec$k:es$k"; done; links="${links#,}"
  if [ "$timer" != direct ]; then
    local cores="$CE"; [ "$threads" -gt 1 ] && cores="$CE-$((CE + threads - 1))"
    taskset -c "$cores" "$engine" --links "$links" --delay-us "$delay" --jitter-us "$jitter" --loss "$loss" --seed "$seed" \
      --timer "$timer" --threads "$threads" >"$SCRATCH/engine.json" &
    epid=$!; sleep 0.4
  fi
  local rp=() sp=()
  for k in $(seq "$n"); do
    nsenter -t "$(eval echo \$PS$k)" -n taskset -c "$((CR + 2 * (k - 1)))" "$tool" recv "10.9.$k.2" 9000 "$((secs + 10))" >"$SCRATCH/recv$k.json" &
    rp+=($!)
  done
  sleep 0.3
  for k in $(seq "$n"); do
    nsenter -t "$(eval echo \$PC$k)" -n taskset -c "$((CS + 2 * (k - 1)))" "$tool" send "10.9.$k.2" 9000 "$pps" "$secs" 200 >"$SCRATCH/send$k.json" &
    sp+=($!)
  done
  wait "${sp[@]}"; wait "${rp[@]}"
  [ -n "$epid" ] && { kill "$epid"; wait "$epid" 2>/dev/null; }
  echo "=== $label  (links=$n threads=$threads timer=$timer pps/link=$pps delay=${delay}us jitter=${jitter}us loss=${loss}% seed=$seed)"
  for k in $(seq "$n"); do
    python3 - "$SCRATCH/send$k.json" "$SCRATCH/recv$k.json" "$k" <<'PY'
import json, sys
s = json.load(open(sys.argv[1])); r = json.load(open(sys.argv[2]))
keep = ["received", "lost", "loss_percent", "lost_hash", "mean_ms", "stdev_ms", "min_ms", "p50_ms", "p99_ms", "p999_ms", "max_ms"]
print(f"  link {sys.argv[3]}: sent {s.get('sent')} at {s.get('achieved_pps')} pps; " + ", ".join(f"{k}={r[k]}" for k in keep if k in r))
PY
  done
  [ -n "$epid" ] && cat "$SCRATCH/engine.json"
}

echo "# S21: kernel $(uname -r), $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs), $(nproc) threads"
if want timers; then
  topology direct 1
  udp_case "baseline: no engine, direct veth" 1 1 direct 100000 0 0 0 1
  topology engine 1
  for timer in epoll_ms epoll_ns timerfd spin spinsleep; do
    for pps in 1000 10000 50000 100000; do
      udp_case "delay 20 ms" 1 1 "$timer" "$pps" 20000 0 0 1
    done
  done
fi
if want links && [ "$cpus" -lt 8 ]; then
  echo "INFO links    skipped: $cpus CPUs, the section of several links needs 8"
elif want links; then
  topology engine 4
  for timer in epoll_ns spinsleep; do
    udp_case "2 links, one thread" 2 1 "$timer" 50000 20000 0 0 1
    udp_case "2 links, one thread each" 2 2 "$timer" 50000 20000 0 0 1
    udp_case "4 links, one thread" 4 1 "$timer" 25000 20000 0 0 1
    udp_case "4 links, one thread each" 4 4 "$timer" 25000 20000 0 0 1
  done
fi
if want jitter; then
  topology engine 1
  udp_case "uniform jitter of 5 ms around 20 ms (a sorted queue; gaps of 20 us at 50000 pps)" 1 1 epoll_ns 50000 20000 5000 0 1
fi
if want loss; then
  topology engine 1
  udp_case "loss 10 percent, seed 7, run 1" 1 1 epoll_ns 10000 20000 0 10 7
  udp_case "loss 10 percent, seed 7, run 2" 1 1 epoll_ns 10000 20000 0 10 7
  udp_case "loss 10 percent, seed 8" 1 1 epoll_ns 10000 20000 0 10 8
fi
if want compare; then
  # S23: the same link on the kernel (netem on the client's interface) and on the engine, three runs each, to see how far two
  # backends are from each other and from themselves
  kernel_case() {  # LABEL PPS NETEM_ARGS...
    local label="$1" pps="$2"; shift 2
    topology direct 1
    nsenter -t "$PC1" -n tc qdisc add dev c1 root netem "$@" || return 1
    nsenter -t "$PS1" -n taskset -c "$CR" "$tool" recv 10.9.1.2 9000 "$((secs + 10))" >"$SCRATCH/recv1.json" &
    local rp=$!; sleep 0.3
    nsenter -t "$PC1" -n taskset -c "$CS" "$tool" send 10.9.1.2 9000 "$pps" "$secs" 200 >"$SCRATCH/send1.json"
    wait "$rp"
    echo "=== $label  (links=1 threads=1 timer=kernel-netem pps/link=$pps)"
    python3 - "$SCRATCH/send1.json" "$SCRATCH/recv1.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1])); r = json.load(open(sys.argv[2]))
keep = ["received", "lost", "loss_percent", "mean_ms", "stdev_ms", "min_ms", "p50_ms", "p99_ms", "p999_ms", "max_ms"]
print(f"  link 1: sent {s.get('sent')} at {s.get('achieved_pps')} pps; " + ", ".join(f"{k}={r[k]}" for k in keep if k in r))
PY
    echo '{"role":"engine","timer":"kernel","threads":0,"forwarded":0,"dropped":0,"send_errors":0,"reordered":0,"max_queue":0,"wakeups":0,"cpu_seconds":0,"wall_seconds":0,"late_mean_us":0,"late_p50_us":0,"late_p99_us":0,"late_p999_us":0,"late_max_us":0,"late_over_20ms":0}'
  }
  for rep in 1 2 3; do
    topology engine 1
    udp_case "A delay 20ms  rep $rep" 1 1 epoll_ns 10000 20000 0 0 $rep
    udp_case "A delay 20ms  rep $rep" 1 1 spinsleep 10000 20000 0 0 $rep
    kernel_case "A delay 20ms  rep $rep" 10000 delay 20ms
    topology engine 1
    udp_case "B delay 20ms jitter 5ms  rep $rep" 1 1 epoll_ns 10000 20000 5000 0 $rep
    udp_case "B delay 20ms jitter 5ms  rep $rep" 1 1 spinsleep 10000 20000 5000 0 $rep
    kernel_case "B delay 20ms jitter 5ms  rep $rep" 10000 delay 20ms 5ms
    topology engine 1
    udp_case "C loss 10pct  rep $rep" 1 1 epoll_ns 10000 0 0 10 $rep
    udp_case "C loss 10pct  rep $rep" 1 1 spinsleep 10000 0 0 10 $rep
    kernel_case "C loss 10pct  rep $rep" 10000 loss 10%
  done
fi
if want tcp; then
  # tcp_case LABEL MODE OFFLOAD DELAY_US: MODE direct|engine; OFFLOAD on (left alone), off (transmit checksum offload switched off at
  # the node ends), segs (one segment per frame, as the backend does with D-086) or vnet (segs, and the engine keeps the
  # unfinished checksum with PACKET_VNET_HDR)
  tcp_case() {
    local label="$1" mode="$2" off="$3" delay="$4" epid="" extra=""
    topology "$mode" 1
    [ "$off" = off ] && offload_off 1
    if [ "$off" = segs ] || [ "$off" = vnet ]; then
      nsenter -t "$PC1" -n ip link set c1 gso_max_size 1514 gso_max_segs 1
      nsenter -t "$PS1" -n ip link set s1 gso_max_size 1514 gso_max_segs 1
    fi
    [ "$off" = vnet ] && extra="--vnet-hdr"
    if [ "$mode" = engine ]; then
      taskset -c "$CE" "$engine" --links ec1:es1 --delay-us "$delay" --timer "${TCP_TIMER:-spinsleep}" $extra >"$SCRATCH/engine.json" &
      epid=$!; sleep 0.4
    fi
    : >"$SCRATCH/tcp-server.json"; : >"$SCRATCH/tcp-client.json"
    nsenter -t "$PS1" -n taskset -c "$CR" timeout $((secs + 8)) python3 "$here/../tcpflow.py" server 10.9.1.2 9100 1 "$secs" >"$SCRATCH/tcp-server.json" 2>&1 &
    local sp=$!; sleep 0.5
    nsenter -t "$PC1" -n taskset -c "$CS" timeout $((secs + 8)) python3 "$here/../tcpflow.py" client 10.9.1.2 9100 "$((secs + 1))" cubic >"$SCRATCH/tcp-client.json" 2>&1
    wait "$sp"
    [ -n "$epid" ] && { kill "$epid"; wait "$epid" 2>/dev/null; }
    echo "=== TCP: $label  (mode=$mode offloads=$off delay=${delay}us each way)"
    python3 - "$SCRATCH/tcp-server.json" "$SCRATCH/tcp-client.json" <<'PY'
import json, sys
def last(path):
    lines = [l for l in open(path).read().strip().splitlines() if l.startswith("{")]
    return json.loads(lines[-1]) if lines else None
server, client = last(sys.argv[1]), last(sys.argv[2])
if server:
    rates = server.get("bytes_per_second") or []
    flat = [x for r in rates for x in (r if isinstance(r, list) else [r])]
    print("  server: goodput of the connection", [round(x * 8 / 1e6, 1) for x in flat[:1]], "mbit/s over its seconds:", [round(x * 8 / 1e6) for x in flat[:6]])
else:
    print("  server: no result (no connection arrived)")
print("  client:", "no result (the connection did not work)" if not client else json.dumps({k: client.get(k) for k in ("retransmits", "retrans", "snd_cwnd", "rtt")}))
PY
    [ -n "$epid" ] && cat "$SCRATCH/engine.json"
  }
  tcp_case "direct veth, offloads as they are" direct on 0
  tcp_case "direct veth, one segment per frame (D-086)" direct segs 0
  tcp_case "engine, offloads left on" engine on 0
  tcp_case "engine, transmit checksum offload off (ioctl)" engine off 0
  tcp_case "engine, PACKET_VNET_HDR, one segment per frame" engine vnet 0
  tcp_case "engine, PACKET_VNET_HDR, 10 ms each way" engine vnet 10000
fi
