#!/usr/bin/env bash
# S22: netlink by a thin layer of our own over `netlink-packet-route` (no `unsafe`) and `netlink-sys`: the messages for netem, the
# fifos, codel, pie, fq_codel, tbf and htb are encoded here, applied in a user namespace, and the kernel's dump is compared with
# what the `tc` command makes of the same settings. Also the time of a change of a running netem.
#   bash spikes/rust-netlink-own/run.sh
set -u
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$PATH:/usr/sbin:/sbin"
cargo build --release --quiet --manifest-path "$here/Cargo.toml" || exit 1
if [ "$(id -u)" -eq 0 ]; then exec unshare -n "$here/target/release/spike-netlink-own"; fi
exec unshare -Urn "$here/target/release/spike-netlink-own"
