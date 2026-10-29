#!/usr/bin/env bash
# Runs all spikes and writes spikes/out/summary.md.
#   bash spikes/run-all.sh          as yourself (tests the unprivileged case)
#   sudo bash spikes/run-all.sh     as root (tests the root case)
# Nothing here changes the network of your machine: every check runs in namespaces that exist only while it runs.
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export SPIKE_OUT="${SPIKE_OUT:-$here/out}"
mkdir -p "$SPIKE_OUT"
: >"$SPIKE_OUT/results.txt"

for script in s1-userns s2-netem-seed s3-netem-features s4-delay-precision s5-change-rate; do
  echo
  echo "=== $script ==="
  bash "$here/$script.sh" || true
done

{
  echo "# Results of the spikes"
  echo
  echo "- date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "- kernel: $(uname -r)"
  echo "- user: $(id -un) (uid $(id -u))"
  echo "- tc: $(tc -V 2>&1 | head -n 1)"
  echo
  echo "| Result | Count |"
  echo "| --- | --- |"
  for kind in PASS FAIL WARN; do
    echo "| $kind | $(grep -c "^$kind " "$SPIKE_OUT/results.txt")"
  done
  echo
  echo '```'
  cat "$SPIKE_OUT/results.txt"
  echo '```'
} >"$SPIKE_OUT/summary.md"

echo
echo "Summary written to $SPIKE_OUT/summary.md"
echo "PASS: $(grep -c '^PASS ' "$SPIKE_OUT/results.txt")  FAIL: $(grep -c '^FAIL ' "$SPIKE_OUT/results.txt")"
