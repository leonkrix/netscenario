# Reports

`netscenario report <dir>` makes a report from a result directory: the directory of a run (the `--out` of `netscenario run`) or of a set of runs (`--repetitions`, [STATISTICS.md](STATISTICS.md)). It reads the files of the directory and nothing else, so the same directory gives the same bytes (CLI-02, RPT-02, RPT-03, RPT-04; decision [D-076](decisions/D-076.md), earlier [D-043](decisions/D-043.md)).

```text
netscenario report results/lte --format md >> "$GITHUB_STEP_SUMMARY"
netscenario report results/lte --format junit --out junit.xml
netscenario report results/lte --format html --out report.html
```

The report goes to standard output, or to the file of `--out`, for every format. The command exits with 0 whenever it could make the report, also for a JUnit report with failed cases: the failures are in the report. A directory that is not a result, or a file that cannot be read, is exit code 2 with the name of the file.

A sample of each format, made from a fixed run, is in [`docs/sample-report/`](sample-report/): [`report.md`](sample-report/report.md), [`junit.xml`](sample-report/junit.xml) and [`report.html`](sample-report/report.html). A test makes the three again and fails if they differ, so they show what the code writes today.

## Markdown (`--format md`)

Made for the summary of a CI job. For a run: a table with the backend, the seed, the duration and the machine, **one table for all workloads** (a column for each workload: packets, loss, reordering, the delay percentiles, jitter, mean throughput, the longest run of lost packets), the timeline with how late each step was, the time to recover, the programs with their metrics, and the notes of the run. A run that was interrupted, and a loss that does not repeat from its seed, are warnings in quotation blocks above the tables. For a set: the headline metrics of each workload (loss, the delay percentiles 50, 95 and 99, the throughput) with their intervals, all metrics in a collapsed block, the metrics that have no interval and why, and the seed of each run.

## JUnit (`--format junit`, RPT-02)

One test suite for the scenario. **One test case for each workload** (and one for each program of your own), with the metrics as `<property>` elements. The names are those of the pytest plugin, `netscenario.<scenario>.<workload>.<metric>`: `sent`, `received`, `lost`, `loss_ratio`, `reordered`, `delay_p50_s` and `delay_p99_s`, and a few more (`duplicates`, `delay_min_s`, `delay_p95_s`, `delay_max_s`, `jitter_mean_s`, `throughput_mean_bps`, `loss_runs_longest`). Times are in seconds. The suite has the version, the backend, the fingerprint of the scenario and the seed as properties.

A case is a **failure** in two cases:

- the run was **interrupted** (`timing.interrupted_ns`), so the numbers are those of the part that ran: every case of the run fails;
- the kernel **refused packets** of the workload (`sender.send_errors`) and no `down` step of the timeline explains it. A link that is shut on purpose refuses the packets sent into it, and that is what the step is for; the report then says so in the text of the case.

A warning about reproducibility is **not** a failure: it is in the text of the case. A result is only written for a run that went through, so a program with `on-exit: fail` that ended with an error has ended the run without a result and there is nothing to report; thresholds on a result (`expect`) come with a later release, and with them failures for numbers.

For a set: one case for each run, and the headline metrics of the set as properties of the suite (`netscenario.<scenario>.<metric>.mean`, `.low` and `.high`, and `repetitions` and `confidence`).

## HTML (`--format html`, RPT-03)

**One file** with inline CSS and inline SVG: no script, no font, no image and no address to fetch, so it can be attached to a CI run and opened anywhere. It follows the light and the dark scheme of the reader. A test checks that the page has no external reference. For a run, for each workload:

- the numbers of the Markdown table;
- the **throughput** over time: the payload that arrived in each bin and the payload that was sent in it (the dashed line);
- the **one-way delay** over time: the median and the 95th and 99th percentile of each bin, with a gap where nothing arrived;
- the **lost packets** over time: the share of the packets sent in each bin that did not arrive;
- the **distribution of the delays** of all packets, with the median and the 99th percentile marked;
- the events of the timeline as dashed lines with their numbers in the first three charts, and a table of the events and of the times to recover;
- the bins as a table in a collapsed block (the first 200).

For a set: for each headline metric a drawing with **one dot for each run**, the interval around the mean as a bar and the mean as a line, and the table of all metrics.

The charts need the series and the records of the run: `report --format html` reads `series-<id>.jsonl` and `flow-<n>.records` from the directory, and says which file is missing. Results of a version without a series get a page without the charts over time. The HTML is a file for people; the data are in `result.json` and `runset.json`.

## In a CI job

GitHub Actions: the Markdown goes to the summary of the job, the HTML is kept as an artifact, and the JUnit file to the step that shows test reports.

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

(Pin the action to a commit as in the workflows of this repository.) `report` for a directory of a set needs no run to be present beyond `runset.json`; the HTML of one run needs the files of that run.

## What a report does not do

- It does not judge. The failures of the JUnit report are those above; numbers are judged by your test, with an interval for a set ([STATISTICS.md](STATISTICS.md)).
- It does not compare two runs (a later release) and does not draw the series of a set over time: a set is a table and a dot plot of its runs.
- It is made from the directory as it is: a report made after files were moved or edited says what the files say.
