#!/usr/bin/env bash
# S6 (optional): what works inside a Docker container? Runs the other spikes in an Ubuntu container,
# three times: with NET_ADMIN only, with Docker's default restrictions removed, and without any extra right.
# Needs Docker and a network connection (to install iproute2 in the container).
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"

if ! command -v docker >/dev/null 2>&1; then
  echo "Docker is not installed; nothing to do."
  exit 0
fi

run() {
  local label="$1"
  shift
  echo
  echo "=== container: $label ==="
  docker run --rm "$@" -v "$root":/work -w /work -e SPIKE_OUT=/tmp/out ubuntu:24.04 bash -c '
    apt-get update -qq >/dev/null &&
    apt-get install -y -qq --no-install-recommends iproute2 iputils-ping python3 procps util-linux >/dev/null &&
    bash spikes/run-all.sh | tail -n 60'
}

run "no extra rights"
run "NET_ADMIN" --cap-add NET_ADMIN
run "NET_ADMIN and SYS_ADMIN, no seccomp profile" --cap-add NET_ADMIN --cap-add SYS_ADMIN --security-opt seccomp=unconfined
