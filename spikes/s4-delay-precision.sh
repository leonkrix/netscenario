#!/usr/bin/env bash
# S4: how exact is a measurement of one-way delay through netem on one machine?
# A UDP stream goes from this namespace to a second one over a veth pair; both share one clock.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [ "${1:-outer}" = outer ]; then
  if ! command -v python3 >/dev/null 2>&1; then fail S4.0 "python3 is needed for this spike"; exit 0; fi
  enter_ns S4.0 "$0"
fi

count=2000
pps=1000

if ! pair_up; then
  fail S4.0 "could not set up the pair of namespaces (run S1 first)"
  ns_stop
  exit 0
fi

# measure ID "text" netem-arguments...
measure() {
  local id="$1" text="$2"
  shift 2
  tc qdisc del dev v0 root 2>/dev/null || true
  if [ "$#" -gt 0 ]; then
    if ! tc qdisc add dev v0 root netem "$@"; then
      fail "$id" "$text :: netem refused the settings"
      return
    fi
  fi
  in_ns python3 "$SPIKE_DIR/udp_delay.py" server 10.9.0.2 9000 "$count" >"$SPIKE_OUT/server.$id.txt" &
  local server=$!
  # Wait until the server says that it is ready
  local i
  for i in $(seq 1 50); do
    grep -q ready "$SPIKE_OUT/server.$id.txt" 2>/dev/null && break
    sleep 0.1
  done
  python3 "$SPIKE_DIR/udp_delay.py" client 10.9.0.2 9000 "$count" "$pps"
  wait "$server"
  info "$id" "$text :: $(tail -n 1 "$SPIKE_OUT/server.$id.txt")"
}

measure S4.1 "no netem (the overhead of the path itself)"
measure S4.2 "delay 20ms" delay 20ms
measure S4.3 "delay 20ms 5ms distribution normal" delay 20ms 5ms distribution normal
measure S4.4 "delay 20ms, rate 2mbit" delay 20ms rate 2mbit
ns_stop
info S4.9 "Read: mean_ms of S4.2 should be 20 plus the overhead of S4.1; stdev_ms is the measurement noise; p99_ms - p50_ms the tail."
