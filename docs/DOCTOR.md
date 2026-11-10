# `netscenario doctor` and `netscenario clean`

Both commands belong to the Kernel backend (D-005, D-022). They are for the person who sets up a machine and for the CI that has to know why a run does not start. Everything they do happens in namespaces of their own or reads `/proc`; nothing touches the network of the host (SAF-01).

## `netscenario doctor`

```bash
netscenario doctor            # text for people
netscenario doctor --json     # the same checks as JSON
netscenario doctor --timers   # also the precision of the timers (about two seconds)
```

It looks at the machine and prints one line per check, in four groups (five with `--timers`, below), and for everything that is not as it should be a line `fix:` that says what to do. The exit code is **0** if scenarios can run here and **3** if something has to be fixed first (the codes are listed in `netscenario --help`). A warning does not change the exit code: it means a scenario can run, with a limit that the line names.

| Group | Check (`id`) | What it tests | Not as it should be |
| --- | --- | --- | --- |
| Tools | `tool-ip`, `tool-tc`, `tool-unshare`, `tool-nsenter`, `tool-prlimit` | The tool is found (in the `PATH` and in `/usr/sbin`, `/sbin`, `/usr/local/sbin`, `/usr/bin`, `/bin`; the line says where it looked) and prints its version (`prlimit` limits the memory of the programs of a run) | `FAIL`, with the package to install |
| Tools | `tool-kill` | Needed by `clean` only | `warn` |
| Rights | `way-user-namespace`, `way-own-rights` | Whether the exact `unshare` command of a run works, once in an unprivileged user namespace and once with the rights of the process (root or the capabilities `CAP_NET_ADMIN` and `CAP_SYS_ADMIN`). The probe runs inside the namespaces it creates | `info` for the way that is not available, with the answer of `unshare` |
| Rights | `can-create-namespaces` | At least one of the two ways works | `FAIL`; the fix names what applies to this machine: the AppArmor setting of Ubuntu 24.04, `kernel.unprivileged_userns_clone`, `user.max_user_namespaces`, what a container needs, or running as root |
| Kernel and tc | `veth` | A virtual Ethernet pair can be created | `FAIL` (module `veth`) |
| Kernel and tc | `network` | a node can switch on `ip_forward` and have a route through a next hop (`ip route add ... via ...`), which a network of several nodes needs (D-078) | `FAIL`: scenarios with routers or several links are refused, point to point still runs |
| Kernel and tc | `ipv6` | a node can have IPv6 addresses (`/proc/sys/net/ipv6` exists and an address can be added) | `warn`: scenarios with `ip: v6` or `ip: dual` are refused; everything else runs (D-081) |
| Kernel and tc | `netem` | `tc` and the kernel accept delay, loss and rate | `FAIL` (module `sch_netem`; on Ubuntu cloud kernels `linux-modules-extra`) |
| Kernel and tc | `tables` | `tc` finds the tables of the distributions (`normal.dist`, `pareto.dist`, `paretonormal.dist`). A `tc` that was built from source looks in `/usr/lib/tc`; `TC_LIB_DIR` points it elsewhere | `warn`: a jitter with the distribution that is missing fails (it is refused before the network is built, with the answer of `tc`) |
| Kernel and tc | `custom-tables` | `tc` loads a table of our own, `NAME.dist` from the directory of `TC_LIB_DIR` (what a jitter from a file of samples needs, D-084). The tools `mktemp` and `yes` make the test table | `warn`: a scenario with `distribution: table` is refused |
| Kernel and tc | `seed` | iproute2 is 6.6.0 or newer **and** `tc` accepts a seed (ENV-07) | `warn`: a scenario with `seed` is refused, and one without is not reproducible |
| Kernel and tc | `models` | `tc` and the kernel accept the Gilbert-Elliott and the four-state loss, reordering, duplication and corruption | `warn`: a scenario that uses a refused model is refused by the run |
| Kernel and tc | `queues` | the fabric (two ports joined by a redirect, `cls_u32` and `act_mirred`) and the tree of a queue (a `netem` with a delay of 1 microsecond, an `htb` class) can be built, which a direction with a `queue` needs (D-079) | `warn`: scenarios with a `queue` are refused here; `info` if the veth pair could not be made |
| Kernel and tc | `disciplines` | the kernel accepts `pfifo`, `bfifo`, `codel`, `fq_codel` and `pie` as the leaf of the class (D-082) | `warn` with the answer of the kernel for each one it refuses; a scenario with that discipline is refused before the network is built; `info` if the tree of a queue cannot be built |
| Kernel and tc | `duplicate-tree` | the kernel accepts a duplicating `netem` next to another `netem` in one tree, which a direction with a rate and a jitter or a reordering needs (D-052) | `warn`: a direction with `duplicate`, a `rate` and a `jitter` or `reorder` is refused by the run, with the answer of the kernel; `info` if the first queue of the tree is refused already |
| Kernel and tc | `raw-socket` | a node has `CAP_NET_RAW` in its effective capabilities, which a stream without the checksum of UDP needs (its sender is a raw socket, D-056) and which the naming of damaged packets by sequence number needs (the receiver reads the interface with a packet socket, D-057). All of the capabilities in a user namespace; in a run with the rights of the process, what the process has | `warn`: a workload with `udp-checksum: false` is refused by the run, and the damaged packets of a link with `corrupt` are counted but not named; the fix names root, `setpriv --ambient-caps +net_raw`, `AmbientCapabilities=` and the user namespace |
| Features | one per row of [COVERAGE.md](COVERAGE.md) | What the backend declares for this machine (`Exact`, in part, or refused with the reason): by the version of `tc` and by what the probe above found out, so a model that the kernel refuses is a `warn` on its own line, with the answer of the kernel (D-068) | `warn` with the reason and the way out |

The steps after `veth` are not blamed on the kernel when no pair could be created, and nothing of the kernel is claimed when no way to get namespaces works.

### `--json`

```json
{
  "version": 1,
  "ok": true,
  "checks": [
    { "section": "tools", "id": "tool-tc", "status": "ok", "title": "tc",
      "detail": "/usr/sbin/tc (tc utility, iproute2-6.15.0)", "fix": null }
  ]
}
```

`status` is `ok`, `warn`, `fail` or `info`; `version` is the version of this form and changes when a field changes its meaning. `ok` is true when no check failed.

### `--timers`: how precise the timers of this machine are (ENV-05, D-097)

A packet leaves at the slot the plan gives it only as exactly as the machine wakes a sleeping core, and a step of a timeline is done only as fast as `tc` answers. How late that is depends on the processor, its idle states, the timer slack of the process and the load, so no figure can be promised; `--timers` measures this machine for about two seconds and adds a group **Timers**:

| Check (`id`) | What it measures | `warn` when |
| --- | --- | --- |
| `timer-clock` | The source of `CLOCK_MONOTONIC`, what a reading costs and the smallest step between two readings. A sender reads the clock for every packet. | a reading costs more than 1 microsecond (a `hpet` or `acpi_pm` source, a virtual machine that traps the clock) |
| `timer-sleep-200us`, `timer-sleep-1ms` | How much longer than asked a sleep of that length took: the median, the 99th percentile and the largest of 1,000 and 300 sleeps. It is the call the senders and the steps wait with. | the 99th percentile is later than 150 microseconds, the stretch a sender spins at the end: a later wake-up sends the packet late |
| `timer-slack` | The timer slack of the process (50 microseconds unless changed: the kernel may delay a timer this much, which is why a median of about 50 microseconds late is usual) and the governor of the first processor. | never (`info`) |
| `timer-load` | The load average of the last minute against the number of processors, and the runnable tasks. | the load is above the number of processors |
| `timer-tc-change` | What a change of the root qdisc costs through the `tc` that lives for a run (D-038): 500 changes of a `netem` in a network namespace of its own, a `pfifo` where the kernel has no `netem`. It is what a step of a timeline waits for. | the 99th percentile is above 2 milliseconds; `info` with the reason where no namespace can be made |

A late timer is a `warn` and never a `FAIL`: the exit code is the one of the rest of the report, and a run is not refused because of it. `--json` has the same checks and, next to them, the measurements under `timers` (`timers`, `tc_change`, or `tc_change_error`). From Python: `ns.doctor(timers=True)`.

Every run records a short version of the same measurement (about 40 milliseconds, taken just before the network is built) as `provenance.timers` ([RESULTS.md](RESULTS.md)), and the text and the page of a run have a line for it. What a run actually met is in `max_lateness_ns` of each workload and, for the steps, in `applied_ns - scheduled_ns` of the `events`; the report says what the machine does when it is idle of this run, those say what the run got.

### The gate: a run follows the probe (D-068)

The feature lines are the declaration of the backend, and a run uses the same one. The declaration follows the version of `tc` **and** a probe of the machine: the first time a run asks what the backend can do, it tries the probe once, in a throwaway namespace and in the way the run takes (about 15 ms), and what the kernel refuses is not declared, with its answer. So a scenario that needs it is refused before anything is built, with the line of the file, and not after the network is up with an error of `tc`.

- Not `netem` at all, or no virtual Ethernet pair: everything that needs it is refused.
- A model that the kernel refuses (the loss in bursts, reordering, duplication, corruption) or a seed that it does not accept: the feature is refused, and only that one.
- A jitter with the distribution `normal`, where `tc` finds no table: refused at the jitter (in a link or in a step of the timeline), and the fix names `TC_LIB_DIR`. The declaration of "Jitter" stays `exact`, because the uniform jitter works.
- A duplicating `netem` next to another one in a tree (D-052), and a stream without the checksum of UDP where a node has no `CAP_NET_RAW` (D-056): as before, from the same probe.

If the probe cannot be run (no `sh`, no way to get namespaces) nothing is known, and the declaration follows the version of `tc` alone; the run then finds out for itself, as it did before.

A run takes the way that `doctor` reports: its own rights if the process is root or has `CAP_NET_ADMIN` and `CAP_SYS_ADMIN` in its ambient set and they work, else the user namespace (D-035, [PERMISSIONS.md](PERMISSIONS.md)). The result of a run says which one it used (`provenance.backend_details.rights`).

## `netscenario clean`

```bash
netscenario clean --dry-run   # what would be ended
netscenario clean             # end it
netscenario clean --json
```

A run needs no cleaning: it lives in a PID namespace, and when its supervisor dies, even by `kill -9`, the kernel ends every process of the run and removes the network namespaces nobody holds any more (D-022). `clean` is the safety net for what that does not cover: a run that was started without the PID namespace (an older version, a holder started by hand). Nothing is named, so it finds such processes by the shape of their command line in `/proc`: a program whose name starts with `netscenario` and the arguments `hold <run id>` or `supervise <run id> <nodes>`, with a run id of three numbers.

- A run **with** a supervisor is never touched, whoever started it. The report counts these as `live_runs`.
- A run **without** one is a leftover; its holders are ended with `kill -KILL` after their command line was read once more (a reused process id is not hit). The network namespace goes with the last process.
- Only processes that the user may signal can be ended. One that cannot is listed with the reason, and the exit code is **3**.
- `--dry-run` changes nothing and exits with 0.
