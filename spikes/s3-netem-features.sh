#!/usr/bin/env bash
# S3: which impairments and queue disciplines can be configured here?
set -u
. "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

if [ "${1:-outer}" = outer ]; then
  if [ "$(id -u)" -eq 0 ]; then exec unshare -n bash "$0" inner; fi
  exec unshare -Urn bash "$0" inner
fi

if ! ip link add v0 type veth peer name v1; then
  fail S3.0 "no veth pair, nothing to test"
  exit 0
fi
ip link set v0 up

# try ID "text" qdisc-arguments...   on a clean root qdisc
try() {
  local id="$1" text="$2"
  shift 2
  tc qdisc del dev v0 root 2>/dev/null || true
  check "$id" "$text" tc qdisc add dev v0 root "$@"
}

try S3.01 "netem delay + jitter (uniform)"        netem delay 40ms 5ms
try S3.02 "netem delay + jitter (normal table)"   netem delay 40ms 10ms distribution normal
try S3.03 "netem delay + jitter (pareto table)"   netem delay 40ms 10ms distribution pareto
try S3.04 "netem loss random"                     netem loss 2%
try S3.05 "netem loss gemodel (Gilbert-Elliott)"  netem loss gemodel 2% 30% 50% 0.1%
try S3.06 "netem loss state (4-state Markov)"     netem loss state 2% 30% 10% 20% 0.5%
try S3.07 "netem reorder"                         netem delay 10ms reorder 25% 50%
try S3.08 "netem duplicate"                       netem duplicate 1%
try S3.09 "netem corrupt"                         netem corrupt 0.1%
try S3.10 "netem rate"                            netem rate 10mbit
try S3.11 "netem limit (queue length)"            netem delay 10ms limit 500
try S3.12 "netem loss with ecn"                   netem loss 2% ecn
try S3.13 "tbf (token bucket)"                    tbf rate 10mbit burst 32kbit latency 50ms
try S3.14 "htb"                                   htb default 1
try S3.15 "fq_codel"                              fq_codel
try S3.16 "codel"                                 codel
try S3.17 "pie"                                   pie
try S3.18 "cake"                                  cake bandwidth 10mbit
try S3.19 "fq"                                    fq

# netem below a token bucket: the usual way to combine a delay and a rate limit
tc qdisc del dev v0 root 2>/dev/null || true
if tc qdisc add dev v0 root handle 1: tbf rate 10mbit burst 32kbit latency 50ms &&
  tc qdisc add dev v0 parent 1:1 handle 10: netem delay 20ms loss 1%; then
  pass S3.20 "tbf with netem as its child"
else
  fail S3.20 "tbf with netem as its child"
fi

# Changing the parameters of a qdisc that is running
tc qdisc del dev v0 root 2>/dev/null || true
tc qdisc add dev v0 root netem delay 20ms
check S3.21 "change a running netem" tc qdisc change dev v0 root netem delay 80ms

# The receiving side (to shape what arrives) with an ifb device
check S3.22 "ifb device (for shaping the receiving side)" ip link add ifb0 type ifb
check S3.23 "ingress qdisc" tc qdisc add dev v0 handle ffff: ingress
