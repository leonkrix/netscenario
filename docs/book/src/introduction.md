# netscenario

Code that talks over a network behaves differently on a good link and on a bad one: a video call over a mobile network that loses bursts of packets, a game on a link with 150 ms of delay, a protocol over a path that goes down for two seconds. netscenario makes this a normal, repeatable test. You **describe a network** (nodes, links, delay, loss, rate, changes over time), **run traffic or your own program in it**, **measure every packet**, and **assert on the numbers**.

The network is made of Linux network namespaces and the real kernel queues (`netem`, `tbf`, `htb`), so the packets of your program take a real path. Nothing outside the namespaces is touched, and on most machines nothing needs root.

## Where to start

Everybody starts with [Install and check the machine](install.md) and [Your first run](first-run.md). Then pick the path that is yours:

| You are... | You want to... | Read |
| --- | --- | --- |
| **A tester or developer** | check in a Python test that your code survives a bad link | [Testing with pytest](pytest-first-test.md) |
| **A network engineer** | write a scenario in a file, run it from the command line, read the numbers | [Scenario files and the command line](files.md) |
| **The maintainer of a CI** | run scenarios on every pull request, keep the reports, know when a machine is too noisy | [In a CI](ci.md) |

The chapters under [Shapes of a network](shape-one-link.md) show, one by one, the networks you can describe: a single link, a chain, a star, a dumbbell, several paths, a link that moves.

## What you can rely on

- **A scenario is data.** A short YAML file or a Python builder; the same scenario gives the same result structure everywhere.
- **Nothing is approximated silently.** If the machine cannot reproduce something faithfully, the run is refused with the feature, the backend and a way out ([`netscenario capabilities`](install.md)).
- **A result carries its provenance.** The seed, the kernel, the tool versions and the precision of the timers of the machine are in the result, and a recorded run can be replayed.

## About this book

Every scenario in this book is read by the same reader as a run, and the short ones are run in the CI of every pull request, so what you see here works with the version of the program that the book was built from. The pages explain and show; the exact definition of every key, field and option lives in the [reference documents](reference.md).
