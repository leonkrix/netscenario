# Permissions: what a run needs, and how to give it

A run builds a small network out of Linux network namespaces: one namespace for each node, virtual Ethernet pairs for the links, and `netem` on each direction. Creating them needs a right. This page lists the ways to have it, what each costs, where each works, and how to set up a CI. `netscenario doctor` ([DOCTOR.md](DOCTOR.md)) tests all of it on your machine and prints the fix for what fails, so run it first.

**What netscenario never does:** it never changes an interface, a route or a firewall rule of the host. Everything lives in namespaces that it creates and that disappear with the run, also when the run is killed ([ARCHITECTURE.md](ARCHITECTURE.md), D-022). The supervisor of a run refuses to start in the network namespace of the host.

## What is needed

| What | Why | Where it comes from |
| --- | --- | --- |
| The tools `ip` and `tc` | Links and `netem` | Package `iproute2`. Version 6.6.0 or newer if a scenario has a `seed`, see [COVERAGE.md](COVERAGE.md) |
| The tools `unshare`, `nsenter` and `prlimit` | Namespaces, and the memory limit of the programs of a run | Package `util-linux` |
| The kernel modules `veth` and `sch_netem` | The links and the impairments | Part of the kernel; on Ubuntu cloud kernels `sch_netem` is in `linux-modules-extra-$(uname -r)` |
| The right to create namespaces | See the next section | See the next section |

Python needs nothing more: the wheel carries the two programs of a run (`netscenario` and `netscenario-agent`), and the Python process itself does not need any right, because the namespaces are created by a child process ([PYTHON.md](PYTHON.md)).

## The ways to get the right

`doctor` tests two ways with the exact command that a run uses, and a run chooses the same way as `doctor` reports (D-035): **its own rights if the process has them and they work, else a user namespace.** A process has rights of its own if it is root, or if it has `CAP_NET_ADMIN` and `CAP_SYS_ADMIN` in its ambient set (below).

### 1. An unprivileged user namespace (the default)

`unshare --user --map-root-user` makes you root *inside* a new user namespace, and there you may create network namespaces. Nothing is installed, no `sudo`, no change to the host. This is the way for a laptop and the preferred way for a CI.

| System | Works |
| --- | --- |
| Debian 13 | by default (measured, kernel 6.12) |
| Ubuntu 22.04 | by default (measured on the GitHub-hosted runner, kernel 6.8, and in the CI of this project) |
| Ubuntu 24.04 and newer | **not by default**: AppArmor restricts it. One setting, below (measured on the GitHub-hosted runner, kernel 7.0) |
| Other distributions | not tested. `doctor` reads the three settings that are known to forbid it and names them |

The three settings, and what to do if `doctor` names one:

| Setting | Meaning | Fix |
| --- | --- | --- |
| `kernel.apparmor_restrict_unprivileged_userns = 1` | Ubuntu 24.04 and newer | `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0`; to keep it after a reboot, put the line `kernel.apparmor_restrict_unprivileged_userns=0` in a file in `/etc/sysctl.d/` (for example `60-netscenario.conf`) |
| `kernel.unprivileged_userns_clone = 0` | Some Debian and Arch kernels | `sudo sysctl -w kernel.unprivileged_userns_clone=1` |
| `user.max_user_namespaces = 0` | Namespaces switched off | `sudo sysctl -w user.max_user_namespaces=15000` |

**What the Ubuntu setting costs.** Ubuntu restricts unprivileged user namespaces because they widen the attack surface of the kernel for every program on the machine. The sysctl lifts that for the whole machine, not for netscenario alone. On a developer laptop or a throw-away CI runner this is usually fine; on a shared or hardened server use the next way instead. A narrower way is an AppArmor profile that allows only the program that creates the namespace (`unshare`, not netscenario); this project has not tried it, and only the sysctl is tested.

### 2. The rights of the process: root, or two ambient capabilities

Without a user namespace, a process that has `CAP_NET_ADMIN` and `CAP_SYS_ADMIN` creates the namespaces itself. Root has them:

```bash
sudo netscenario run scenario.yaml
```

`doctor` reports this way as `own rights`.

- **Root** is tested: the CI of this project runs the whole Kernel backend a second time with `sudo`, so that both ways stay working (D-022). A container that runs as root with `SYS_ADMIN` is root in this sense. If root is refused and a user namespace works (a container without `SYS_ADMIN` on a kernel that allows user namespaces), the run uses the user namespace.
- **An ordinary user with the two capabilities in the *ambient* set** is tested too: the CI starts the tests as an ordinary user with `setpriv --ambient-caps` and checks that the run takes its own rights, and that it does not with only one of the capabilities or with none. The ambient set is the one that survives `exec`, and a run starts `unshare`, which starts the supervisor; each has only what the one before gave it. Ways to give a process ambient capabilities:

  ```bash
  # From root: run as the user "ci", with the two capabilities as ambient capabilities
  sudo setpriv --reuid=ci --regid=ci --init-groups \
      --inh-caps=+net_admin,+sys_admin --ambient-caps=+net_admin,+sys_admin \
      netscenario run scenario.yaml
  ```

  or in a systemd unit `AmbientCapabilities=CAP_NET_ADMIN CAP_SYS_ADMIN`. **A workload with `udp-checksum: false` also needs `CAP_NET_RAW`** (its sender is a raw socket): add `+net_raw` to both lists above, or run in a user namespace, where it is there. Without it `doctor` warns (`raw-socket`) and the run refuses the workload before it builds anything. **Capabilities set on the file** (`setcap` on `netscenario`) do not help: they are gone for the programs that netscenario starts, and `doctor` shows `own rights` as not available. In Docker, `--cap-add` gives capabilities to root in the container; whether they are ambient for another user (`--user`) is not tested.

With `sudo`, the environment matters: `sudo` drops the `PATH`, so a newer `tc` or a `netscenario` in `~/.cargo/bin` is not found. Use `sudo env "PATH=$PATH" netscenario ...` or give the full path.

### Which to choose

| Situation                                | Use                                                                           |
| ---------------------------------------- | ----------------------------------------------------------------------------- |
| Your own machine, Debian or Ubuntu 22.04 | 1, nothing to do                                                              |
| Your own machine, Ubuntu 24.04           | 1 after the sysctl, or 2 with `sudo` if you do not want to change the machine |
| A CI on GitHub-hosted runners            | 1, see below                                                                  |
| A shared or hardened server              | 2, as root or with the two ambient capabilities                               |
| A container                              | See below                                                                     |

## Containers

A container is a special case, because Docker's default profile (seccomp) forbids `unshare`, and so no namespace can be created.

- `NET_ADMIN` alone is **not enough**.
- It works with `SYS_ADMIN` and a seccomp profile that allows `unshare`; the simple way, and the one that `doctor` suggests, is:

  ```bash
  docker run --cap-add SYS_ADMIN --security-opt seccomp=unconfined ...
  ```

  (measured in spike S6; a narrower seccomp profile that only allows `unshare` and `setns` is not tested.)
- Rootless inside a container, Kubernetes, Podman and other runtimes: not tested. The requirements are the same as on a machine: the tools above, and the right to create namespaces.
- A container without extra rights cannot run the Kernel backend. A backend that works there (the proxy for UDP) is planned for release 0.5 ([COVERAGE.md](COVERAGE.md), "Where it can run").

## A CI

### GitHub Actions, hosted runners

| Runner image   | Namespaces without root | `tc`           |
| -------------- | ----------------------- | -------------- |
| `ubuntu-22.04` | by default              | iproute2 5.15  |
| `ubuntu-24.04` | after the step below    | iproute2 6.1.0 |

Add this step before the tests (it does nothing on images that do not have the setting):

```yaml
- name: Allow unprivileged user namespaces
  run: |
    if sysctl -n kernel.apparmor_restrict_unprivileged_userns >/dev/null 2>&1; then
      sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
    fi
```

Both `tc` versions are older than 6.6.0 and do not know the `seed` of `netem`. A scenario that has a `seed` and loss is then **refused** with an error that says so and how to get out of it. Your choices:

1. Install a newer iproute2 (the CI of this project builds 6.15.0 from the source tarball and checks its checksum: [`.github/actions/namespaces/action.yml`](../.github/actions/namespaces/action.yml)). Note that a `tc` you built yourself looks for the distribution tables (`normal.dist`) in `/usr/lib/tc`: set `TC_LIB_DIR` to the `netem` folder of the build, as that action does.
2. Leave out `seed`. The scenario runs, gets a drawn seed that the result records, and says that its loss is not reproducible.

The pytest plugin **fails** a test on a machine that cannot run scenarios when the variable `CI` is set (GitHub Actions sets it), and skips on a laptop, so a missing permission cannot hide as a green run ([PYTEST.md](PYTEST.md)).

### Other CI systems

GitLab, Jenkins, CircleCI and others are not tested. The requirements are the ones of this page: the tools, the modules, and one of the two ways. Make `netscenario doctor` the first step of the job: its exit code is 0 when scenarios can run, 3 when not, and its output says what to change. For the runners of a CI that run in containers, see above.

## Limits that a run sets itself

A run uses at most 8 namespaces (release 0.1 needs two), ends after the duration of the scenario plus 60 seconds, and limits the memory of the agents it starts to 1 GiB with `prlimit` (the unprivileged way has no cgroups). A run that hangs is ended by a watchdog and cleaned up; `netscenario clean` is the safety net for what a hard kill might leave ([DOCTOR.md](DOCTOR.md)).
