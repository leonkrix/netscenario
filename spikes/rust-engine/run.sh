#!/usr/bin/env bash
# S8: can a userspace packet engine forward at a useful rate inside an unprivileged user namespace?
#
#   client namespace --veth-- ENGINE namespace --veth-- server namespace
#
# The engine holds the two inner ends of the veth pairs, has no addresses, and forwards frames with
# AF_PACKET sockets: fixed delay, seeded random loss. The client sends UDP at a given rate, the server
# measures loss and the one-way delay. Everything happens in namespaces created by this script, so the
# network of the host is never touched.
#   bash spikes/rust-engine/run.sh            full series (about 2 minutes)
#   QUICK=1 bash spikes/rust-engine/run.sh    a short series
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$PATH:/usr/sbin:/sbin"

if [ "${1:-outer}" = outer ]; then
  cargo build --release --quiet --manifest-path "$here/Cargo.toml" || exit 1
  if [ "$(id -u)" -eq 0 ]; then exec unshare -n bash "$0" inner; fi
  exec unshare -Urn bash "$0" inner
fi

bin="$here/target/release/spike-engine"
secs="${SECS:-5}"
# One physical core each for the engine, the receiver and the sender (a Ryzen 7 5800X has 8; adjust for another CPU)
CORE_ENGINE="${CORE_ENGINE:-2}" CORE_RECV="${CORE_RECV:-3}" CORE_SEND="${CORE_SEND:-4}"

pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done; wait 2>/dev/null; }
trap cleanup EXIT

# ns_new VARIABLE: starts a namespace held by a sleeping process and stores its pid (not through $(...),
# which would wait for the sleeping process to close the pipe)
ns_new() {
  unshare -n sleep 3600 >/dev/null 2>&1 &
  pids+=($!)
  printf -v "$1" '%s' "$!"
  for _ in $(seq 50); do nsenter -t "$!" -n true 2>/dev/null && return 0; sleep 0.1; done
  return 1
}
mac() { ip -br link show dev "$1" | awk '{print $3}'; }
nsmac() { nsenter -t "$1" -n ip -br link show dev "$2" | awk '{print $3}'; }

sysctl -qw net.ipv6.conf.default.disable_ipv6=1 net.ipv6.conf.all.disable_ipv6=1
ns_new PC; ns_new PS
nsenter -t "$PC" -n sysctl -qw net.ipv6.conf.default.disable_ipv6=1
nsenter -t "$PS" -n sysctl -qw net.ipv6.conf.default.disable_ipv6=1

# mode "engine": c0 -- c1 [engine] s1 -- s0 ;  mode "direct": c0 -- s0 (no engine, for the baseline)
topology() {
  ip link del c1 2>/dev/null; ip link del s1 2>/dev/null
  nsenter -t "$PC" -n ip link del c0 2>/dev/null; nsenter -t "$PS" -n ip link del s0 2>/dev/null
  if [ "$1" = engine ]; then
    ip link add c1 type veth peer name c0 && ip link set c0 netns "$PC"
    ip link add s1 type veth peer name s0 && ip link set s0 netns "$PS"
    ip link set c1 up; ip link set s1 up
  else
    ip link add c0 type veth peer name s0 && ip link set c0 netns "$PC" && ip link set s0 netns "$PS"
  fi
  nsenter -t "$PC" -n ip addr add 10.9.0.1/24 dev c0
  nsenter -t "$PS" -n ip addr add 10.9.0.2/24 dev s0
  nsenter -t "$PC" -n ip neigh replace 10.9.0.2 lladdr "$(nsmac "$PS" s0)" dev c0 nud permanent
  nsenter -t "$PS" -n ip neigh replace 10.9.0.1 lladdr "$(nsmac "$PC" c0)" dev s0 nud permanent
  nsenter -t "$PC" -n ip link set c0 up
  nsenter -t "$PS" -n ip link set s0 up
}

# case LABEL TIMER PPS DELAY_US LOSS_PERCENT SEED PAYLOAD
case_run() {
  local label="$1" timer="$2" pps="$3" delay="$4" loss="$5" seed="$6" payload="${7:-200}"
  local engine_pid="" out
  if [ "$timer" != direct ]; then
    taskset -c "$CORE_ENGINE" "$bin" engine c1 s1 "$delay" "$loss" "$seed" "$timer" >"$SCRATCH/engine.json" &
    engine_pid=$!
    sleep 0.3
  fi
  nsenter -t "$PS" -n taskset -c "$CORE_RECV" "$bin" recv 10.9.0.2 9000 "$((secs + 10))" >"$SCRATCH/recv.json" &
  local recv_pid=$!
  sleep 0.3
  nsenter -t "$PC" -n taskset -c "$CORE_SEND" "$bin" send 10.9.0.2 9000 "$pps" "$secs" "$payload" >"$SCRATCH/send.json"
  wait "$recv_pid"
  [ -n "$engine_pid" ] && { kill "$engine_pid"; wait "$engine_pid" 2>/dev/null; }
  echo "=== $label  (timer=$timer pps=$pps delay=${delay}us loss=${loss}% seed=$seed payload=${payload}B)"
  cat "$SCRATCH/send.json" "$SCRATCH/recv.json"
  [ -n "$engine_pid" ] && cat "$SCRATCH/engine.json"
}

SCRATCH="$(mktemp -d)"
trap 'cleanup; rm -rf "$SCRATCH"' EXIT

echo "# S8: kernel $(uname -r), $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs)"
topology direct
case_run "baseline: no engine, direct veth" direct 100000 0 0 1
topology engine
if [ -n "${CHECKSUM_TEST:-}" ]; then
  # With the UDP checksum left to the sender (the veth pair offloads it), does the engine's output arrive?
  export SPIKE_KEEP_CHECKSUM=1
  case_run "checksum kept by the sender, engine 20 ms" epoll_ns 10000 20000 0 1
  exit 0
fi
if [ -n "${QUICK:-}" ]; then
  case_run "engine, 20 ms, epoll_ns" epoll_ns 10000 20000 0 1
  exit 0
fi
for timer in epoll_ms epoll_ns spin; do
  for pps in 1000 10000 50000 100000; do
    case_run "delay 20 ms" "$timer" "$pps" 20000 0 1
  done
done
case_run "engine, no delay, epoll_ns" epoll_ns 100000 0 0 1
case_run "engine, no delay, epoll_ns, 150000 pps" epoll_ns 150000 0 0 1
case_run "loss 10 percent, seed 7 (run 1)" epoll_ns 10000 20000 10 7
case_run "loss 10 percent, seed 7 (run 2)" epoll_ns 10000 20000 10 7
case_run "loss 10 percent, seed 8" epoll_ns 10000 20000 10 8
