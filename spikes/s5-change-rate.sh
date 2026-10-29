#!/usr/bin/env bash
# S5: how fast can the parameters of a running netem be changed? This limits trace-driven links on the kernel backend.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [ "${1:-outer}" = outer ]; then
  enter_ns S5.0 "$0"
fi

if ! ip link add v0 type veth peer name v1; then
  fail S5.0 "no veth pair, nothing to test"
  exit 0
fi
ip link set v0 up
tc qdisc add dev v0 root netem delay 10ms || { fail S5.0 "no netem"; exit 0; }

n=300

# One process per change (what a naive backend does)
start="$(date +%s%N)"
for i in $(seq 1 "$n"); do
  tc qdisc change dev v0 root netem delay "$((10 + i % 50))ms" loss "$((i % 5))%"
done
end="$(date +%s%N)"
per_ms="$(((end - start) / n / 1000))"
info S5.1 "one tc process per change: ${per_ms} microseconds per change ($n changes)"

# One process for many changes (what a backend that batches, or one with netlink, does)
batch="$SPIKE_OUT/batch.tc"
: >"$batch"
for i in $(seq 1 "$n"); do
  echo "qdisc change dev v0 root netem delay $((10 + i % 50))ms loss $((i % 5))%" >>"$batch"
done
start="$(date +%s%N)"
if tc -batch "$batch"; then
  end="$(date +%s%N)"
  info S5.2 "tc -batch: $(((end - start) / n / 1000)) microseconds per change ($n changes)"
  pass S5.3 "changes can be batched"
else
  fail S5.3 "tc -batch failed"
fi
info S5.9 "Read: if a change takes about 1 ms, a trace can change a link about every 10 ms without drifting; much faster than that needs a better backend (the engine)."
