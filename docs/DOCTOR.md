# `netscenario doctor` and `netscenario clean`

Both commands belong to the Kernel backend (D-005, D-022). They are for the person who sets up a machine and for the CI that has to know why a run does not start. Everything they do happens in namespaces of their own or reads `/proc`; nothing touches the network of the host (SAF-01).

## `netscenario doctor`

```bash
netscenario doctor            # text for people
netscenario doctor --json     # the same checks as JSON
```

It looks at the machine and prints one line per check, in four groups, and for everything that is not as it should be a line `fix:` that says what to do. The exit code is **0** if scenarios can run here and **3** if something has to be fixed first (the codes are listed in `netscenario --help`). A warning does not change the exit code: it means a scenario can run, with a limit that the line names.

| Group | Check (`id`) | What it tests | Not as it should be |
| --- | --- | --- | --- |
| Tools | `tool-ip`, `tool-tc`, `tool-unshare`, `tool-nsenter`, `tool-prlimit` | The tool is found (in the `PATH` and in `/usr/sbin`, `/sbin`, `/usr/local/sbin`, `/usr/bin`, `/bin`; the line says where it looked) and prints its version (`prlimit` limits the memory of the programs of a run) | `FAIL`, with the package to install |
| Tools | `tool-kill` | Needed by `clean` only | `warn` |
| Rights | `way-user-namespace`, `way-own-rights` | Whether the exact `unshare` command of a run works, once in an unprivileged user namespace and once with the rights of the process (root or the capabilities `CAP_NET_ADMIN` and `CAP_SYS_ADMIN`). The probe runs inside the namespaces it creates | `info` for the way that is not available, with the answer of `unshare` |
| Rights | `can-create-namespaces` | At least one of the two ways works | `FAIL`; the fix names what applies to this machine: the AppArmor setting of Ubuntu 24.04, `kernel.unprivileged_userns_clone`, `user.max_user_namespaces`, what a container needs, or running as root |
| Kernel and tc | `veth` | A virtual Ethernet pair can be created | `FAIL` (module `veth`) |
| Kernel and tc | `netem` | `tc` and the kernel accept delay, loss and rate | `FAIL` (module `sch_netem`; on Ubuntu cloud kernels `linux-modules-extra`) |
| Kernel and tc | `tables` | `tc` finds the tables of the distributions (`normal.dist`). A `tc` that was built from source looks in `/usr/lib/tc`; `TC_LIB_DIR` points it elsewhere | `warn`: a jitter with `distribution normal` fails |
| Kernel and tc | `seed` | iproute2 is 6.6.0 or newer **and** `tc` accepts a seed (ENV-07) | `warn`: a scenario with `seed` is refused, and one without is not reproducible |
| Kernel and tc | `models` | `tc` and the kernel accept the Gilbert-Elliott and the four-state loss, reordering and duplication | `warn`: a scenario that uses a refused model is refused by the run |
| Kernel and tc | `duplicate-tree` | the kernel accepts a duplicating `netem` next to another `netem` in one tree, which a direction with a rate and a jitter or a reordering needs (D-052) | `warn`: a direction with `duplicate`, a `rate` and a `jitter` or `reorder` is refused by the run, with the answer of the kernel; `info` if the first queue of the tree is refused already |
| Features | one per row of [COVERAGE.md](COVERAGE.md) | What the backend declares for this machine (`Exact`, in part, or refused with the reason) | `warn` with the reason and the way out |

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

### Known limit

The feature lines come from the declaration of the backend, which depends on the version of `tc` only. Missing tables for `distribution normal` show as the warning of `tables`; the declaration of "Jitter" stays `exact`, so a scenario with a normal jitter on such a machine is refused when the run applies it (the error names the table), not before. Making the declaration depend on a probe is open.

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
