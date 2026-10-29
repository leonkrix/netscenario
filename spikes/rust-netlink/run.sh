#!/usr/bin/env bash
# S7: builds the scratch crate and runs it inside a user and network namespace, so that the network of
# the host is never touched (as root: a plain network namespace).
#   bash spikes/rust-netlink/run.sh
set -eu
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
export PATH="$PATH:/usr/sbin:/sbin"
cargo build --release --quiet --manifest-path "$here/Cargo.toml"
bin="$here/target/release/spike-netlink"
if [ "$(id -u)" -eq 0 ]; then exec unshare -n "$bin"; fi
exec unshare -Urn "$bin"
