# Running in a CI

A scenario is a test like any other, so it belongs in the CI of the project it tests. This page is what the maintainer of that CI needs: the permission a run needs, the first step that tells you at once when a machine is wrong, the reports, and how to keep a flaky result out.

## 1. Allow the namespaces

A run needs the right to create network namespaces and the tools `ip`, `tc`, `unshare`, `nsenter` and `prlimit`. On GitHub Actions the hosted runners give it, with one setting on `ubuntu-24.04`:

```yaml
- name: Allow unprivileged user namespaces
  run: |
    if sysctl -n kernel.apparmor_restrict_unprivileged_userns >/dev/null 2>&1; then
      sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
    fi
```

The step does nothing on images that do not have the setting. [Permissions](https://github.com/leonkrix/netscenario/blob/main/docs/PERMISSIONS.md) lists the other ways (root, two capabilities, a container) and what each costs.

## 2. Make `doctor` the first step

```yaml
- name: Can this machine run scenarios?
  run: netscenario doctor
```

The exit code is 0 when scenarios can run and 3 when not, and the output says what to change. A failing `doctor` on the first step of a job saves a long log in which every test says "no namespace".

The pytest plugin does the same by itself: on a machine that cannot run scenarios it **fails** the test when `CI` is set and skips it on a laptop, so a missing permission never turns into a green run.

## 3. Mind the `tc` version

The runner images ship an iproute2 older than 6.6.0, whose `tc` does not know the `seed` of `netem`. A scenario with a seed and loss is then **refused**, with a message that says so. Your choices are to install a newer iproute2 (the CI of this project builds 6.15.0 from the source and checks its checksum) or to leave out `seed`, in which case the run draws one, records it, and says that its loss is not reproducible.

## 4. Keep the reports

```yaml
- name: Run the scenario
  run: netscenario run scenarios/lte-like.yaml --repetitions 10 --out results/lte
- name: Reports
  if: always()
  run: |
    netscenario report results/lte --format md >> "$GITHUB_STEP_SUMMARY"
    netscenario report results/lte --format junit --out results/junit.xml
    netscenario report results/lte/run-001 --format html --out results/report.html
- uses: actions/upload-artifact@v4
  if: always()
  with:
    name: netscenario-reports
    path: results/
```

The Markdown goes to the summary of the job, the HTML is kept as an artifact, and the JUnit file feeds the step that shows test reports. (Pin the action to a commit, as in the workflows of this repository.) The uploaded directory also holds the raw result, so a failing run can be **replayed** on a developer's machine: download the artifact and run `netscenario replay results/lte/run-001`.

## 5. Exit codes

They are part of the interface:

| Code | Meaning |
| --- | --- |
| 0 | The command did what was asked. |
| 1 | A check did not hold: an `expect` of the file, or a replay that lost other packets. |
| 2 | The command line or the scenario is wrong, or it asks for something the backend cannot reproduce exactly. |
| 3 | The machine does not allow the run. Run `netscenario doctor`. |
| 4 | The run failed after it started. The network is torn down all the same. |

## 6. Keep a flaky claim out

A claim about a measurement is stable only if its bound sits far from the value on a noisy machine. Three habits keep a CI green for the right reasons:

- **Judge the mean of several runs**, not one run: `--repetitions 10` and a bound on the 95 % interval ([Statistics and checks](pytest-statistics.md)).
- **Derive the bound** from the model of the thing you test (a count: four standard deviations; a delay: its exact physical lower bound and room only upward) and write it next to the assertion.
- **Know the machine.** The result records the precision of the timers when the run began, and `doctor --timers` measures it on its own: [Noisy machines and timers](timers.md).

A run is a few seconds plus the setup of the network. Scenarios that only parse (`netscenario validate`) take milliseconds and make a good first job on every pull request.
