#!/usr/bin/env bash
# S1: what can an unprivileged user do with namespaces, virtual links and netem on this machine?
# Everything happens inside namespaces that are created for this run; the network of the host is not touched.
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [ "${1:-outer}" = outer ]; then
  info S1.0 "kernel: $(uname -r)"
  info S1.0 "os: $(. /etc/os-release 2>/dev/null && echo "${PRETTY_NAME:-unknown}")"
  info S1.0 "user: $(id -un) (uid $(id -u))"
  for tool in ip tc unshare nsenter ping python3 sysctl; do
    if command -v "$tool" >/dev/null 2>&1; then pass S1.t "tool: $tool"; else fail S1.t "tool missing: $tool"; fi
  done
  info S1.0 "tc: $(tc -V 2>&1 | head -n 1)"
  for key in kernel.unprivileged_userns_clone kernel.apparmor_restrict_unprivileged_userns user.max_user_namespaces; do
    info S1.0 "sysctl $key = $(sysctl -n "$key" 2>/dev/null || echo 'not present')"
  done
  if [ "$(id -u)" -eq 0 ]; then
    info S1.0 "running as root: using plain network namespaces (the rootless case is not tested)"
    enter_ns S1.1 "$0"
  fi
  if ! check S1.1 "unshare --user --map-root-user --net" unshare -Urn true; then
    info S1.0 "No unprivileged user namespaces here. Run the spikes with sudo as well, and see spikes/README.md."
    exit 0
  fi
  exec unshare -Urn bash "$0" inner
fi

# --- inside the new network namespace (root here; with -r nobody outside) ---
check S1.2 "create a veth pair" ip link add v0 type veth peer name v1
if ns_start; then pass S1.3 "start a second namespace held by a process"; else fail S1.3 "start a second namespace held by a process"; fi
if [ -n "${NS_PID:-}" ]; then
  check S1.4 "move the veth end into it" ip link set v1 netns "$NS_PID"
  check S1.5 "enter it with nsenter" in_ns ip link show
  ns_stop
fi
ip link del v0 2>/dev/null || true

if pair_up; then
  pass S1.6 "two namespaces, a veth pair, addresses, and ping works"
  check S1.7 "netem on the veth end: delay" tc qdisc replace dev v0 root netem delay 20ms
  check S1.8 "netem: loss" tc qdisc replace dev v0 root netem loss 10%
  check S1.9 "tc qdisc show works" tc qdisc show dev v0
  tc qdisc replace dev v0 root netem delay 20ms 2>/dev/null
  rtt="$(ping -c 5 -i 0.2 -W 2 10.9.0.2 2>&1 | tail -n 1)"
  info S1.A "ping with 20 ms of netem on one direction: $rtt"
else
  fail S1.6 "two namespaces with a working veth pair"
fi
ns_stop
