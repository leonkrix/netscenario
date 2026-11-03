# A lab of several network namespaces for the spikes of release 0.3. Sourced after lib.sh, inside a namespace made by
# enter_ns (the "bench"): every node is a namespace held by a sleeping process, and the bench only creates things.
#
#   node_new NAME [v6]        a node (IPv6 is off in it unless "v6" is given; with v6 DAD, RA and autoconf are off)
#   node NAME cmd...          run a command in the node
#   link_new N A B ADDR_A ADDR_B   a veth pair l<N>a (in A) and l<N>b (in B) with the addresses (CIDR) and fixed neighbours
#   lab_stop                  end all the holders
declare -A NODE_PID=()
LAB_PIDS=()

node_new() {
  local name="$1" v6="${2:-}" pid i
  unshare -n sleep 3600 >/dev/null 2>&1 &
  pid=$!
  for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do
    nsenter -t "$pid" -n true 2>/dev/null && break
    sleep 0.05
  done
  NODE_PID[$name]=$pid
  LAB_PIDS+=("$pid")
  if [ "$v6" = v6 ]; then
    nsenter -t "$pid" -n sh -c 'sysctl -qw net.ipv6.conf.default.disable_ipv6=0 net.ipv6.conf.default.accept_dad=0 net.ipv6.conf.default.accept_ra=0 net.ipv6.conf.default.autoconf=0' 2>/dev/null
  else
    nsenter -t "$pid" -n sh -c 'sysctl -qw net.ipv6.conf.default.disable_ipv6=1 net.ipv6.conf.all.disable_ipv6=1' 2>/dev/null
  fi
  nsenter -t "$pid" -n ip link set lo up
}
node() { local name="$1"; shift; nsenter -t "${NODE_PID[$name]}" -n "$@"; }
node_forward() { # NAME: forwarding on, both families
  node "$1" sysctl -qw net.ipv4.ip_forward=1 net.ipv6.conf.all.forwarding=1 2>/dev/null || node "$1" sysctl -qw net.ipv4.ip_forward=1
}
mac_of() { node "$1" cat "/sys/class/net/$2/address" 2>/dev/null || node "$1" ip -br link show dev "$2" | awk '{print $3}'; }
link_new() {
  local n="$1" a="$2" b="$3" ipa="$4" ipb="$5" ifa="l${1}a" ifb="l${1}b" ma mb
  ip link add "$ifa" type veth peer name "$ifb" || return 1
  ip link set "$ifa" netns "${NODE_PID[$a]}" || return 1
  ip link set "$ifb" netns "${NODE_PID[$b]}" || return 1
  node "$a" ip addr add "$ipa" dev "$ifa" ${LINK_FLAGS:-} || return 1
  node "$b" ip addr add "$ipb" dev "$ifb" ${LINK_FLAGS:-} || return 1
  ma="$(node "$a" ip -br link show dev "$ifa" | awk '{print $3}')"
  mb="$(node "$b" ip -br link show dev "$ifb" | awk '{print $3}')"
  node "$a" ip neigh replace "${ipb%/*}" lladdr "$mb" dev "$ifa" nud permanent
  node "$b" ip neigh replace "${ipa%/*}" lladdr "$ma" dev "$ifb" nud permanent
  node "$a" ip link set "$ifa" up
  node "$b" ip link set "$ifb" up
}
lab_stop() {
  local pid
  for pid in "${LAB_PIDS[@]:-}"; do [ -n "$pid" ] && kill "$pid" 2>/dev/null; done
  wait 2>/dev/null
  LAB_PIDS=()
  NODE_PID=()
}
now_ms() { echo $(( $(date +%s%N) / 1000000 )); }
# rx_tx NODE IF   prints "rx_packets tx_packets" of the interface
rx_tx() { node "$1" cat /proc/net/dev | awk -v i="$2:" '$1==i {print $3, $11}'; }
