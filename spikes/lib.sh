# Shared helpers of the spikes. Sourced by the scripts, not run on its own.
# Every check writes one line to out/results.txt: PASS, FAIL, WARN or INFO, an id and a text.

SPIKE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SPIKE_OUT="${SPIKE_OUT:-$SPIKE_DIR/out}"
mkdir -p "$SPIKE_OUT"
RESULTS="$SPIKE_OUT/results.txt"

pass() { printf 'PASS %-7s %s\n' "$1" "$2" | tee -a "$RESULTS"; }
fail() { printf 'FAIL %-7s %s\n' "$1" "$2" | tee -a "$RESULTS"; }
warn() { printf 'WARN %-7s %s\n' "$1" "$2" | tee -a "$RESULTS"; }
info() { printf 'INFO %-7s %s\n' "${1:--}" "${2:-}" | tee -a "$RESULTS"; }

# check ID "text" command...   runs the command: PASS when it succeeds, FAIL (with its error) when not
check() {
  local id="$1" text="$2"
  shift 2
  local err
  if err="$("$@" 2>&1)"; then
    pass "$id" "$text"
    return 0
  fi
  fail "$id" "$text :: $(printf '%s' "$err" | head -n 3 | tr '\n' ' ')"
  return 1
}

# A second network namespace without `ip netns` (which needs a mount namespace): a process that
# holds an empty one. Sets NS_PID; the namespace lives as long as the process.
ns_start() {
  unshare -n sleep 3600 &
  NS_PID=$!
  # Give unshare a moment to create the namespace before anybody enters it
  local i
  for i in 1 2 3 4 5 6 7 8 9 10; do
    if nsenter -t "$NS_PID" -n true 2>/dev/null; then
      return 0
    fi
    sleep 0.1
  done
  return 1
}
ns_stop() {
  if [ -n "${NS_PID:-}" ]; then
    kill "$NS_PID" 2>/dev/null
    wait "$NS_PID" 2>/dev/null
  fi
  return 0
}
in_ns() { nsenter -t "$NS_PID" -n "$@"; }

# Two namespaces joined by a veth pair: this one (side A, v0, 10.9.0.1) and the second (side B,
# v1, 10.9.0.2). The checks run inside `unshare -Urn` (or `unshare -n` as root), so nothing here
# touches the network of the host.
pair_up() {
  ns_start || return 1
  # Every packet that enters a netem qdisc takes numbers from its random generator, so a packet that
  # nobody asked for (IPv6 duplicate address detection and router solicitations, ARP probes) shifts the
  # whole loss pattern (seen on 2026-10-29 in S2 on Ubuntu 22.04: the same seed gave the same losses
  # up to packet 168 and a pattern shifted by one after it). So IPv6 is switched off for the
  # interfaces *before* they exist, and the neighbour entries are fixed instead of learned.
  sysctl -qw net.ipv6.conf.default.disable_ipv6=1 2>/dev/null || true
  sysctl -qw net.ipv6.conf.all.disable_ipv6=1 2>/dev/null || true
  in_ns sysctl -qw net.ipv6.conf.default.disable_ipv6=1 2>/dev/null || true
  in_ns sysctl -qw net.ipv6.conf.all.disable_ipv6=1 2>/dev/null || true
  ip link add v0 type veth peer name v1 || return 1
  ip link set v1 netns "$NS_PID" || return 1
  ip addr add 10.9.0.1/24 dev v0 || return 1
  in_ns ip addr add 10.9.0.2/24 dev v1 || return 1
  local mac0 mac1
  # (not /sys/class/net: sysfs shows the network namespace it was mounted in, not the one we entered)
  mac0="$(ip -br link show dev v0 2>/dev/null | awk '{print $3}')"
  mac1="$(in_ns ip -br link show dev v1 2>/dev/null | awk '{print $3}')"
  if [ -n "$mac0" ] && [ -n "$mac1" ]; then
    ip neigh replace 10.9.0.2 lladdr "$mac1" dev v0 nud permanent || true
    in_ns ip neigh replace 10.9.0.1 lladdr "$mac0" dev v1 nud permanent || true
  fi
  ip link set v0 up || return 1
  ip link set lo up || true
  in_ns ip link set v1 up || return 1
  in_ns ip link set lo up || true
  # A short check that the path works (this also warms up the interfaces)
  ping -c 2 -W 1 10.9.0.2 >/dev/null 2>&1 || return 1
  return 0
}
